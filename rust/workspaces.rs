use crate::config::{Config, matches_glob};
use crate::model::{Project, Workspaces};
use serde_json::Value;
use std::fs;
use std::path::Path;
use walkdir::WalkDir;

pub fn parse_workspace_patterns(source: &str) -> Vec<String> {
    let mut patterns = vec![];
    let mut in_packages = false;
    for line in source.lines() {
        if line.trim() == "packages:" && !line.starts_with(char::is_whitespace) {
            in_packages = true;
            continue;
        }
        if in_packages && !line.is_empty() && !line.starts_with(char::is_whitespace) {
            break;
        }
        if !in_packages {
            continue;
        }
        let trimmed = line.trim();
        let Some(value) = trimmed.strip_prefix('-') else {
            continue;
        };
        let value = value
            .trim()
            .split(" #")
            .next()
            .unwrap_or_default()
            .trim()
            .trim_matches(['\'', '"']);
        if !value.is_empty() {
            patterns.push(value.to_string());
        }
    }
    patterns
}

fn workspace_matches(directory: &str, patterns: &[String]) -> bool {
    let mut included = false;
    for pattern in patterns {
        if let Some(negative) = pattern.strip_prefix('!') {
            if matches_glob(directory, negative) {
                included = false;
            }
        } else if matches_glob(directory, pattern) {
            included = true;
        }
    }
    included
}

pub fn discover_workspaces(cwd: &Path, config: &Config) -> Result<Workspaces, String> {
    let patterns = if let Some(patterns) = &config.workspace_patterns {
        patterns.clone()
    } else {
        let path = cwd.join(&config.workspace_file);
        parse_workspace_patterns(
            &fs::read_to_string(&path)
                .map_err(|error| format!("Cannot read {}: {error}", path.display()))?,
        )
    };
    let mut projects = vec![];
    for entry in WalkDir::new(cwd)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            !entry.file_type().is_dir()
                || !matches!(
                    entry.file_name().to_str(),
                    Some(".git" | "node_modules" | "dist" | "build" | "build2" | "target")
                )
        })
        .filter_map(Result::ok)
    {
        if entry.file_name() != "package.json" || !entry.file_type().is_file() {
            continue;
        }
        let Some(directory) = entry.path().parent() else {
            continue;
        };
        let Ok(relative) = directory.strip_prefix(cwd) else {
            continue;
        };
        let directory = relative.to_string_lossy().replace('\\', "/");
        if directory.is_empty() || !workspace_matches(&directory, &patterns) {
            continue;
        }
        let source = fs::read_to_string(entry.path())
            .map_err(|error| format!("Cannot read {}: {error}", entry.path().display()))?;
        let manifest: Value = json5::from_str(&source)
            .map_err(|error| format!("Invalid {}: {error}", entry.path().display()))?;
        let Some(name) = manifest
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };
        projects.push(Project {
            dir: directory.clone(),
            manifest,
            manifest_path: format!("{directory}/package.json"),
            name,
        });
    }
    projects.sort_by(|left, right| left.dir.cmp(&right.dir));
    Ok(Workspaces { projects })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_workspace_patterns() {
        assert_eq!(
            parse_workspace_patterns("packages:\n  - 'packages/*'\n  - apps/* # apps\ncatalog:\n"),
            ["packages/*", "apps/*"]
        );
    }
}
