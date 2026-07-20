use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub struct Binding {
    #[serde(rename = "excludeDefault", default, skip_serializing_if = "is_false")]
    pub exclude_default: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exported: Option<String>,
    pub imported: String,
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Dependency {
    #[serde(default)]
    pub bindings: Vec<Binding>,
    #[serde(
        rename = "globPattern",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub glob_pattern: Option<String>,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub specifier: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unresolved: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Record {
    pub dependencies: Vec<Dependency>,
    #[serde(default)]
    pub exports: Vec<String>,
    #[serde(rename = "mtimeMs")]
    pub mtime_ms: f64,
    #[serde(rename = "rawDependencies", default)]
    pub raw_dependencies: Vec<Dependency>,
    #[serde(rename = "resolutionWatches", default)]
    pub resolution_watches: Vec<ResolutionWatch>,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResolutionWatch {
    pub candidate: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Importer {
    #[serde(default)]
    pub bindings: Vec<Binding>,
    pub importer: String,
    pub kind: String,
    pub specifier: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UnresolvedImporter {
    pub importer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub reason: String,
    pub specifier: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Project {
    pub dir: String,
    pub manifest: Value,
    pub manifest_path: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Workspaces {
    pub projects: Vec<Project>,
}

impl Workspaces {
    pub fn project_for_file(&self, file: &str) -> Option<&Project> {
        self.projects
            .iter()
            .filter(|project| file == project.dir || file.starts_with(&format!("{}/", project.dir)))
            .max_by_key(|project| project.dir.len())
    }

    pub fn project_by_name(&self, name: &str) -> Option<&Project> {
        self.projects.iter().find(|project| project.name == name)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GraphStats {
    pub cache: String,
    #[serde(rename = "parsedFiles")]
    pub parsed_files: usize,
    #[serde(rename = "reusedFiles")]
    pub reused_files: usize,
    pub snapshot: String,
    pub validation: String,
}

impl GraphStats {
    pub fn cached(file_count: usize, validation: &str) -> Self {
        Self {
            cache: "hit".into(),
            parsed_files: 0,
            reused_files: file_count,
            snapshot: "loaded".into(),
            validation: validation.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Graph {
    pub exports_by_file: HashMap<String, Vec<String>>,
    pub files: Vec<String>,
    pub reverse: HashMap<String, Vec<Importer>>,
    pub stats: GraphStats,
    pub unresolved_by_workspace: HashMap<String, Vec<UnresolvedImporter>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Reason {
    #[serde(rename = "exportedSpecifier", skip_serializing_if = "Option::is_none")]
    pub exported_specifier: Option<String>,
    pub file: String,
    #[serde(rename = "importedSpecifier", skip_serializing_if = "Option::is_none")]
    pub imported_specifier: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Predecessor {
    pub exported_specifier: Option<String>,
    pub from: String,
    pub imported_specifier: String,
    pub kind: String,
    pub specifier: String,
}
