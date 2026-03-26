use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

const DEFAULT_DEFINITION_PATH: &str = "./config.yml";

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(rename = "emailFolder")]
    pub email_folder: String,
    pub accounts: Vec<Account>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Account {
    pub name: String,
    pub group: bool,
    pub recover: bool,
    pub login: String,
    pub password: String,
    pub server: String,
    pub port: u16,
    #[serde(rename = "sslEnabled", default = "default_ssl_enabled")]
    pub ssl_enabled: bool,
    #[serde(rename = "imapOutFolder")]
    pub imap_out_folder: Vec<String>,
    #[serde(rename = "imapInFolder")]
    pub imap_in_folder: Vec<String>,
}

/// Provides the Java-compatible default for `sslEnabled` when the YAML omits it.
fn default_ssl_enabled() -> bool {
    true
}

/// Loads the YAML configuration from the explicit CLI path or the default\n/// `./config.yml`, preserving the same external behavior as the Java program.
pub fn load_config(path: Option<&str>) -> Result<Config> {
    let path = path
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_DEFINITION_PATH);
    println!("We are loading the configuration from {}", path);

    let content = fs::read_to_string(Path::new(path))
        .with_context(|| format!("unable to read config file {}", path))?;
    let config = serde_yaml::from_str::<Config>(&content)
        .with_context(|| format!("unable to parse YAML config {}", path))?;
    Ok(config)
}
