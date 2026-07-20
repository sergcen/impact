use monorepa_impact::graph::read_cache_metadata;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

fn write(root: &Path, file: &str, source: &str) {
    let path = root.join(file);
    fs::create_dir_all(path.parent().expect("file parent")).expect("create parent");
    fs::write(path, source).expect("write fixture");
}

fn process_latency(binary: &str, root: &Path, path: &Path, args: &[&str]) -> (f64, f64) {
    let mut samples = Vec::with_capacity(60);
    for _ in 0..60 {
        let started = Instant::now();
        let output = Command::new(binary)
            .args(args)
            .current_dir(root)
            .env("PATH", path)
            .output()
            .expect("warm query");
        assert!(output.status.success(), "warm query failed");
        samples.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    samples.sort_by(f64::total_cmp);
    (
        samples[samples.len() / 2],
        samples[(samples.len() * 95 / 100).min(samples.len() - 1)],
    )
}

#[test]
#[ignore = "run explicitly with the bench:cache package script"]
fn trusted_and_automatic_dependents_cache_process_p95_stay_below_5ms() {
    let root = tempfile::tempdir().expect("fixture");
    let empty_path = tempfile::tempdir().expect("empty PATH");
    write(
        root.path(),
        "workspace.yaml",
        "packages:\n  - 'packages/*'\n",
    );
    write(
        root.path(),
        "affected.config.json",
        r#"{ "workspaceFile": "workspace.yaml" }"#,
    );
    write(
        root.path(),
        "packages/library/package.json",
        r#"{
            "name": "@fixture/library",
            "exports": { ".": "./src/feature.ts" }
        }"#,
    );
    write(
        root.path(),
        "packages/library/src/feature.ts",
        "export const feature = true;\n",
    );
    write(
        root.path(),
        "packages/library/src/consumer.ts",
        "import { feature } from './feature';\nvoid feature;\n",
    );
    let binary = env!("CARGO_BIN_EXE_monorepa");
    let initial = Command::new(binary)
        .args([
            "dependents",
            "packages/library/src/feature.ts",
            "--strict-cache",
            "--json",
        ])
        .current_dir(root.path())
        .env("PATH", empty_path.path())
        .output()
        .expect("cold query");
    assert!(initial.status.success(), "cold query failed");

    let cache_directory = root.path().join("node_modules/.cache/monorepa-impact");
    let metadata = read_cache_metadata(&cache_directory).expect("binary cache metadata");
    fs::remove_file(cache_directory.join(metadata.graph_file))
        .expect("remove full graph to enforce the shard-only path");

    let probe = Command::new(binary)
        .args([
            "dependents",
            "packages/library/src/feature.ts",
            "--trust-cache",
            "--json",
        ])
        .current_dir(root.path())
        .env("PATH", empty_path.path())
        .output()
        .expect("trusted cache probe");
    assert!(probe.status.success(), "trusted cache probe failed");
    let probe: serde_json::Value = serde_json::from_slice(&probe.stdout).expect("probe JSON");
    assert_eq!(probe["graphStats"]["cache"], "hit");
    assert_eq!(probe["graphStats"]["validation"], "trusted");

    let automatic_probe = Command::new(binary)
        .args(["dependents", "packages/library/src/feature.ts", "--json"])
        .current_dir(root.path())
        .env("PATH", empty_path.path())
        .output()
        .expect("automatic cache probe without Git on PATH");
    assert!(automatic_probe.status.success(), "automatic probe failed");
    let automatic_probe: serde_json::Value =
        serde_json::from_slice(&automatic_probe.stdout).expect("automatic probe JSON");
    assert_eq!(automatic_probe["graphStats"]["cache"], "hit");
    assert_eq!(automatic_probe["graphStats"]["validation"], "automatic");

    let (dependent_p50, dependent_p95) = process_latency(
        binary,
        root.path(),
        empty_path.path(),
        &[
            "dependents",
            "packages/library/src/feature.ts",
            "--trust-cache",
            "--json",
        ],
    );
    println!("warm process latency: dependents p50={dependent_p50:.2}ms p95={dependent_p95:.2}ms");
    assert!(
        dependent_p95 <= 5.0,
        "dependents cache p95 exceeded 5ms: {dependent_p95:.2}ms"
    );

    let (automatic_p50, automatic_p95) = process_latency(
        binary,
        root.path(),
        empty_path.path(),
        &["dependents", "packages/library/src/feature.ts", "--json"],
    );
    println!(
        "automatic process latency: dependents p50={automatic_p50:.2}ms p95={automatic_p95:.2}ms"
    );
    assert!(
        automatic_p95 <= 5.0,
        "automatic dependents cache p95 exceeded 5ms: {automatic_p95:.2}ms"
    );
}
