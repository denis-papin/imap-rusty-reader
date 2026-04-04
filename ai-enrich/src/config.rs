use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

const DEFAULT_DEFINITION_PATH: &str = "./config.yml";
const DEFAULT_MODEL: &str = "gpt-4o-mini";
const DEFAULT_OUTPUT_SUFFIX: &str = ".ai.json";
const DEFAULT_MAX_ATTACHMENT_BYTES: usize = 2_000_000;
const DEFAULT_MAX_ATTACHMENTS_PER_EMAIL: usize = 12;
const DEFAULT_RETRY_COUNT: usize = 2;
const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
const DEFAULT_REQUEST_DELAY_MS: u64 = 0;
const DEFAULT_PROMPT_CACHE_PREFIX: &str = "ai-enrich";

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(rename = "emailFolder")]
    pub email_folder: String,
    #[serde(rename = "parseIaFolder", default = "default_parse_ia_folder")]
    pub parse_ia_folder: String,
    #[serde(rename = "aiEnabled", default = "default_ai_enabled")]
    pub ai_enabled: bool,
    #[serde(rename = "aiModel", default = "default_ai_model")]
    pub ai_model: String,
    #[serde(rename = "aiAgentsFile")]
    pub ai_agents_file: Option<String>,
    #[serde(rename = "aiOutputSuffix", default = "default_ai_output_suffix")]
    pub ai_output_suffix: String,
    #[serde(
        rename = "aiMaxAttachmentBytes",
        default = "default_ai_max_attachment_bytes"
    )]
    pub ai_max_attachment_bytes: usize,
    #[serde(
        rename = "aiMaxAttachmentsPerEmail",
        default = "default_ai_max_attachments_per_email"
    )]
    pub ai_max_attachments_per_email: usize,
    #[allow(dead_code)]
    #[serde(rename = "aiSendRawPdf", default = "default_ai_send_raw_pdf")]
    pub ai_send_raw_pdf: bool,
    #[serde(rename = "aiSendRawImages", default = "default_ai_send_raw_images")]
    pub ai_send_raw_images: bool,
    #[serde(rename = "aiRetryCount", default = "default_ai_retry_count")]
    pub ai_retry_count: usize,
    #[serde(rename = "aiTimeoutSeconds", default = "default_ai_timeout_seconds")]
    pub ai_timeout_seconds: u64,
    #[serde(rename = "aiRequestDelayMs", default = "default_ai_request_delay_ms")]
    pub ai_request_delay_ms: u64,
    #[serde(
        rename = "aiPromptCachePrefix",
        default = "default_ai_prompt_cache_prefix"
    )]
    pub ai_prompt_cache_prefix: String,
    #[serde(rename = "aiBaseUrl")]
    pub ai_base_url: Option<String>,
    pub accounts: Vec<Account>,
}

#[allow(dead_code)]
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

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub config: Config,
    pub agents_path: PathBuf,
}

fn default_ssl_enabled() -> bool {
    true
}

fn default_parse_ia_folder() -> String {
    "parse-ai".to_string()
}

fn default_ai_enabled() -> bool {
    false
}

fn default_ai_model() -> String {
    DEFAULT_MODEL.to_string()
}

fn default_ai_output_suffix() -> String {
    DEFAULT_OUTPUT_SUFFIX.to_string()
}

fn default_ai_max_attachment_bytes() -> usize {
    DEFAULT_MAX_ATTACHMENT_BYTES
}

fn default_ai_max_attachments_per_email() -> usize {
    DEFAULT_MAX_ATTACHMENTS_PER_EMAIL
}

fn default_ai_send_raw_pdf() -> bool {
    true
}

fn default_ai_send_raw_images() -> bool {
    false
}

fn default_ai_retry_count() -> usize {
    DEFAULT_RETRY_COUNT
}

fn default_ai_timeout_seconds() -> u64 {
    DEFAULT_TIMEOUT_SECONDS
}

fn default_ai_request_delay_ms() -> u64 {
    DEFAULT_REQUEST_DELAY_MS
}

fn default_ai_prompt_cache_prefix() -> String {
    DEFAULT_PROMPT_CACHE_PREFIX.to_string()
}

pub fn load_runtime_config(
    path: Option<&str>,
    agents_override: Option<&str>,
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

    let agents_path = resolve_agents_path(&config_path, &config, agents_override)?;
    Ok(RuntimeConfig {
        config,
        agents_path,
    })
}

fn resolve_agents_path(
    config_path: &Path,
    config: &Config,
    agents_override: Option<&str>,
) -> Result<PathBuf> {
    let candidate = agents_override
        .map(PathBuf::from)
        .or_else(|| config.ai_agents_file.as_ref().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("AGENTS.md"));

    if candidate.is_absolute() {
        return Ok(candidate);
    }

    let base = config_path.parent().unwrap_or_else(|| Path::new("."));
    Ok(base.join(candidate))
}
