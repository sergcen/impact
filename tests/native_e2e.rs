use monorepa_impact::graph::{REVERSE_SHARD_COUNT, read_cache_metadata, reverse_shard};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/real-monorepo");

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create fixture directory");
    for entry in fs::read_dir(source).expect("read fixture directory") {
        let entry = entry.expect("fixture entry");
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("fixture file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy fixture file");
        }
    }
}

fn write(root: &Path, file: &str, source: &str) {
    let path = root.join(file);
    fs::create_dir_all(path.parent().expect("file parent")).expect("create parent");
    fs::write(path, source).expect("write fixture");
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", root.join(".missing-test-gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_CONFIG_COUNT")
        .output()
        .expect("start git");
    assert!(
        output.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture() -> TempDir {
    let root = tempfile::tempdir().expect("temp repository");
    copy_tree(Path::new(FIXTURE), root.path());
    git(root.path(), &["init", "--quiet"]);
    git(
        root.path(),
        &["config", "user.email", "affected-test@example.com"],
    );
    git(root.path(), &["config", "user.name", "Affected Test"]);
    git(root.path(), &["add", "."]);
    git(
        root.path(),
        &[
            "commit",
            "--quiet",
            "--no-gpg-sign",
            "-m",
            "initial fixture",
        ],
    );
    root
}

struct RestrictedPath {
    path: PathBuf,
    #[cfg(unix)]
    _directory: TempDir,
}

impl RestrictedPath {
    fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(unix)]
fn git_only_path() -> RestrictedPath {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().expect("Git-only PATH");
    let git = std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
        .map(|directory| directory.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git executable");
    symlink(git, directory.path().join("git")).expect("link git");
    assert!(!directory.path().join("node").exists());
    RestrictedPath {
        path: directory.path().to_path_buf(),
        _directory: directory,
    }
}

#[cfg(windows)]
fn git_only_path() -> RestrictedPath {
    let git = std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
        .map(|directory| directory.join("git.exe"))
        .find(|candidate| candidate.is_file())
        .expect("git executable");
    let path = git.parent().expect("Git directory").to_path_buf();
    assert!(!path.join("node.exe").exists());
    RestrictedPath { path }
}

fn cli(root: &Path, args: &[&str], path: Option<&Path>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_monorepa"));
    command.args(args).current_dir(root);
    if let Some(path) = path {
        command.env("PATH", path);
    }
    command.output().expect("start native CLI")
}

fn json(output: Output) -> Value {
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON output")
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("JSON string array")
        .iter()
        .map(|value| value.as_str().expect("JSON string").to_string())
        .collect()
}

fn dependent_files(root: &Path, args: &[&str], path: &Path) -> Vec<String> {
    let mut arguments = args.to_vec();
    arguments.push("--json");
    strings(&json(cli(root, &arguments, Some(path)))["dependentFiles"])
}

fn cache_metadata(root: &Path) -> Value {
    serde_json::to_value(
        read_cache_metadata(&cache_directory(root)).expect("binary cache metadata"),
    )
    .expect("metadata value")
}

fn cache_directory(root: &Path) -> PathBuf {
    root.join("node_modules/.cache/monorepa-impact")
}

fn cache_files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(cache_directory(root))
        .expect("cache directory")
        .map(|entry| {
            let entry = entry.expect("cache entry");
            let name = entry
                .file_name()
                .into_string()
                .expect("UTF-8 cache filename");
            let bytes = fs::read(entry.path()).expect("cache file");
            (name, bytes)
        })
        .collect()
}

fn assert_generation_complete(root: &Path, metadata: &Value) {
    let directory = cache_directory(root);
    for field in ["graphFile", "unresolvedFile", "validationFile"] {
        assert!(
            directory
                .join(metadata[field].as_str().expect("generation file"))
                .is_file(),
            "missing generation file {field}"
        );
    }
    let shards = metadata["reverseShardFiles"]
        .as_array()
        .expect("reverse shard files");
    assert_eq!(shards.len(), REVERSE_SHARD_COUNT);
    for shard in shards {
        assert!(
            directory
                .join(shard.as_str().expect("reverse shard file"))
                .is_file(),
            "missing reverse shard {shard}"
        );
    }
}

#[test]
fn dependents_preserve_bindings_across_aliases_types_and_star_reexports() {
    let root = fixture();
    let empty_path = tempfile::tempdir().expect("empty PATH");
    let user = json(cli(
        root.path(),
        &[
            "--dependents",
            "packages/contracts/src/models/user.ts",
            "--specifier",
            "userVersion",
            "--strict-cache",
            "--json",
        ],
        Some(empty_path.path()),
    ));
    assert_eq!(
        strings(&user["dependentFiles"]),
        [
            "apps/storefront/src/main.ts",
            "packages/contracts/src/index.ts",
        ]
    );
    assert_eq!(user["graphStats"]["cache"], "miss");
    assert_eq!(
        user["reasons"]["packages/contracts/src/index.ts"][1]["importedSpecifier"],
        "userVersion"
    );
    assert_eq!(
        user["reasons"]["packages/contracts/src/index.ts"][1]["exportedSpecifier"],
        "currentUserVersion"
    );

    assert_eq!(
        dependent_files(
            root.path(),
            &[
                "--dependents",
                "packages/contracts/src/models/user.ts",
                "--specifier",
                "userVersion",
                "--direct",
            ],
            empty_path.path(),
        ),
        ["packages/contracts/src/index.ts"]
    );
    assert_eq!(
        dependent_files(
            root.path(),
            &[
                "--dependents",
                "packages/contracts/src/models/user.ts",
                "--specifier",
                "User",
            ],
            empty_path.path(),
        ),
        [
            "apps/storefront/src/main.ts",
            "packages/contracts/src/index.ts",
            "packages/ui/src/button.tsx",
        ]
    );
    let renamed_export = json(cli(
        root.path(),
        &[
            "--dependents",
            "packages/contracts/src/flags.ts",
            "--specifier",
            "default",
            "--json",
        ],
        Some(empty_path.path()),
    ));
    assert_eq!(
        strings(&renamed_export["dependentFiles"]),
        ["apps/admin/src/default-flag.ts"]
    );
    assert_eq!(
        renamed_export["reasons"]["apps/admin/src/default-flag.ts"][1]["via"],
        "@fixture/contracts/flagsmodified"
    );
    assert_eq!(
        dependent_files(
            root.path(),
            &[
                "--dependents",
                "packages/contracts/src/flags.ts",
                "--specifier",
                "checkoutFlag",
            ],
            empty_path.path(),
        ),
        [
            "apps/storefront/src/lazy-checkout.ts",
            "apps/storefront/src/main.ts",
            "packages/contracts/src/index.ts",
        ]
    );

    let repeated = json(cli(
        root.path(),
        &[
            "dependents",
            "packages/contracts/src/models/user.ts",
            "packages/contracts/src/models/order.ts",
            "--specifier",
            "User",
            "--specifier",
            "Order",
            "--json",
        ],
        Some(empty_path.path()),
    ));
    assert_eq!(
        strings(&repeated["targetFiles"]),
        [
            "packages/contracts/src/models/order.ts",
            "packages/contracts/src/models/user.ts",
        ]
    );
    assert_eq!(strings(&repeated["targetSpecifiers"]), ["Order", "User"]);
    assert_eq!(
        strings(&repeated["dependentFiles"]),
        [
            "apps/storefront/src/main.ts",
            "packages/contracts/src/index.ts",
            "packages/ui/src/button.tsx",
        ]
    );

    let empty = json(cli(
        root.path(),
        &[
            "--dependents",
            "packages/contracts/src/models/user.ts",
            "--specifier",
            "notExported",
            "--json",
        ],
        Some(empty_path.path()),
    ));
    assert_eq!(strings(&empty["dependentFiles"]), Vec::<String>::new());
    assert_eq!(strings(&empty["projects"]), Vec::<String>::new());
}

#[test]
fn resolver_covers_workspace_exports_tsconfig_styles_assets_and_dynamic_edges() {
    let root = fixture();
    let empty_path = tempfile::tempdir().expect("empty PATH");
    let query = |file: &str, specifier: Option<&str>| {
        let mut args = vec!["--dependents", file];
        if let Some(specifier) = specifier {
            args.extend(["--specifier", specifier]);
        }
        dependent_files(root.path(), &args, empty_path.path())
    };

    assert_eq!(
        query("packages/contracts/src/models/order.ts", Some("Order")),
        [
            "apps/storefront/src/main.ts",
            "packages/contracts/src/index.ts",
        ]
    );
    assert_eq!(
        query("packages/ui/src/icons/cart.tsx", Some("CartIcon")),
        ["apps/admin/src/main.ts"]
    );
    assert_eq!(
        query("apps/storefront/src/config.ts", Some("localConfig")),
        ["apps/storefront/src/main.ts"]
    );
    assert_eq!(
        query("apps/storefront/src/routes/home.ts", None),
        ["apps/storefront/src/main.ts"]
    );
    assert_eq!(
        query("apps/storefront/src/lazy-checkout.ts", None),
        ["apps/storefront/src/main.ts"]
    );
    assert_eq!(
        query("apps/storefront/src/legacy.cjs", None),
        ["apps/storefront/src/main.ts"]
    );
    assert_eq!(
        query("apps/storefront/src/assets/hero.svg", None),
        [
            "apps/storefront/src/main.ts",
            "apps/storefront/src/styles/main.scss",
        ]
    );
    assert_eq!(
        query("packages/ui/src/logo.svg", None),
        [
            "apps/storefront/src/main.ts",
            "apps/storefront/src/styles/main.scss",
            "packages/ui/src/button.css",
            "packages/ui/src/button.tsx",
            "packages/ui/src/tokens.css",
        ]
    );
}

#[test]
fn conservative_edges_match_named_specifiers_for_namespace_side_effect_and_runtime_loading() {
    let root = fixture();
    let empty_path = tempfile::tempdir().expect("empty PATH");
    write(
        root.path(),
        "apps/admin/src/namespace-consumer.ts",
        "import * as contracts from '@fixture/contracts/models/user';\nvoid contracts;\n",
    );
    write(
        root.path(),
        "apps/admin/src/side-effect-consumer.ts",
        "import '@fixture/contracts/models/user';\n",
    );

    assert_eq!(
        dependent_files(
            root.path(),
            &[
                "--dependents",
                "packages/contracts/src/models/user.ts",
                "--specifier",
                "userVersion",
                "--strict-cache",
            ],
            empty_path.path(),
        ),
        [
            "apps/admin/src/namespace-consumer.ts",
            "apps/admin/src/side-effect-consumer.ts",
            "apps/storefront/src/main.ts",
            "packages/contracts/src/index.ts",
        ]
    );
    assert_eq!(
        dependent_files(
            root.path(),
            &[
                "--dependents",
                "apps/storefront/src/lazy-checkout.ts",
                "--specifier",
                "loadCheckout",
            ],
            empty_path.path(),
        ),
        ["apps/storefront/src/main.ts"]
    );
    assert_eq!(
        dependent_files(
            root.path(),
            &[
                "--dependents",
                "apps/storefront/src/routes/home.ts",
                "--specifier",
                "homeRoute",
            ],
            empty_path.path(),
        ),
        ["apps/storefront/src/main.ts"]
    );
    assert_eq!(
        dependent_files(
            root.path(),
            &[
                "--dependents",
                "apps/storefront/src/legacy.cjs",
                "--specifier",
                "legacy",
            ],
            empty_path.path(),
        ),
        ["apps/storefront/src/main.ts"]
    );
}

#[test]
fn affected_is_export_selective_and_conservative_for_closed_workspace_subpaths() {
    let root = fixture();
    let native_path = git_only_path();
    write(
        root.path(),
        "packages/contracts/src/models/user.ts",
        "export interface User { id: string }\nexport const userVersion = 2;\nexport default function createUser(id: string): User { return { id }; }\n",
    );

    let affected = json(cli(
        root.path(),
        &["affected", "--base", "HEAD", "--strict-cache", "--json"],
        Some(native_path.path()),
    ));
    assert_eq!(
        affected["changedSpecifiers"]["packages/contracts/src/models/user.ts"],
        serde_json::json!(["userVersion"])
    );
    assert_eq!(
        strings(&affected["projects"]),
        [
            "@fixture/contracts",
            "@fixture/legacy",
            "@fixture/storefront",
        ]
    );
    assert_eq!(
        affected["reasons"]["@fixture/legacy"][1]["type"],
        "unresolved-workspace-fallback"
    );

    let precise = json(cli(
        root.path(),
        &[
            "--base",
            "HEAD",
            "--config",
            "affected.no-fallback.json",
            "--strict-cache",
            "--json",
        ],
        Some(native_path.path()),
    ));
    assert_eq!(
        strings(&precise["projects"]),
        ["@fixture/contracts", "@fixture/storefront"]
    );
    assert!(!strings(&precise["affectedFiles"]).contains(&"apps/legacy/src/main.ts".to_string()));
}

#[test]
fn affected_propagates_package_manifest_changes_and_deleted_exports_conservatively() {
    let manifest_root = fixture();
    let native_path = git_only_path();
    write(
        manifest_root.path(),
        "packages/contracts/package.json",
        r#"{
            "name": "@fixture/contracts",
            "exports": {
                ".": { "import": "./dist/index.js" },
                "./models/*": { "import": "./dist/models/*.js" },
                "./flagsmodified": { "import": "./src/flags.ts" },
                "./internal": { "import": "./src/internal.ts" }
            }
        }"#,
    );
    let manifest = json(cli(
        manifest_root.path(),
        &["--base", "HEAD", "--strict-cache", "--json"],
        Some(native_path.path()),
    ));
    assert_eq!(
        strings(&manifest["projects"]),
        [
            "@fixture/admin",
            "@fixture/contracts",
            "@fixture/legacy",
            "@fixture/storefront",
            "@fixture/ui",
        ]
    );
    assert_eq!(
        manifest["changedSpecifiers"]["packages/contracts/package.json"],
        serde_json::json!(["*"])
    );

    let deleted_root = fixture();
    fs::remove_file(
        deleted_root
            .path()
            .join("packages/contracts/src/models/user.ts"),
    )
    .expect("delete exported module");
    let deleted = json(cli(
        deleted_root.path(),
        &["--base", "HEAD", "--strict-cache", "--json"],
        Some(native_path.path()),
    ));
    assert_eq!(
        deleted["changedSpecifiers"]["packages/contracts/src/models/user.ts"],
        serde_json::json!(["*"])
    );
    assert_eq!(
        strings(&deleted["projects"]),
        [
            "@fixture/contracts",
            "@fixture/legacy",
            "@fixture/storefront",
            "@fixture/ui",
        ]
    );
}

#[test]
fn automatic_cache_tracks_renames_and_package_export_changes() {
    let root = fixture();
    let native_path = git_only_path();
    let initial = json(cli(
        root.path(),
        &[
            "--dependents",
            "apps/storefront/src/config.ts",
            "--specifier",
            "localConfig",
            "--strict-cache",
            "--json",
        ],
        Some(native_path.path()),
    ));
    assert_eq!(initial["graphStats"]["snapshot"], "rebuilt");
    let main_path = root.path().join("apps/storefront/src/main.ts");
    let main = fs::read_to_string(&main_path).expect("storefront main");
    fs::rename(
        root.path().join("apps/storefront/src/config.ts"),
        root.path().join("apps/storefront/src/currency.ts"),
    )
    .expect("rename local module");
    write(
        root.path(),
        "apps/storefront/src/main.ts",
        &main.replace("@app/config", "@app/currency"),
    );

    let renamed = json(cli(
        root.path(),
        &[
            "--dependents",
            "apps/storefront/src/currency.ts",
            "--specifier",
            "localConfig",
            "--json",
        ],
        Some(native_path.path()),
    ));
    assert_eq!(
        strings(&renamed["dependentFiles"]),
        ["apps/storefront/src/main.ts"]
    );
    assert_eq!(renamed["graphStats"]["snapshot"], "incremental");
    assert_eq!(renamed["graphStats"]["parsedFiles"], 2);
    let removed = cli(
        root.path(),
        &["--dependents", "apps/storefront/src/config.ts", "--json"],
        Some(native_path.path()),
    );
    assert!(!removed.status.success());
    assert!(String::from_utf8_lossy(&removed.stderr).contains("Modules not found"));

    write(
        root.path(),
        "packages/contracts/package.json",
        r#"{
            "name": "@fixture/contracts",
            "exports": {
                ".": { "import": "./dist/index.js" },
                "./models/*": { "import": "./dist/models/*.js" },
                "./flagsmodified": { "import": "./src/flags.ts" },
                "./internal": { "import": "./src/internal.ts" }
            }
        }"#,
    );
    let package_exports = json(cli(
        root.path(),
        &[
            "--dependents",
            "packages/contracts/src/internal.ts",
            "--specifier",
            "internalContract",
            "--json",
        ],
        Some(native_path.path()),
    ));
    assert_eq!(
        strings(&package_exports["dependentFiles"]),
        ["apps/legacy/src/main.ts"]
    );
    assert_eq!(package_exports["graphStats"]["snapshot"], "incremental");
    assert_eq!(package_exports["graphStats"]["parsedFiles"], 1);
}

#[test]
fn automatic_cache_reresolves_extended_json_tsconfig_inputs() {
    let root = fixture();
    let native_path = git_only_path();
    write(
        root.path(),
        "configs/typescript-base.json",
        r#"{
            "compilerOptions": {
                "baseUrl": "../apps/storefront",
                "paths": { "@base/*": ["src/*"] }
            }
        }"#,
    );
    write(
        root.path(),
        "apps/storefront/tsconfig.json",
        r#"{ "extends": "../../configs/typescript-base.json" }"#,
    );
    write(
        root.path(),
        "apps/storefront/src/base-consumer.ts",
        "import { localConfig } from '@base/config';\nvoid localConfig;\n",
    );

    let initial = json(cli(
        root.path(),
        &[
            "--dependents",
            "apps/storefront/src/config.ts",
            "--direct",
            "--strict-cache",
            "--json",
        ],
        Some(native_path.path()),
    ));
    assert_eq!(
        strings(&initial["dependentFiles"]),
        ["apps/storefront/src/base-consumer.ts"]
    );

    write(
        root.path(),
        "configs/typescript-base.json",
        r#"{
            "compilerOptions": {
                "baseUrl": "../apps/storefront",
                "paths": { "@base/*": ["src/routes/*"] }
            }
        }"#,
    );
    let refreshed = json(cli(
        root.path(),
        &[
            "--dependents",
            "apps/storefront/src/config.ts",
            "--direct",
            "--json",
        ],
        Some(native_path.path()),
    ));
    assert!(strings(&refreshed["dependentFiles"]).is_empty());
    assert_eq!(refreshed["graphStats"]["snapshot"], "incremental");
    assert_eq!(refreshed["graphStats"]["parsedFiles"], 1);
}

#[test]
fn automatic_cache_reresolves_only_importers_watching_added_or_deleted_candidates() {
    let root = fixture();
    let native_path = git_only_path();
    write(
        root.path(),
        "apps/storefront/src/priority/index.ts",
        "export const selected = 'index';\n",
    );
    write(
        root.path(),
        "apps/storefront/src/priority-consumer.ts",
        "import { selected } from './priority';\nvoid selected;\n",
    );

    let initial = json(cli(
        root.path(),
        &[
            "--dependents",
            "apps/storefront/src/priority/index.ts",
            "--direct",
            "--strict-cache",
            "--json",
        ],
        Some(native_path.path()),
    ));
    assert_eq!(
        strings(&initial["dependentFiles"]),
        ["apps/storefront/src/priority-consumer.ts"]
    );

    write(
        root.path(),
        "apps/storefront/src/priority.ts",
        "export const selected = 'file';\n",
    );
    let added = json(cli(
        root.path(),
        &[
            "--dependents",
            "apps/storefront/src/priority.ts",
            "--direct",
            "--json",
        ],
        Some(native_path.path()),
    ));
    assert_eq!(
        strings(&added["dependentFiles"]),
        ["apps/storefront/src/priority-consumer.ts"]
    );
    assert_eq!(added["graphStats"]["snapshot"], "incremental");
    assert_eq!(added["graphStats"]["parsedFiles"], 1);

    fs::remove_file(root.path().join("apps/storefront/src/priority.ts"))
        .expect("remove higher-priority candidate");
    let deleted = json(cli(
        root.path(),
        &[
            "--dependents",
            "apps/storefront/src/priority/index.ts",
            "--direct",
            "--json",
        ],
        Some(native_path.path()),
    ));
    assert_eq!(
        strings(&deleted["dependentFiles"]),
        ["apps/storefront/src/priority-consumer.ts"]
    );
    assert_eq!(deleted["graphStats"]["snapshot"], "incremental");
    assert_eq!(deleted["graphStats"]["parsedFiles"], 0);
}

#[test]
fn root_inputs_select_named_dependent_and_all_projects_and_expand_commands() {
    let root = fixture();
    let native_path = git_only_path();

    write(
        root.path(),
        "docs/architecture.md",
        "# Updated fixture architecture\n",
    );
    let named = json(cli(
        root.path(),
        &["--base", "HEAD", "--strict-cache", "--json"],
        Some(native_path.path()),
    ));
    assert_eq!(strings(&named["projects"]), ["@fixture/storefront"]);
    git(root.path(), &["restore", "docs/architecture.md"]);

    write(
        root.path(),
        "tsconfig.base.json",
        r#"{ "compilerOptions": { "strict": true, "noUncheckedIndexedAccess": true } }"#,
    );
    let dependents = json(cli(
        root.path(),
        &["--base", "HEAD", "--strict-cache", "--json"],
        Some(native_path.path()),
    ));
    assert_eq!(strings(&dependents["projects"]), ["@fixture/storefront"]);
    git(root.path(), &["restore", "tsconfig.base.json"]);

    write(
        root.path(),
        "tooling/eslint/index.js",
        "export const rules = { semi: 'warn' };\n",
    );
    let all = json(cli(
        root.path(),
        &["--base", "HEAD", "--strict-cache", "--json"],
        Some(native_path.path()),
    ));
    let expected = [
        "@fixture/admin",
        "@fixture/contracts",
        "@fixture/data",
        "@fixture/eslint-config",
        "@fixture/legacy",
        "@fixture/shared",
        "@fixture/storefront",
        "@fixture/ui",
    ];
    assert_eq!(strings(&all["projects"]), expected);

    let command = cli(
        root.path(),
        &[
            "affected",
            "--base",
            "HEAD",
            "--strict-cache",
            "--",
            "echo __child__ {workspaces}",
        ],
        Some(native_path.path()),
    );
    assert!(command.status.success());
    let stdout = String::from_utf8_lossy(&command.stdout);
    let expected_filters = expected
        .iter()
        .map(|project| format!("--filter={project}"))
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        stdout.lines().last(),
        Some(format!("__child__ {expected_filters}").as_str())
    );
    assert!(!stdout.contains("@fixture/ignored"));

    let failed = cli(
        root.path(),
        &[
            "affected",
            "--base",
            "HEAD",
            "--strict-cache",
            "--",
            "exit 37",
        ],
        Some(native_path.path()),
    );
    assert_eq!(failed.status.code(), Some(37));
}

#[test]
fn cache_handles_cold_warm_shard_only_automatic_trusted_and_head_changes() {
    let root = fixture();
    let empty_path = tempfile::tempdir().expect("empty PATH");
    let native_path = git_only_path();
    let args = [
        "--dependents",
        "packages/contracts/src/models/user.ts",
        "--specifier",
        "userVersion",
        "--json",
    ];

    let cold = json(cli(
        root.path(),
        &[
            "--dependents",
            "packages/contracts/src/models/user.ts",
            "--specifier",
            "userVersion",
            "--strict-cache",
            "--json",
        ],
        Some(native_path.path()),
    ));
    assert_eq!(cold["graphStats"]["cache"], "miss");
    assert_eq!(cold["graphStats"]["validation"], "strict");
    for (file, bytes) in cache_files(root.path()) {
        assert!(file.ends_with(".bin"), "non-binary cache artifact: {file}");
        assert_eq!(
            bytes.get(..4),
            Some(b"FODC".as_slice()),
            "missing binary cache header: {file}"
        );
    }

    let warm = json(cli(root.path(), &args, Some(empty_path.path())));
    assert_eq!(warm["graphStats"]["cache"], "hit");
    assert_eq!(warm["graphStats"]["validation"], "automatic");
    let warm_shards = cache_metadata(root.path())["reverseShardFiles"]
        .as_array()
        .expect("warm reverse shard files")
        .clone();

    write(
        root.path(),
        "apps/admin/src/main.ts",
        "import { userVersion } from '@fixture/contracts/models/user';\nvoid userVersion;\n",
    );
    write(
        root.path(),
        "apps/admin/src/new-consumer.ts",
        "import { userVersion } from '@fixture/contracts/models/user';\nvoid userVersion;\n",
    );
    let automatic = json(cli(root.path(), &args, Some(native_path.path())));
    assert_eq!(automatic["graphStats"]["cache"], "hit");
    assert_eq!(automatic["graphStats"]["snapshot"], "incremental");
    assert_eq!(automatic["graphStats"]["parsedFiles"], 2);
    let automatic_files = strings(&automatic["dependentFiles"]);
    assert_eq!(
        automatic_files,
        [
            "apps/admin/src/main.ts",
            "apps/admin/src/new-consumer.ts",
            "apps/storefront/src/main.ts",
            "packages/contracts/src/index.ts",
        ]
    );
    let automatic_generation = cache_metadata(root.path())["graphFile"]
        .as_str()
        .expect("automatic graph file")
        .to_string();
    let automatic_shards = cache_metadata(root.path())["reverseShardFiles"]
        .as_array()
        .expect("automatic reverse shard files")
        .clone();
    let reused_shards = warm_shards
        .iter()
        .zip(&automatic_shards)
        .filter(|(warm, automatic)| warm == automatic)
        .count();
    assert!(reused_shards > 0);
    assert!(reused_shards < REVERSE_SHARD_COUNT);

    write(
        root.path(),
        "apps/admin/src/main.ts",
        "export const adminWithoutUserDependency = true;\n",
    );
    let changed_again = json(cli(root.path(), &args, Some(native_path.path())));
    assert_eq!(changed_again["graphStats"]["cache"], "hit");
    assert_eq!(changed_again["graphStats"]["snapshot"], "incremental");
    assert_eq!(changed_again["graphStats"]["parsedFiles"], 1);
    assert_eq!(
        strings(&changed_again["dependentFiles"]),
        [
            "apps/admin/src/new-consumer.ts",
            "apps/storefront/src/main.ts",
            "packages/contracts/src/index.ts",
        ]
    );
    let changed_again_generation = cache_metadata(root.path())["graphFile"]
        .as_str()
        .expect("changed-again graph file")
        .to_string();
    assert_ne!(changed_again_generation, automatic_generation);

    write(
        root.path(),
        "apps/admin/src/new-consumers/second-consumer.ts",
        "import { userVersion } from '@fixture/contracts/models/user';\nvoid userVersion;\n",
    );
    let trusted = json(cli(
        root.path(),
        &[
            "--dependents",
            "packages/contracts/src/models/user.ts",
            "--specifier",
            "userVersion",
            "--trust-cache",
            "--json",
        ],
        Some(empty_path.path()),
    ));
    assert_eq!(trusted["graphStats"]["cache"], "hit");
    assert_eq!(trusted["graphStats"]["validation"], "trusted");
    assert!(
        !strings(&trusted["dependentFiles"])
            .contains(&"apps/admin/src/new-consumers/second-consumer.ts".to_string())
    );

    let refreshed = json(cli(root.path(), &args, Some(native_path.path())));
    assert_eq!(refreshed["graphStats"]["cache"], "hit");
    assert_eq!(refreshed["graphStats"]["snapshot"], "incremental");
    assert_eq!(refreshed["graphStats"]["parsedFiles"], 1);
    assert_eq!(
        strings(&refreshed["dependentFiles"]),
        [
            "apps/admin/src/new-consumer.ts",
            "apps/admin/src/new-consumers/second-consumer.ts",
            "apps/storefront/src/main.ts",
            "packages/contracts/src/index.ts",
        ]
    );
    let refreshed_generation = cache_metadata(root.path())["graphFile"]
        .as_str()
        .expect("refreshed graph file")
        .to_string();
    assert_ne!(refreshed_generation, changed_again_generation);

    write(
        root.path(),
        "apps/admin/src/new-consumers/second-consumer.ts",
        "export const secondConsumerWithoutUserDependency = true;\n",
    );
    let untracked_changed_again = json(cli(root.path(), &args, Some(native_path.path())));
    assert_eq!(untracked_changed_again["graphStats"]["cache"], "hit");
    assert_eq!(
        untracked_changed_again["graphStats"]["snapshot"],
        "incremental"
    );
    assert_eq!(untracked_changed_again["graphStats"]["parsedFiles"], 1);
    assert_eq!(
        strings(&untracked_changed_again["dependentFiles"]),
        [
            "apps/admin/src/new-consumer.ts",
            "apps/storefront/src/main.ts",
            "packages/contracts/src/index.ts",
        ]
    );
    let untracked_changed_generation = cache_metadata(root.path())["graphFile"]
        .as_str()
        .expect("untracked-changed graph file")
        .to_string();
    assert_ne!(untracked_changed_generation, refreshed_generation);

    git(root.path(), &["add", "."]);
    git(root.path(), &["commit", "--quiet", "-m", "add consumers"]);
    let new_head = json(cli(root.path(), &args, Some(native_path.path())));
    assert_eq!(new_head["graphStats"]["cache"], "hit");
    assert_eq!(new_head["graphStats"]["snapshot"], "incremental");
    assert_eq!(new_head["graphStats"]["parsedFiles"], 3);
    assert_eq!(
        new_head["dependentFiles"],
        untracked_changed_again["dependentFiles"]
    );
    let head_generation = cache_metadata(root.path())["graphFile"]
        .as_str()
        .expect("HEAD graph file")
        .to_string();
    assert_ne!(head_generation, untracked_changed_generation);

    let trusted_alias = json(cli(
        root.path(),
        &[
            "--dependents",
            "packages/contracts/src/models/user.ts",
            "--specifier",
            "userVersion",
            "--trust-cache",
            "--json",
        ],
        Some(empty_path.path()),
    ));
    assert_eq!(trusted_alias["graphStats"]["cache"], "hit");
    assert_eq!(trusted_alias["graphStats"]["validation"], "trusted");
    assert_eq!(trusted_alias["dependentFiles"], new_head["dependentFiles"]);

    let metadata = cache_metadata(root.path());
    fs::remove_file(
        root.path()
            .join("node_modules/.cache/monorepa-impact")
            .join(metadata["graphFile"].as_str().expect("graph file")),
    )
    .expect("remove full graph");
    let shard_only = json(cli(root.path(), &args, Some(native_path.path())));
    assert_eq!(shard_only["dependentFiles"], new_head["dependentFiles"]);
    assert_eq!(shard_only["graphStats"]["cache"], "hit");

    let cache_before_no_cache = cache_files(root.path());
    let uncached = json(cli(
        root.path(),
        &[
            "--dependents",
            "packages/contracts/src/models/user.ts",
            "--specifier",
            "userVersion",
            "--no-cache",
            "--json",
        ],
        Some(native_path.path()),
    ));
    assert_eq!(uncached["graphStats"]["cache"], "miss");
    assert_eq!(uncached["graphStats"]["snapshot"], "disabled");
    assert_eq!(uncached["dependentFiles"], new_head["dependentFiles"]);
    assert_eq!(cache_files(root.path()), cache_before_no_cache);

    let rebuilt = json(cli(
        root.path(),
        &[
            "--dependents",
            "packages/contracts/src/models/user.ts",
            "--specifier",
            "userVersion",
            "--rebuild-cache",
            "--json",
        ],
        Some(empty_path.path()),
    ));
    assert_eq!(rebuilt["graphStats"]["cache"], "miss");
    assert_eq!(rebuilt["dependentFiles"], new_head["dependentFiles"]);
    assert_ne!(
        cache_metadata(root.path())["graphFile"],
        Value::String(head_generation)
    );
}

#[test]
fn cache_recovers_incomplete_generations_and_invalidates_root_inputs_and_config() {
    let root = fixture();
    let native_path = git_only_path();
    let args = [
        "--dependents",
        "packages/contracts/src/models/user.ts",
        "--specifier",
        "userVersion",
        "--json",
    ];
    let initial = json(cli(
        root.path(),
        &[
            "--dependents",
            "packages/contracts/src/models/user.ts",
            "--specifier",
            "userVersion",
            "--strict-cache",
            "--json",
        ],
        Some(native_path.path()),
    ));
    let initial_metadata = cache_metadata(root.path());
    assert_generation_complete(root.path(), &initial_metadata);
    let initial_graph = initial_metadata["graphFile"]
        .as_str()
        .expect("initial graph")
        .to_string();
    let target_shard = reverse_shard("packages/contracts/src/models/user.ts");
    let target_shard_file = initial_metadata["reverseShardFiles"][target_shard]
        .as_str()
        .expect("target shard file");
    fs::remove_file(cache_directory(root.path()).join(target_shard_file))
        .expect("remove target shard");

    let recovered = json(cli(root.path(), &args, Some(native_path.path())));
    assert_eq!(recovered["graphStats"]["cache"], "miss");
    assert_eq!(recovered["dependentFiles"], initial["dependentFiles"]);
    let recovered_metadata = cache_metadata(root.path());
    assert_ne!(recovered_metadata["graphFile"], initial_graph);
    assert_generation_complete(root.path(), &recovered_metadata);
    assert!(
        !cache_directory(root.path())
            .join(target_shard_file)
            .exists()
    );

    write(
        root.path(),
        "tooling/eslint/index.js",
        "export const rules = { semi: 'warn', quotes: 'error' };\n",
    );
    let invalidated_root = json(cli(root.path(), &args, Some(native_path.path())));
    assert_eq!(invalidated_root["graphStats"]["cache"], "hit");
    assert_eq!(invalidated_root["graphStats"]["snapshot"], "incremental");
    assert_eq!(invalidated_root["graphStats"]["parsedFiles"], 1);
    assert_eq!(
        invalidated_root["dependentFiles"],
        initial["dependentFiles"]
    );
    let root_generation = cache_metadata(root.path())["graphFile"]
        .as_str()
        .expect("root-input graph")
        .to_string();

    let config_path = root.path().join("affected.config.jsonc");
    let config = fs::read_to_string(&config_path).expect("affected config");
    write(
        root.path(),
        "affected.config.jsonc",
        &format!("{config}\n// force config-file-state invalidation\n"),
    );
    let invalidated_config = json(cli(root.path(), &args, Some(native_path.path())));
    assert_eq!(invalidated_config["graphStats"]["cache"], "miss");
    assert_eq!(
        invalidated_config["dependentFiles"],
        initial["dependentFiles"]
    );
    let config_metadata = cache_metadata(root.path());
    assert_ne!(config_metadata["graphFile"], root_generation);
    assert_generation_complete(root.path(), &config_metadata);
}

#[test]
fn cli_validates_modes_and_supports_human_explanations() {
    let root = fixture();
    let invalid: &[(&[&str], &str)] = &[
        (
            &["--dependents", "file.ts", "--base", "HEAD"],
            "--base is only valid with affected",
        ),
        (
            &["--dependents", "file.ts", "--", "echo unreachable"],
            "A child command is only valid with affected",
        ),
        (&["--direct"], "--direct is only valid with dependents"),
        (
            &["--specifier", "value"],
            "--specifier is only valid with dependents",
        ),
        (&["dependents"], "dependents requires at least one <file>"),
        (
            &["dependents", "file.ts", "--base", "HEAD"],
            "--base is only valid with affected",
        ),
        (
            &["affected", "--direct"],
            "--direct is only valid with dependents",
        ),
        (
            &["affected", "--specifier", "value"],
            "--specifier is only valid with dependents",
        ),
        (
            &["affected", "echo unreachable"],
            "child commands must follow --",
        ),
        (
            &["--trust-cache", "--strict-cache"],
            "--trust-cache cannot be combined with --strict-cache",
        ),
        (
            &["--trust-cache", "--no-cache"],
            "--trust-cache cannot be combined with --no-cache",
        ),
        (&["--base"], "--base requires a value"),
        (&["--config"], "--config requires a value"),
        (&["--dependents"], "--dependents requires a value"),
        (&["--specifier"], "--specifier requires a value"),
        (
            &["affected", "--dependents", "file.ts"],
            "--dependents is a legacy option",
        ),
        (
            &["unknown"],
            "Unknown command: unknown; expected 'affected' or 'dependents'",
        ),
        (&["--dead-code"], "Unknown option: --dead-code"),
        (&["--dead-code=files"], "Unknown option: --dead-code=files"),
        (&["--unknown"], "Unknown option: --unknown"),
    ];
    for (args, expected) in invalid {
        let output = cli(root.path(), args, None);
        assert!(!output.status.success(), "unexpected success for {args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "missing {expected:?} for {args:?}"
        );
    }

    let help = cli(root.path(), &["--help"], None);
    assert!(help.status.success());
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.starts_with("Usage: monorepa"));
    for command in ["affected", "dependents"] {
        assert!(help.contains(command));
    }
    assert!(!help.contains("--dependents"));
    assert!(!help.contains("--dead-code"));

    let affected_help = cli(root.path(), &["affected", "--help"], None);
    assert!(affected_help.status.success());
    let affected_help = String::from_utf8_lossy(&affected_help.stdout);
    assert!(affected_help.starts_with("Usage: monorepa affected"));
    assert!(affected_help.contains("--base"));
    assert!(!affected_help.contains("--specifier"));

    let dependents_help = cli(root.path(), &["dependents", "--help"], None);
    assert!(dependents_help.status.success());
    let dependents_help = String::from_utf8_lossy(&dependents_help.stdout);
    assert!(dependents_help.starts_with("Usage: monorepa dependents"));
    assert!(dependents_help.contains("--specifier"));
    assert!(dependents_help.contains("--direct"));
    assert!(!dependents_help.contains("--base"));

    let version = cli(root.path(), &["--version"], None);
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        format!("monorepa {}", env!("CARGO_PKG_VERSION"))
    );

    let empty_path = tempfile::tempdir().expect("empty PATH");
    let missing_git = cli(
        root.path(),
        &["affected", "--base", "HEAD", "--json"],
        Some(empty_path.path()),
    );
    assert!(!missing_git.status.success());
    assert!(
        String::from_utf8_lossy(&missing_git.stderr).contains("Failed to start git"),
        "unexpected missing-Git error: {}",
        String::from_utf8_lossy(&missing_git.stderr)
    );

    let explain = cli(
        root.path(),
        &[
            "dependents",
            "packages/contracts/src/models/user.ts",
            "--specifier",
            "userVersion",
            "--strict-cache",
            "--explain",
        ],
        Some(empty_path.path()),
    );
    assert!(explain.status.success());
    let explain = String::from_utf8_lossy(&explain.stdout);
    assert!(explain.contains("packages/contracts/src/index.ts"));
    assert!(explain.contains("[userVersion -> currentUserVersion]"));

    let missing = cli(
        root.path(),
        &["dependents", "packages/contracts/src/missing.ts", "--json"],
        Some(empty_path.path()),
    );
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr)
            .contains("Modules not found or excluded from the graph")
    );

    let outside = tempfile::tempdir().expect("outside repository");
    let outside_file = outside.path().join("outside.ts");
    write(
        outside.path(),
        "outside.ts",
        "export const outside = true;\n",
    );
    let outside = cli(
        root.path(),
        &[
            "dependents",
            outside_file.to_str().expect("outside UTF-8 path"),
            "--json",
        ],
        Some(empty_path.path()),
    );
    assert!(!outside.status.success());
    assert!(
        String::from_utf8_lossy(&outside.stderr).contains("Module must be inside the repository")
    );
}
