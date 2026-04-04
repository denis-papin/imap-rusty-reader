use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use jsonschema::validator_for;
use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::agents::AgentsSpec;
use crate::client::{
    OpenAiClient, ResponseContentItem, ResponseInputItem, ResponseTextConfig, ResponsesRequest,
    json_schema_format,
};
use crate::config::{Account, Config};

const REQUEST_JSON_MAX_CHARS: usize = 16_000;
const REQUEST_XML_MAX_CHARS: usize = 24_000;
const ATTACHMENT_TEXT_MAX_CHARS: usize = 12_000;

#[derive(Debug, Clone)]
pub struct EnrichmentRunner {
    pub config: Config,
    pub agents_path: PathBuf,
    pub force: bool,
    pub limit: Option<usize>,
    pub account_filter: Option<String>,
    pub dry_run: bool,
}

#[derive(Debug, Clone)]
pub struct EmailJob {
    account_name: String,
    folder: PathBuf,
    stem: String,
    json_path: PathBuf,
    xml_path: PathBuf,
    attachments: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
struct AttachmentPromptInput {
    file_name: String,
    mime_type: String,
    size: u64,
    ingestion_mode: String,
    extracted_text: Option<String>,
    note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct MetaFile {
    input_hash: String,
    model: String,
    agents_file: String,
    prompt_cache_key: String,
    response_id: Option<String>,
    usage: Option<Value>,
    completed_at: String,
}

#[derive(Debug, Clone, Serialize)]
struct ErrorFile {
    error: String,
    failed_at: String,
}

impl EnrichmentRunner {
    pub async fn run(&self, agents: &AgentsSpec) -> Result<()> {
        if !self.config.ai_enabled {
            log::warn!("💣 AI enrichment disabled in config (`aiEnabled: false`), nothing to do");
            return Ok(());
        }

        let api_key = std::env::var("OPENAI_API_KEY")
            .with_context(|| "OPENAI_API_KEY is required for ai-enrich")?;
        let client = OpenAiClient::new(
            api_key,
            self.config.ai_base_url.as_deref(),
            self.config.ai_timeout_seconds,
            self.config.ai_request_delay_ms,
        )?;
        let validator = validator_for(&agents.output_schema)
            .with_context(|| "unable to compile JSON schema from AGENTS.md")?;

        let jobs = self.discover_jobs()?;
        if jobs.is_empty() {
            log::info!("😎 No parsed emails found for AI enrichment");
            return Ok(());
        }

        let mut processed = 0usize;
        for job in jobs {
            if let Some(limit) = self.limit {
                if processed >= limit {
                    break;
                }
            }
            processed += 1;
            if let Err(error) = self.process_job(&client, &validator, agents, &job).await {
                log::warn!(
                    "💣 AI enrichment failed [{}]: {error:#}",
                    job.folder.display()
                );
                self.write_error_file(&job, &error)?;
            }
        }

        Ok(())
    }

    fn discover_jobs(&self) -> Result<Vec<EmailJob>> {
        let mut jobs = Vec::new();
        for account in &self.config.accounts {
            if self
                .account_filter
                .as_deref()
                .is_some_and(|filter| filter != account.name)
            {
                continue;
            }

            let parse_folder = Path::new(&self.config.email_folder)
                .join(&account.name)
                .join(&self.config.parse_ia_folder);
            if !parse_folder.exists() {
                continue;
            }

            jobs.extend(discover_account_jobs(
                &parse_folder,
                account,
                &self.config.ai_output_suffix,
            )?);
        }

        jobs.sort_by(|left, right| left.folder.cmp(&right.folder));
        Ok(jobs)
    }

    async fn process_job(
        &self,
        client: &OpenAiClient,
        validator: &jsonschema::Validator,
        agents: &AgentsSpec,
        job: &EmailJob,
    ) -> Result<()> {
        let input_hash = compute_input_hash(job, &self.agents_path)?;
        if !self.force && is_job_up_to_date(job, &self.config.ai_output_suffix, &input_hash)? {
            log::info!("😎 Skip AI already enriched [{}]", job.folder.display());
            return Ok(());
        }

        log::info!("🤖 Enrich email [{}]", job.folder.display());

        let json_payload = fs::read_to_string(&job.json_path)
            .with_context(|| format!("unable to read {}", job.json_path.display()))?;
        let xml_payload = fs::read_to_string(&job.xml_path)
            .with_context(|| format!("unable to read {}", job.xml_path.display()))?;
        let attachment_inputs = self.build_attachment_inputs(client, job).await?;

        let request = build_responses_request(
            &self.config.ai_model,
            &self.config.ai_prompt_cache_prefix,
            agents,
            &job.account_name,
            &job.stem,
            &json_payload,
            &xml_payload,
            &attachment_inputs,
        );

        if self.dry_run {
            let preview_path = job.folder.join(format!("{}.ai.request.json", job.stem));
            fs::write(&preview_path, serde_json::to_string_pretty(&request)?)
                .with_context(|| format!("unable to write {}", preview_path.display()))?;
            log::info!("😎 Dry run request written [{}]", preview_path.display());
            return Ok(());
        }

        let response = client
            .create_response_with_retry(&request, self.config.ai_retry_count)
            .await?;
        validator
            .validate(&response.output_json)
            .map_err(|error| anyhow!("response JSON schema validation failed: {error}"))?;
        self.assert_classification_allowed(&response.output_json, &agents.folder_taxonomy)?;

        let output_path = job
            .folder
            .join(format!("{}{}", job.stem, self.config.ai_output_suffix));
        fs::write(
            &output_path,
            serde_json::to_string_pretty(&response.output_json)?,
        )
        .with_context(|| format!("unable to write {}", output_path.display()))?;

        let meta = MetaFile {
            input_hash,
            model: self.config.ai_model.clone(),
            agents_file: self.agents_path.display().to_string(),
            prompt_cache_key: build_prompt_cache_key(
                &self.config.ai_prompt_cache_prefix,
                &self.config.ai_model,
                agents,
            ),
            response_id: response.id,
            usage: response.usage,
            completed_at: Utc::now().to_rfc3339(),
        };
        let meta_path = job.folder.join(format!("{}.ai.meta.json", job.stem));
        fs::write(&meta_path, serde_json::to_string_pretty(&meta)?)
            .with_context(|| format!("unable to write {}", meta_path.display()))?;

        let error_path = job.folder.join(format!("{}.ai.error.json", job.stem));
        if error_path.exists() {
            fs::remove_file(&error_path)
                .with_context(|| format!("unable to remove {}", error_path.display()))?;
        }

        Ok(())
    }

    async fn build_attachment_inputs(
        &self,
        client: &OpenAiClient,
        job: &EmailJob,
    ) -> Result<Vec<AttachmentSource>> {
        let mut items = Vec::new();
        for path in job
            .attachments
            .iter()
            .take(self.config.ai_max_attachments_per_email)
        {
            let metadata = fs::metadata(path)
                .with_context(|| format!("unable to read metadata {}", path.display()))?;
            let file_name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("attachment.bin")
                .to_string();
            let mime_type = guess_mime_type(path);
            let size = metadata.len();

            if size > self.config.ai_max_attachment_bytes as u64 {
                items.push(AttachmentSource::PromptOnly(AttachmentPromptInput {
                    file_name,
                    mime_type,
                    size,
                    ingestion_mode: "skipped".to_string(),
                    extracted_text: None,
                    note: Some(
                        "Attachment skipped because it exceeded aiMaxAttachmentBytes".to_string(),
                    ),
                }));
                continue;
            }

            if is_text_like(path) {
                let text = read_attachment_text(path)?;
                items.push(AttachmentSource::PromptOnly(AttachmentPromptInput {
                    file_name,
                    mime_type,
                    size,
                    ingestion_mode: "inline_text".to_string(),
                    extracted_text: Some(truncate_chars(&text, ATTACHMENT_TEXT_MAX_CHARS)),
                    note: None,
                }));
                continue;
            }

            let is_pdf = path
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| value.eq_ignore_ascii_case("pdf"))
                .unwrap_or(false);
            let is_image = mime_type.starts_with("image/");

            if (is_pdf && self.config.ai_send_raw_pdf)
                || (is_image && self.config.ai_send_raw_images)
            {
                let uploaded = client.upload_file(path).await?;
                items.push(AttachmentSource::PromptAndFile {
                    prompt: AttachmentPromptInput {
                        file_name,
                        mime_type,
                        size,
                        ingestion_mode: "raw_file".to_string(),
                        extracted_text: None,
                        note: Some(
                            "Attachment uploaded as raw file for model inspection".to_string(),
                        ),
                    },
                    file_id: uploaded.file_id,
                });
                continue;
            }

            items.push(AttachmentSource::PromptOnly(AttachmentPromptInput {
                file_name,
                mime_type,
                size,
                ingestion_mode: "metadata_only".to_string(),
                extracted_text: None,
                note: Some("Attachment not sent as raw file; summarize only if supported by available metadata".to_string()),
            }));
        }

        Ok(items)
    }

    fn assert_classification_allowed(
        &self,
        value: &Value,
        folder_taxonomy: &BTreeMap<String, Vec<String>>,
    ) -> Result<()> {
        let main_folder = value
            .get("main_folder")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("model response missing `main_folder`"))?;
        let sub_folder = value
            .get("sub_folder")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("model response missing `sub_folder`"))?;

        let allowed_subfolders = folder_taxonomy
            .get(main_folder)
            .ok_or_else(|| anyhow!("model returned unsupported main_folder `{main_folder}`"))?;

        if !allowed_subfolders.iter().any(|value| value == sub_folder) {
            bail!(
                "model returned unsupported sub_folder `{sub_folder}` for main_folder `{main_folder}`"
            );
        }

        Ok(())
    }

    fn write_error_file(&self, job: &EmailJob, error: &anyhow::Error) -> Result<()> {
        let payload = ErrorFile {
            error: format!("{error:#}"),
            failed_at: Utc::now().to_rfc3339(),
        };
        let path = job.folder.join(format!("{}.ai.error.json", job.stem));
        fs::write(&path, serde_json::to_string_pretty(&payload)?)
            .with_context(|| format!("unable to write {}", path.display()))?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
enum AttachmentSource {
    PromptOnly(AttachmentPromptInput),
    PromptAndFile {
        prompt: AttachmentPromptInput,
        file_id: String,
    },
}

fn discover_account_jobs(
    parse_folder: &Path,
    account: &Account,
    ai_output_suffix: &str,
) -> Result<Vec<EmailJob>> {
    let mut jobs = Vec::new();
    for entry in WalkDir::new(parse_folder).min_depth(1).max_depth(8) {
        let entry = entry?;
        if !entry.file_type().is_dir() {
            continue;
        }

        let folder = entry.path().to_path_buf();
        let mut json_by_stem = BTreeMap::new();
        let mut xml_by_stem = BTreeMap::new();
        let mut attachments = Vec::new();

        for child in
            fs::read_dir(&folder).with_context(|| format!("unable to read {}", folder.display()))?
        {
            let child = child?;
            let path = child.path();
            if !path.is_file() {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if name.ends_with(".ai.meta.json")
                || name.ends_with(".ai.error.json")
                || name.ends_with(".ai.request.json")
                || name.ends_with(ai_output_suffix)
            {
                continue;
            }
            if name.ends_with(".json") {
                if let Some(stem) = path.file_stem().and_then(|value| value.to_str()) {
                    json_by_stem.insert(stem.to_string(), path.clone());
                }
                continue;
            }
            if name.ends_with(".xml") {
                if let Some(stem) = path.file_stem().and_then(|value| value.to_str()) {
                    xml_by_stem.insert(stem.to_string(), path.clone());
                }
                continue;
            }
            attachments.push(path);
        }

        for (stem, json_path) in json_by_stem {
            let Some(xml_path) = xml_by_stem.get(&stem).cloned() else {
                continue;
            };
            jobs.push(EmailJob {
                account_name: account.name.clone(),
                folder: folder.clone(),
                stem,
                json_path,
                xml_path,
                attachments: attachments.clone(),
            });
        }
    }

    Ok(jobs)
}

fn build_responses_request(
    model: &str,
    prompt_cache_prefix: &str,
    agents: &AgentsSpec,
    account_name: &str,
    email_stem: &str,
    json_payload: &str,
    xml_payload: &str,
    attachments: &[AttachmentSource],
) -> ResponsesRequest {
    let mut content = Vec::new();
    content.push(ResponseContentItem::InputText {
        text: build_primary_prompt(
            agents,
            account_name,
            email_stem,
            json_payload,
            xml_payload,
            attachments,
        ),
    });

    for attachment in attachments {
        if let AttachmentSource::PromptAndFile { file_id, .. } = attachment {
            content.push(ResponseContentItem::InputFile {
                file_id: file_id.clone(),
            });
        }
    }

    ResponsesRequest {
        model: model.to_string(),
        instructions: build_instructions(agents),
        input: vec![ResponseInputItem {
            role: "user".to_string(),
            content,
        }],
        store: false,
        prompt_cache_key: Some(build_prompt_cache_key(prompt_cache_prefix, model, agents)),
        text: ResponseTextConfig {
            format: json_schema_format("email_enrichment", agents.output_schema.clone()),
        },
    }
}

fn build_instructions(agents: &AgentsSpec) -> String {
    format!(
        "You are enriching an email dossier for indexing.\n\
Return only JSON matching the provided JSON Schema.\n\
Choose exactly one `main_folder` and one `sub_folder` from the allowed taxonomy.\n\
The `sub_folder` must be valid for the selected `main_folder`.\n\
If evidence is insufficient, say so in the appropriate field and do not invent facts.\n\
Write all free-text JSON values in French.\n\
Specifically, `email_summary` and each attachment `summary` must be written in French.\n\
Keep `main_folder` and `sub_folder` exactly as provided in the taxonomy, without translation.\n\
Follow the business instructions below.\n\n{}",
        agents.instructions
    )
}

fn build_primary_prompt(
    agents: &AgentsSpec,
    account_name: &str,
    email_stem: &str,
    json_payload: &str,
    xml_payload: &str,
    attachments: &[AttachmentSource],
) -> String {
    let attachment_payload = attachments
        .iter()
        .map(|attachment| match attachment {
            AttachmentSource::PromptOnly(prompt) => json!(prompt),
            AttachmentSource::PromptAndFile { prompt, .. } => json!(prompt),
        })
        .collect::<Vec<_>>();
    let metadata_json = compact_json_for_prompt(json_payload);
    let email_body = extract_xml_content_for_prompt(xml_payload);

    format!(
        "Email account: {account_name}\n\
Email folder name: {email_stem}\n\
Allowed folder taxonomy:\n```json\n{}\n```\n\n\
Metadata JSON:\n```json\n{}\n```\n\n\
Email body extracted from XML:\n```text\n{}\n```\n\n\
Attachment inputs:\n```json\n{}\n```\n",
        serde_json::to_string_pretty(&agents.folder_taxonomy).unwrap_or_else(|_| "{}".to_string()),
        truncate_chars(&metadata_json, REQUEST_JSON_MAX_CHARS),
        truncate_chars(&email_body, REQUEST_XML_MAX_CHARS),
        serde_json::to_string_pretty(&attachment_payload).unwrap_or_else(|_| "[]".to_string()),
    )
}

fn compact_json_for_prompt(value: &str) -> String {
    serde_json::from_str::<Value>(value)
        .ok()
        .and_then(|json| serde_json::to_string(&json).ok())
        .unwrap_or_else(|| value.to_string())
}

fn build_prompt_cache_key(prompt_cache_prefix: &str, model: &str, agents: &AgentsSpec) -> String {
    format!(
        "{}:{}:{}",
        prompt_cache_prefix.trim(),
        model.trim(),
        &agents.cache_fingerprint[..16]
    )
}

fn extract_xml_content_for_prompt(xml_payload: &str) -> String {
    let content_regex =
        Regex::new(r#"(?s)<content(?P<attrs>[^>]*)><!\[CDATA\[(?P<body>.*?)\]\]></content>"#)
            .expect("invalid regex");
    let format_regex = Regex::new(r#"format="([^"]+)""#).expect("invalid regex");
    let source_regex = Regex::new(r#"source-format="([^"]+)""#).expect("invalid regex");

    let mut parts = Vec::new();
    for (index, captures) in content_regex.captures_iter(xml_payload).enumerate() {
        let attrs = captures
            .name("attrs")
            .map(|value| value.as_str())
            .unwrap_or("");
        let format = format_regex
            .captures(attrs)
            .and_then(|captures| captures.get(1))
            .map(|value| value.as_str())
            .unwrap_or("unknown");
        let source = source_regex
            .captures(attrs)
            .and_then(|captures| captures.get(1))
            .map(|value| value.as_str());
        let body = captures
            .name("body")
            .map(|value| value.as_str().trim())
            .unwrap_or("");
        if body.is_empty() {
            continue;
        }

        let label = match source {
            Some(source) => format!("Part {} [{} from {}]", index + 1, format, source),
            None => format!("Part {} [{}]", index + 1, format),
        };
        parts.push(format!(
            "{label}\n{}",
            truncate_chars(body, REQUEST_XML_MAX_CHARS / 2)
        ));
    }

    if parts.is_empty() {
        truncate_chars(xml_payload, REQUEST_XML_MAX_CHARS)
    } else {
        parts.join("\n\n")
    }
}

fn guess_mime_type(path: &Path) -> String {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "txt" => "text/plain",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "json" => "application/json",
        "xml" => "application/xml",
        "html" | "htm" => "text/html",
        "pdf" => "application/pdf",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn is_text_like(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "txt" | "md" | "csv" | "json" | "xml" | "html" | "htm"
            )
        })
        .unwrap_or(false)
}

fn read_attachment_text(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("unable to read {}", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let truncated = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        format!("{truncated}\n...[truncated]")
    } else {
        truncated
    }
}

fn compute_input_hash(job: &EmailJob, agents_path: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    hash_file(&mut hasher, &job.json_path)?;
    hash_file(&mut hasher, &job.xml_path)?;
    hash_file(&mut hasher, agents_path)?;
    for attachment in &job.attachments {
        hash_file(&mut hasher, attachment)?;
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn hash_file(hasher: &mut Sha256, path: &Path) -> Result<()> {
    let mut file =
        fs::File::open(path).with_context(|| format!("unable to open {}", path.display()))?;
    let mut buffer = [0u8; 8192];
    loop {
        let count = file
            .read(&mut buffer)
            .with_context(|| format!("unable to read {}", path.display()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(())
}

fn is_job_up_to_date(job: &EmailJob, ai_output_suffix: &str, input_hash: &str) -> Result<bool> {
    let output_path = job.folder.join(format!("{}{}", job.stem, ai_output_suffix));
    let meta_path = job.folder.join(format!("{}.ai.meta.json", job.stem));
    if !output_path.exists() || !meta_path.exists() {
        return Ok(false);
    }

    let meta_payload = fs::read_to_string(&meta_path)
        .with_context(|| format!("unable to read {}", meta_path.display()))?;
    let meta: Value = serde_json::from_str(&meta_payload)
        .with_context(|| format!("unable to parse {}", meta_path.display()))?;
    Ok(meta.get("input_hash").and_then(Value::as_str) == Some(input_hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_like_detection_matches_expected_extensions() {
        assert!(is_text_like(Path::new("a.txt")));
        assert!(is_text_like(Path::new("a.json")));
        assert!(!is_text_like(Path::new("a.pdf")));
    }

    #[test]
    fn truncate_adds_marker_when_needed() {
        let text = truncate_chars("abcdef", 3);
        assert!(text.contains("[truncated]"));
    }

    #[test]
    fn extracts_body_text_from_xml_content_nodes() {
        let xml = r#"<email-content><content format="plain"><![CDATA[Bonjour]]></content></email-content>"#;
        let body = extract_xml_content_for_prompt(xml);
        assert!(body.contains("Bonjour"));
        assert!(body.contains("Part 1"));
    }
}
