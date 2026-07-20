use crate::config::Config;
use crate::model::{Dependency, Project, ResolutionWatch, Workspaces};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

const SOURCE_EXTENSIONS: [&str; 21] = [
    ".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs", ".json", ".css", ".scss",
    ".less", ".svg", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".ico", ".woff", ".woff2",
];

#[derive(Clone, Default)]
struct PathMapping {
    base_dir: PathBuf,
    pattern: String,
    targets: Vec<String>,
}

#[derive(Clone, Default)]
struct ProjectResolution {
    base_url: Option<PathBuf>,
    paths: Vec<PathMapping>,
}

pub struct Resolver<'a> {
    config: &'a Config,
    cwd: &'a Path,
    known_files: &'a HashSet<String>,
    project_resolutions: HashMap<String, ProjectResolution>,
    workspaces: &'a Workspaces,
    workspace_names: Vec<String>,
}

impl<'a> Resolver<'a> {
    pub fn new(
        config: &'a Config,
        cwd: &'a Path,
        known_files: &'a HashSet<String>,
        workspaces: &'a Workspaces,
    ) -> Self {
        let mut workspace_names: Vec<String> = workspaces
            .projects
            .iter()
            .map(|project| project.name.clone())
            .collect();
        workspace_names.sort_by_key(|name| std::cmp::Reverse(name.len()));
        let project_resolutions = workspaces
            .projects
            .iter()
            .map(|project| {
                (
                    project.name.clone(),
                    parse_tsconfig(
                        &cwd.join(&project.dir).join("tsconfig.json"),
                        &mut HashSet::new(),
                    ),
                )
            })
            .collect();
        Self {
            config,
            cwd,
            known_files,
            project_resolutions,
            workspaces,
            workspace_names,
        }
    }

    pub fn resolve(&self, importer: &str, dependency: &Dependency) -> Dependency {
        self.resolve_with_watches(importer, dependency).0
    }

    pub fn resolve_with_watches(
        &self,
        importer: &str,
        dependency: &Dependency,
    ) -> (Dependency, Vec<ResolutionWatch>) {
        let mut result = dependency.clone();
        let mut watched = vec![];
        let specifier = dependency
            .specifier
            .split(['?', '#'])
            .next()
            .unwrap_or(&dependency.specifier);
        if specifier.is_empty()
            || specifier.starts_with("node:")
            || specifier.starts_with("http:")
            || specifier.starts_with("https:")
            || specifier.starts_with("data:")
        {
            return (result, watched);
        }
        if specifier.starts_with('.') {
            let importer_directory = Path::new(importer)
                .parent()
                .unwrap_or_else(|| Path::new(""));
            let candidate = normalize_path(importer_directory.join(specifier))
                .to_string_lossy()
                .replace('\\', "/");
            if let Some(target) =
                resolve_file_candidate_watched(&candidate, self.known_files, &mut watched)
            {
                result.target = Some(target);
            } else {
                result.unresolved = Some(dependency.specifier.clone());
            }
            return (result, watched);
        }
        if let Some(workspace_name) = self
            .workspace_names
            .iter()
            .find(|name| specifier == name.as_str() || specifier.starts_with(&format!("{name}/")))
            && let Some(project) = self.workspaces.project_by_name(workspace_name)
        {
            return (
                self.resolve_workspace(project, specifier, result, &mut watched),
                watched,
            );
        }
        if let Some(project) = self.workspaces.project_for_file(importer)
            && let Some(resolution) = self.project_resolutions.get(&project.name)
        {
            for mapping in &resolution.paths {
                let Some(wildcard) = match_path_pattern(&mapping.pattern, specifier) else {
                    continue;
                };
                for target in &mapping.targets {
                    let target = target.replace('*', &wildcard);
                    let absolute = normalize_path(mapping.base_dir.join(target));
                    let relative = absolute
                        .strip_prefix(self.cwd)
                        .unwrap_or(&absolute)
                        .to_string_lossy()
                        .replace('\\', "/");
                    if let Some(target) =
                        resolve_file_candidate_watched(&relative, self.known_files, &mut watched)
                    {
                        result.target = Some(target);
                        return (result, watched);
                    }
                }
            }
            if let Some(base_url) = &resolution.base_url {
                let absolute = normalize_path(base_url.join(specifier));
                let relative = absolute
                    .strip_prefix(self.cwd)
                    .unwrap_or(&absolute)
                    .to_string_lossy()
                    .replace('\\', "/");
                if let Some(target) =
                    resolve_file_candidate_watched(&relative, self.known_files, &mut watched)
                {
                    result.target = Some(target);
                }
            }
        }
        (result, watched)
    }

    fn resolve_workspace(
        &self,
        project: &Project,
        specifier: &str,
        mut result: Dependency,
        watched: &mut Vec<ResolutionWatch>,
    ) -> Dependency {
        let subpath = if specifier == project.name {
            ".".to_string()
        } else {
            format!(".{}", &specifier[project.name.len()..])
        };
        let exports = project.manifest.get("exports");
        let configured = exports.and_then(|exports| {
            resolve_package_export(exports, &subpath, &self.config.export_conditions)
        });
        let mut targets = configured.into_iter().collect::<Vec<_>>();
        if !targets.is_empty() && subpath != "." {
            let subpath = subpath.trim_start_matches("./");
            targets.push(format!("./src/{subpath}"));
            targets.push(format!("./{subpath}"));
        }
        if exports.is_none() {
            if subpath == "." {
                for key in ["module", "main", "types"] {
                    if let Some(target) = project.manifest.get(key).and_then(Value::as_str) {
                        targets.push(if target.starts_with('.') {
                            target.to_string()
                        } else {
                            format!("./{target}")
                        });
                    }
                }
                targets.push("./src/index".into());
            } else {
                let subpath = subpath.trim_start_matches("./");
                targets.push(format!("./{subpath}"));
                targets.push(format!("./src/{subpath}"));
            }
        }
        if targets.is_empty() || targets.iter().any(|target| !target.starts_with('.')) {
            result.workspace = Some(project.name.clone());
            result.reason = Some(if targets.is_empty() {
                "not-exported".into()
            } else {
                "external-export-target".into()
            });
            return result;
        }
        for target in targets {
            let target = normalize_path(Path::new(&project.dir).join(target));
            let mut candidates = vec![target.to_string_lossy().replace('\\', "/")];
            if candidates[0].contains("/dist/") {
                candidates.push(candidates[0].replace("/dist/", "/src/"));
                candidates.push(candidates[0].replace("/dist/lib/", "/src/"));
            }
            for candidate in candidates {
                if let Some(target) =
                    resolve_file_candidate_watched(&candidate, self.known_files, watched)
                {
                    result.target = Some(target);
                    result.workspace = Some(project.name.clone());
                    return result;
                }
            }
        }
        result.workspace = Some(project.name.clone());
        result.reason = Some("missing-export-target".into());
        result
    }
}

fn resolve_file_candidate_watched(
    candidate: &str,
    known_files: &HashSet<String>,
    watched: &mut Vec<ResolutionWatch>,
) -> Option<String> {
    let target = resolve_file_candidate(candidate, known_files);
    watched.push(ResolutionWatch {
        candidate: candidate.to_string(),
        selected: target.clone(),
    });
    target
}

fn resolve_conditional_target(value: &Value, conditions: &[String]) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Array(values) => values
            .iter()
            .find_map(|value| resolve_conditional_target(value, conditions)),
        Value::Object(values) => {
            for condition in conditions {
                if let Some(value) = values.get(condition)
                    && let Some(target) = resolve_conditional_target(value, conditions)
                {
                    return Some(target);
                }
            }
            values
                .iter()
                .filter(|(condition, _)| !condition.starts_with('.'))
                .find_map(|(_, value)| resolve_conditional_target(value, conditions))
        }
        _ => None,
    }
}

pub fn resolve_package_export(
    exports: &Value,
    requested_subpath: &str,
    conditions: &[String],
) -> Option<String> {
    let (target, wildcard) = match exports {
        Value::Object(map) if map.keys().any(|key| key.starts_with('.')) => {
            if let Some(target) = map.get(requested_subpath) {
                (target, None)
            } else {
                let mut keys: Vec<_> = map.keys().filter(|key| key.contains('*')).collect();
                keys.sort_by_key(|key| std::cmp::Reverse(key.replace('*', "").len()));
                let (target, wildcard) = keys.into_iter().find_map(|key| {
                    let (prefix, suffix) = key.split_once('*')?;
                    (requested_subpath.starts_with(prefix) && requested_subpath.ends_with(suffix))
                        .then(|| {
                            let end = requested_subpath.len() - suffix.len();
                            (&map[key], requested_subpath[prefix.len()..end].to_string())
                        })
                })?;
                (target, Some(wildcard))
            }
        }
        _ if requested_subpath == "." => (exports, None),
        _ => return None,
    };
    let resolved = resolve_conditional_target(target, conditions)?;
    Some(match wildcard {
        Some(wildcard) => resolved.replace('*', &wildcard),
        None => resolved,
    })
}

fn normalize_path(path: impl AsRef<Path>) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.as_ref().components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn file_candidate_alternatives(candidate: &str) -> Vec<String> {
    let path = Path::new(candidate);
    let extension = path.extension().and_then(|value| value.to_str());
    let mut alternatives = vec![candidate.to_string()];
    if extension.is_none() {
        for extension in SOURCE_EXTENSIONS {
            alternatives.push(format!("{candidate}{extension}"));
        }
        for extension in SOURCE_EXTENSIONS {
            alternatives.push(format!("{candidate}/index{extension}"));
        }
    }
    if matches!(extension, Some("js" | "jsx" | "mjs" | "cjs")) {
        let stem = candidate
            .rsplit_once('.')
            .map(|item| item.0)
            .unwrap_or(candidate);
        for extension in ["ts", "tsx", "mts", "cts"] {
            alternatives.push(format!("{stem}.{extension}"));
        }
    }
    if extension == Some("ts") {
        alternatives.push(format!("{}.tsx", candidate.trim_end_matches(".ts")));
    }
    if candidate.ends_with(".d.ts") {
        alternatives.push(format!("{}.ts", candidate.trim_end_matches(".d.ts")));
    }
    alternatives
}

pub fn resolve_file_candidate(candidate: &str, known_files: &HashSet<String>) -> Option<String> {
    file_candidate_alternatives(candidate)
        .into_iter()
        .find(|alternative| known_files.contains(alternative))
}

pub fn resolution_watch_matches(watch: &ResolutionWatch, changed_file: &str) -> bool {
    for alternative in file_candidate_alternatives(&watch.candidate) {
        if alternative == changed_file {
            return true;
        }
        if watch.selected.as_ref() == Some(&alternative) {
            return false;
        }
    }
    false
}

fn match_path_pattern(pattern: &str, specifier: &str) -> Option<String> {
    let Some((prefix, suffix)) = pattern.split_once('*') else {
        return (pattern == specifier).then(String::new);
    };
    if !specifier.starts_with(prefix) || !specifier.ends_with(suffix) {
        return None;
    }
    Some(specifier[prefix.len()..specifier.len() - suffix.len()].to_string())
}

fn parse_tsconfig(path: &Path, visited: &mut HashSet<PathBuf>) -> ProjectResolution {
    if !path.is_file() || !visited.insert(path.to_path_buf()) {
        return ProjectResolution::default();
    }
    let Ok(source) = fs::read_to_string(path) else {
        return ProjectResolution::default();
    };
    let Ok(config) = json5::from_str::<Value>(&source) else {
        return ProjectResolution::default();
    };
    let directory = path.parent().unwrap_or_else(|| Path::new(""));
    let mut inherited = ProjectResolution::default();
    let extended: Vec<&str> = match config.get("extends") {
        Some(Value::String(value)) => vec![value],
        Some(Value::Array(values)) => values.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    for extended in extended {
        if !extended.starts_with('.') && !Path::new(extended).is_absolute() {
            continue;
        }
        let mut extended = normalize_path(directory.join(extended));
        if extended.extension().is_none() {
            extended.set_extension("json");
        }
        inherited = parse_tsconfig(&extended, visited);
    }
    let options = config.get("compilerOptions").and_then(Value::as_object);
    let base_url = options
        .and_then(|options| options.get("baseUrl"))
        .and_then(Value::as_str)
        .map(|value| normalize_path(directory.join(value)))
        .or(inherited.base_url);
    let paths = if let Some(paths) = options
        .and_then(|options| options.get("paths"))
        .and_then(Value::as_object)
    {
        paths
            .iter()
            .map(|(pattern, targets)| PathMapping {
                base_dir: base_url.clone().unwrap_or_else(|| directory.to_path_buf()),
                pattern: pattern.clone(),
                targets: targets
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect(),
            })
            .collect()
    } else {
        inherited.paths
    };
    ProjectResolution { base_url, paths }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_conditional_wildcard_exports() {
        let exports = serde_json::json!({
            "./feature/*": {
                "import": "./src/features/*/index.ts",
                "types": "./dist/features/*/index.d.ts"
            }
        });
        assert_eq!(
            resolve_package_export(
                &exports,
                "./feature/orders",
                &["import".into(), "types".into()]
            ),
            Some("./src/features/orders/index.ts".into())
        );
    }
}
