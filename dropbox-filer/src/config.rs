use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;

const DEFAULT_DEFINITION_PATH: &str = "./config.yml";
const DEFAULT_AI_OUTPUT_SUFFIX: &str = ".ai.json";
const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
const DEFAULT_PARSE_IA_FOLDER: &str = "parse-ai";

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(rename = "emailFolder")]
    pub email_folder: String,
    #[serde(rename = "parseIaFolder", default = "default_parse_ia_folder")]
    pub parse_ia_folder: String,
    #[serde(rename = "aiOutputSuffix", default = "default_ai_output_suffix")]
    pub ai_output_suffix: String,
    #[serde(rename = "dropboxRootFolder")]
    pub dropbox_root_folder: Option<String>,
    #[serde(rename = "dropboxAccessToken")]
    pub dropbox_access_token: Option<String>,
    #[serde(rename = "dropboxAppKey")]
    pub dropbox_app_key: Option<String>,
    #[serde(rename = "dropboxAppSecret")]
    pub dropbox_app_secret: Option<String>,
    #[serde(rename = "dropboxRefreshToken")]
    pub dropbox_refresh_token: Option<String>,
    #[serde(
        rename = "dropboxTimeoutSeconds",
        default = "default_dropbox_timeout_seconds"
    )]
    pub dropbox_timeout_seconds: u64,
    #[serde(rename = "dropboxOauthTokenUrl")]
    pub dropbox_oauth_token_url: Option<String>,
    #[serde(rename = "dropboxApiBaseUrl")]
    pub dropbox_api_base_url: Option<String>,
    #[serde(rename = "dropboxContentBaseUrl")]
    pub dropbox_content_base_url: Option<String>,
    pub accounts: Vec<Account>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Account {
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub config: Config,
    pub dropbox_root_folder: String,
}

fn default_parse_ia_folder() -> String {
    DEFAULT_PARSE_IA_FOLDER.to_string()
}

fn default_ai_output_suffix() -> String {
    DEFAULT_AI_OUTPUT_SUFFIX.to_string()
}

fn default_dropbox_timeout_seconds() -> u64 {
    DEFAULT_TIMEOUT_SECONDS
}

pub fn load_runtime_config(
    path: Option<&str>,
    root_override: Option<&str>,
) -> Result<RuntimeConfig> {
    let path = path
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_DEFINITION_PATH);
    println!("We are loading the configuration from {}", path);

    let config_path = PathBuf::from(path);
    let content = fs::read_to_string(&config_path)
        .with_context(|| format!("unable to read config file {}", config_path.display()))?;
    let config = serde_yaml::from_str::<Config>(&content)
        .with_context(|| format!("unable to parse YAML config {}", config_path.display()))?;

    let dropbox_root_folder = resolve_dropbox_root(&config, root_override)?;
    Ok(RuntimeConfig {
        config,
        dropbox_root_folder,
    })
}

fn resolve_dropbox_root(config: &Config, root_override: Option<&str>) -> Result<String> {
    root_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| config.dropbox_root_folder.clone())
        .ok_or_else(|| {
            anyhow!(
                "dropbox root folder is required: set `dropboxRootFolder` in config.yml or pass `--root`"
            )
        })
}

#[allow(dead_code)]
fn _config_base_dir(path: &Path) -> &Path {
    path.parent().unwrap_or_else(|| Path::new("."))
}
