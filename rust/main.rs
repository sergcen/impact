use monorepa_impact::analysis::{
    AffectedResult, DependentResult, analyze_affected, query_graph, query_snapshot,
};
use monorepa_impact::config::{DEFAULT_CACHE_DIRECTORY, load_config};
use monorepa_impact::git::{collect_changed_files, collect_changed_specifiers};
use monorepa_impact::graph::{BuildOptions, Snapshot, build_graph, update_cached_graph};
use monorepa_impact::workspaces::discover_workspaces;
use std::env;
use std::io::{self, BufWriter, Write};
use std::process::{Command, ExitCode, Stdio};

const HELP: &str = "Usage: monorepa-impact <command> [options]

Commands:
  affected              Find workspaces affected by Git changes
  dependents <file>...  Find modules that depend on one or more files

Run 'monorepa-impact <command> --help' for command-specific options.

Global options:
  -V, --version         Show the version
  -h, --help            Show this help";

const AFFECTED_HELP: &str =
    "Usage: monorepa-impact affected [options] [-- command with {workspaces}]

Find workspaces affected by the Git diff and current working tree.

Options:
  --base <ref>          Compare HEAD with this Git ref
  --config <path>       Use a JSON/JSONC affected config
  --json                Print machine-readable output
  --explain             Print dependency chains for projects
  --no-cache            Do not read or write the graph cache
  --rebuild-cache       Rebuild the dependency graph cache
  --strict-cache        Force a graph rebuild from the current working tree
  --trust-cache         Skip automatic working-tree validation for this query
  -h, --help            Show this help";

const DEPENDENTS_HELP: &str = "Usage: monorepa-impact dependents <file> [<file>...] [options]

Find direct or transitive modules that depend on the target files.

Options:
  --specifier <name>    Follow one imported/exported binding (repeatable)
  --direct              Return only immediate importers
  --config <path>       Use a JSON/JSONC affected config
  --json                Print machine-readable output
  --explain             Print dependency chains for modules
  --no-cache            Do not read or write the graph cache
  --rebuild-cache       Rebuild the dependency graph cache
  --strict-cache        Force a graph rebuild from the current working tree
  --trust-cache         Skip automatic working-tree validation for this query
  -h, --help            Show this help";

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum QueryMode {
    #[default]
    Affected,
    Dependents,
}

#[derive(Default)]
struct Options {
    base: Option<String>,
    command: Vec<String>,
    config: Option<String>,
    dependents: Vec<String>,
    direct: bool,
    explain: bool,
    help: bool,
    json: bool,
    mode: QueryMode,
    explicit_mode: bool,
    rebuild: bool,
    specifiers: Vec<String>,
    strict: bool,
    trust_requested: bool,
    use_cache: bool,
    version: bool,
}

fn read_value(args: &[String], index: &mut usize, option: &str) -> Result<String, String> {
    if let Some(value) = args[*index].strip_prefix(&format!("{option}=")) {
        return Ok(value.to_string());
    }
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        use_cache: true,
        ..Options::default()
    };
    let mut index = match args.first().map(String::as_str) {
        Some("affected") => {
            options.mode = QueryMode::Affected;
            options.explicit_mode = true;
            1
        }
        Some("dependents") => {
            options.mode = QueryMode::Dependents;
            options.explicit_mode = true;
            1
        }
        _ => 0,
    };
    while index < args.len() {
        let argument = &args[index];
        match argument.as_str() {
            "--help" | "-h" => options.help = true,
            "--json" => options.json = true,
            "--explain" => options.explain = true,
            "--direct" => options.direct = true,
            "--no-cache" => options.use_cache = false,
            "--rebuild-cache" => options.rebuild = true,
            "--strict-cache" => options.strict = true,
            "--trust-cache" => options.trust_requested = true,
            "--version" | "-V" => options.version = true,
            "--" => {
                if options.explicit_mode && options.mode == QueryMode::Dependents {
                    options.dependents.extend_from_slice(&args[index + 1..]);
                } else {
                    options.command.extend_from_slice(&args[index + 1..]);
                }
                break;
            }
            _ if argument == "--base" || argument.starts_with("--base=") => {
                options.base = Some(read_value(args, &mut index, "--base")?);
            }
            _ if argument == "--config" || argument.starts_with("--config=") => {
                options.config = Some(read_value(args, &mut index, "--config")?);
            }
            _ if argument == "--dependents" || argument.starts_with("--dependents=") => {
                if options.explicit_mode {
                    return Err(
                        "--dependents is a legacy option; use 'dependents <file>' instead".into(),
                    );
                }
                options.mode = QueryMode::Dependents;
                options
                    .dependents
                    .push(read_value(args, &mut index, "--dependents")?);
            }
            _ if argument == "--specifier" || argument.starts_with("--specifier=") => {
                options
                    .specifiers
                    .push(read_value(args, &mut index, "--specifier")?);
            }
            _ if argument.starts_with('-') => {
                return Err(format!("Unknown option: {argument}"));
            }
            _ if options.explicit_mode && options.mode == QueryMode::Dependents => {
                options.dependents.push(argument.clone());
            }
            _ if options.explicit_mode => {
                return Err(format!(
                    "Unexpected argument for affected: {argument}; child commands must follow --"
                ));
            }
            _ => {
                return Err(format!(
                    "Unknown command: {argument}; expected 'affected' or 'dependents'"
                ));
            }
        }
        index += 1;
    }
    if options.help || options.version {
        return Ok(options);
    }
    let dependent_query = options.mode == QueryMode::Dependents;
    if dependent_query && options.dependents.is_empty() {
        return Err("dependents requires at least one <file>".into());
    }
    if options.direct && !dependent_query {
        return Err("--direct is only valid with dependents".into());
    }
    if !options.specifiers.is_empty() && !dependent_query {
        return Err("--specifier is only valid with dependents".into());
    }
    if dependent_query && options.base.is_some() {
        return Err("--base is only valid with affected".into());
    }
    if dependent_query && !options.command.is_empty() {
        return Err("A child command is only valid with affected".into());
    }
    if options.trust_requested && options.strict {
        return Err("--trust-cache cannot be combined with --strict-cache".into());
    }
    if options.trust_requested && !options.use_cache {
        return Err("--trust-cache cannot be combined with --no-cache".into());
    }
    Ok(options)
}

fn write_json(value: &impl serde::Serialize) -> Result<(), String> {
    let stdout = io::stdout();
    let mut output = BufWriter::new(stdout.lock());
    serde_json::to_writer(&mut output, value).map_err(|error| error.to_string())?;
    output.write_all(b"\n").map_err(|error| error.to_string())
}

fn print_dependent(result: &DependentResult, options: &Options) -> Result<(), String> {
    if options.json {
        return write_json(result);
    }
    if options.explain {
        for file in &result.dependent_files {
            println!("{file}");
            for reason in result.reasons.get(file).into_iter().flatten() {
                let binding = reason
                    .imported_specifier
                    .as_ref()
                    .map(|imported| {
                        reason
                            .exported_specifier
                            .as_ref()
                            .map(|exported| format!(" [{imported} -> {exported}]"))
                            .unwrap_or_else(|| format!(" [{imported}]"))
                    })
                    .unwrap_or_default();
                let suffix = reason
                    .via
                    .as_ref()
                    .map(|via| format!(" via {via}{binding}"))
                    .unwrap_or(binding);
                println!("  {}: {}{}", reason.kind, reason.file, suffix);
            }
        }
    } else if !result.dependent_files.is_empty() {
        println!("{}", result.dependent_files.join("\n"));
    }
    Ok(())
}

fn print_affected(result: &AffectedResult, options: &Options) -> Result<(), String> {
    if options.json {
        return write_json(result);
    }
    if options.explain {
        for project in &result.projects {
            println!("{project}");
            for reason in result.reasons.get(project).into_iter().flatten() {
                let suffix = reason
                    .via
                    .as_ref()
                    .map(|via| format!(" via {via}"))
                    .unwrap_or_default();
                println!("  {}: {}{}", reason.kind, reason.file, suffix);
            }
        }
    } else if options.command.is_empty() && !result.projects.is_empty() {
        println!("{}", result.projects.join("\n"));
    }
    Ok(())
}

#[cfg(unix)]
fn shell(command: &str) -> Command {
    let mut shell = Command::new("/bin/sh");
    shell.arg("-c").arg(command);
    shell
}

#[cfg(windows)]
fn shell(command: &str) -> Command {
    let executable = env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into());
    let mut shell = Command::new(executable);
    shell.args(["/D", "/S", "/C", command]);
    shell
}

#[cfg(not(any(unix, windows)))]
fn shell(command: &str) -> Command {
    let mut shell = Command::new("sh");
    shell.arg("-c").arg(command);
    shell
}

fn execute_command(result: &AffectedResult, options: &Options) -> Result<i32, String> {
    if options.command.is_empty() || result.projects.is_empty() {
        return Ok(0);
    }
    let filters = result
        .projects
        .iter()
        .map(|project| format!("--filter={project}"))
        .collect::<Vec<_>>()
        .join(" ");
    let command = options.command.join(" ").replace("{workspaces}", &filters);
    println!("Affected projects:\n{}\n", result.projects.join("\n"));
    println!("Selective command: {command}");
    let status = shell(&command)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| format!("Failed to start selective command: {error}"))?;
    Ok(status.code().unwrap_or(1))
}

fn run() -> Result<i32, String> {
    let args: Vec<String> = env::args().skip(1).collect();
    let options = parse_args(&args)?;
    if options.help {
        let help = match (options.explicit_mode, options.mode) {
            (true, QueryMode::Affected) => AFFECTED_HELP,
            (true, QueryMode::Dependents) => DEPENDENTS_HELP,
            _ => HELP,
        };
        println!("{help}");
        return Ok(0);
    }
    if options.version {
        println!("monorepa-impact {}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }
    let cwd = env::current_dir().map_err(|error| error.to_string())?;
    let dependent_query = options.mode == QueryMode::Dependents;
    if dependent_query
        && options.use_cache
        && !options.rebuild
        && !options.strict
        && options.config.is_none()
    {
        let cache_directory = cwd.join(DEFAULT_CACHE_DIRECTORY);
        if let Ok(mut snapshot) = Snapshot::load(&cwd, &cache_directory, options.trust_requested)
            && let Ok(result) = query_snapshot(
                &cwd,
                &mut snapshot,
                &options.dependents,
                &options.specifiers,
                options.direct,
            )
        {
            print_dependent(&result, &options)?;
            return Ok(0);
        }
    }
    let loaded = load_config(&cwd, options.config.as_deref())?;
    let use_cache = options.use_cache && loaded.config.cache.enabled;
    let incremental =
        (use_cache && !options.rebuild && !options.strict && !options.trust_requested)
            .then(|| update_cached_graph(&cwd, &loaded.config, loaded.path.as_deref()))
            .transpose()
            .ok()
            .flatten();
    let (graph, workspaces) = if let Some(cached) = incremental {
        cached
    } else {
        let workspaces = discover_workspaces(&cwd, &loaded.config)?;
        let graph = build_graph(
            &cwd,
            &loaded.config,
            loaded.path.as_deref(),
            &workspaces,
            BuildOptions {
                rebuild: options.rebuild,
                strict: options.strict,
                trust_cache: options.trust_requested,
                use_cache,
            },
        )?;
        (graph, workspaces)
    };
    if dependent_query {
        let result = query_graph(
            &cwd,
            &graph,
            &workspaces,
            &options.dependents,
            &options.specifiers,
            options.direct,
        )?;
        print_dependent(&result, &options)?;
        return Ok(0);
    }
    let base = options
        .base
        .clone()
        .unwrap_or_else(|| loaded.config.base.clone());
    let changed_files = collect_changed_files(&cwd, &base)?;
    let changed_specifiers = collect_changed_specifiers(&cwd, &base, &changed_files);
    let result = analyze_affected(
        &graph,
        &workspaces,
        &loaded.config,
        base,
        changed_files,
        changed_specifiers,
    );
    print_affected(&result, &options)?;
    execute_command(&result, &options)
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code as u8),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
