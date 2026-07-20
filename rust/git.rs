use crate::extract::changed_export_specifiers;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::Command;

pub fn git_output(cwd: &Path, args: &[&str], optional: bool) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| format!("Failed to start git: {error}"))?;
    if !output.status.success() {
        if optional {
            return Err(String::new());
        }
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn git_files(cwd: &Path, args: &[&str], optional: bool) -> Result<Vec<String>, String> {
    Ok(git_output(cwd, args, optional)?
        .split('\0')
        .filter(|file| !file.is_empty())
        .map(|file| file.replace('\\', "/"))
        .collect())
}

pub fn collect_changed_files(cwd: &Path, base: &str) -> Result<Vec<String>, String> {
    let mut files = BTreeSet::new();
    if !base.is_empty() && base != "HEAD" {
        files.extend(git_files(
            cwd,
            &[
                "diff",
                "--name-only",
                "-z",
                "--no-renames",
                &format!("{base}...HEAD"),
            ],
            false,
        )?);
    }
    for args in [
        &["diff", "--name-only", "-z", "--no-renames", "HEAD"][..],
        &["ls-files", "--others", "--exclude-standard", "-z"][..],
    ] {
        match git_files(cwd, args, true) {
            Ok(changed) => files.extend(changed),
            Err(error) if error.is_empty() => {}
            Err(error) => return Err(error),
        }
    }
    Ok(files.into_iter().collect())
}

fn comparison_ref(cwd: &Path, base: &str) -> String {
    if base.is_empty() || base == "HEAD" {
        return "HEAD".into();
    }
    git_output(cwd, &["merge-base", base, "HEAD"], true)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| base.to_string())
}

fn read_at_ref(cwd: &Path, reference: &str, file: &str) -> Option<String> {
    git_output(cwd, &["show", &format!("{reference}:{file}")], true).ok()
}

pub fn collect_changed_specifiers(
    cwd: &Path,
    base: &str,
    files: &[String],
) -> BTreeMap<String, Vec<String>> {
    let reference = comparison_ref(cwd, base);
    files
        .iter()
        .map(|file| {
            let current = fs::read_to_string(cwd.join(file)).ok();
            let previous = read_at_ref(cwd, &reference, file);
            (
                file.clone(),
                changed_export_specifiers(file, previous.as_deref(), current.as_deref()),
            )
        })
        .collect()
}
