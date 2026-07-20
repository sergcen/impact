use globset::Glob;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_CACHE_DIRECTORY: &str = "node_modules/.cache/monorepa-impact";
pub const CONFIG_NAMES: [&str; 2] = ["affected.config.json", "affected.config.jsonc"];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct CacheConfig {
    pub directory: String,
    pub enabled: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            directory: DEFAULT_CACHE_DIRECTORY.into(),
            enabled: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ProjectSelection {
    Keyword(String),
    Names(Vec<String>),
}

impl Default for ProjectSelection {
    fn default() -> Self {
        Self::Keyword("dependents".into())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RootInput {
    #[serde(rename = "invalidateGraph")]
    pub invalidate_graph: bool,
    pub patterns: Vec<String>,
    pub projects: ProjectSelection,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub base: String,
    pub cache: CacheConfig,
    #[serde(rename = "exportConditions")]
    pub export_conditions: Vec<String>,
    pub extensions: Vec<String>,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    #[serde(rename = "packageFallback")]
    pub package_fallback: String,
    #[serde(rename = "rootInputs")]
    pub root_inputs: Vec<RootInput>,
    #[serde(rename = "workspaceFile")]
    pub workspace_file: String,
    #[serde(rename = "workspacePatterns")]
    pub workspace_patterns: Option<Vec<String>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            base: "origin/master".into(),
            cache: CacheConfig::default(),
            export_conditions: vec![
                "import".into(),
                "browser".into(),
                "default".into(),
                "types".into(),
                "require".into(),
            ],
            extensions: [
                ".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs", ".json", ".css",
                ".scss", ".less", ".svg", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".ico",
                ".woff", ".woff2",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            include: vec!["**/*".into()],
            exclude: [
                "**/node_modules/**",
                "**/dist/**",
                "**/build/**",
                "**/build2/**",
                "**/coverage/**",
                "**/storybook-static/**",
                "**/test-results/**",
                "**/playwright-report/**",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            package_fallback: "unresolved".into(),
            root_inputs: vec![],
            workspace_file: "pnpm-workspace.yaml".into(),
            workspace_patterns: None,
        }
    }
}

pub struct LoadedConfig {
    pub config: Config,
    pub path: Option<PathBuf>,
}

pub fn load_config(cwd: &Path, requested: Option<&str>) -> Result<LoadedConfig, String> {
    let path = if let Some(requested) = requested {
        let requested = Path::new(requested);
        Some(if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            cwd.join(requested)
        })
    } else {
        CONFIG_NAMES
            .iter()
            .map(|name| cwd.join(name))
            .find(|candidate| candidate.is_file())
    };

    let Some(path) = path else {
        return Ok(LoadedConfig {
            config: Config::default(),
            path: None,
        });
    };
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
    let config: Config = json5::from_str(&source)
        .map_err(|error| format!("Invalid affected config {}: {error}", path.display()))?;

    if config.package_fallback != "unresolved" && config.package_fallback != "none" {
        return Err("packageFallback must be either \"unresolved\" or \"none\"".into());
    }
    for input in &config.root_inputs {
        if input.patterns.is_empty() {
            return Err("rootInputs.patterns must not be empty".into());
        }
        if let ProjectSelection::Keyword(keyword) = &input.projects
            && keyword != "all"
            && keyword != "dependents"
        {
            return Err(format!(
                "rootInputs.projects must be all, dependents, or an array; got {keyword}"
            ));
        }
    }

    Ok(LoadedConfig {
        config,
        path: Some(path),
    })
}

pub fn matches_glob(value: &str, pattern: &str) -> bool {
    Glob::new(pattern)
        .map(|glob| glob.compile_matcher().is_match(value))
        .unwrap_or(false)
}

pub fn matches_any_glob(value: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pattern| matches_glob(value, pattern))
}

pub fn config_signature(config: &Config) -> String {
    let encoded = serde_json::to_string(config).unwrap_or_default();
    let mut hash = 2_166_136_261_u32;
    for byte in encoded.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    format!("{}:{hash:08x}", encoded.len())
}
