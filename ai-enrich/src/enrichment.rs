use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use jsonschema::validator_for;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::agents::AgentsSpec;
use crate::client::{
    OpenAiClient, ResponseContentItem, ResponseEnvelope, ResponseInputItem, ResponseTextConfig,
    ResponsesRequest, json_schema_format,
};
use crate::config::{Account, Config};

const REQUEST_JSON_MAX_CHARS: usize = 16_000;
const REQUEST_XML_MAX_CHARS: usize = 24_000;
const ATTACHMENT_TEXT_MAX_CHARS: usize = 12_000;
const AI_OUTPUT_LAYOUT_VERSION: &str = "2";

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
    attachment_path: Option<PathBuf>,
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

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ParsedEmailMetadata {
    #[serde(default)]
    attachments: Vec<ParsedAttachmentMetadata>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ParsedAttachmentMetadata {
    original_name: String,
    #[serde(default)]
    size: u64,
    mime_type: String,
    #[serde(default)]
    extracted_text: Option<String>,
    #[serde(default)]
    extracted_text_method: Option<String>,
    #[serde(default)]
    extracted_text_truncated: Option<bool>,
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

#[derive(Debug, Clone)]
struct ValidationFailure {
    message: String,
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
        let parsed_metadata = serde_json::from_str::<ParsedEmailMetadata>(&json_payload)
            .with_context(|| format!("unable to parse {}", job.json_path.display()))?;
        let attachment_inputs = self
            .build_attachment_inputs(client, job, &parsed_metadata)
            .await?;

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

        let response = self
            .create_validated_response(
                client,
                validator,
                agents,
                job,
                &json_payload,
                &xml_payload,
                &attachment_inputs,
                &request,
            )
            .await?;

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
        let meta_path = meta_sidecar_path(&job.folder, &job.stem);
        fs::write(&meta_path, serde_json::to_string_pretty(&meta)?)
            .with_context(|| format!("unable to write {}", meta_path.display()))?;

        let error_path = error_sidecar_path(&job.folder, &job.stem);
        if error_path.exists() {
            fs::remove_file(&error_path)
                .with_context(|| format!("unable to remove {}", error_path.display()))?;
        }

        Ok(())
    }

    async fn create_validated_response(
        &self,
        client: &OpenAiClient,
        validator: &jsonschema::Validator,
        agents: &AgentsSpec,
        job: &EmailJob,
        json_payload: &str,
        xml_payload: &str,
        attachment_inputs: &[AttachmentSource],
        initial_request: &ResponsesRequest,
    ) -> Result<ResponseEnvelope> {
        let mut response = client
            .create_response_with_retry(initial_request, self.config.ai_retry_count)
            .await?;

        for repair_attempt in 0..=self.config.ai_retry_count {
            match self.validate_model_output(
                validator,
                &response.output_json,
                &agents.folder_taxonomy,
            ) {
                Ok(()) => return Ok(response),
                Err(error) if repair_attempt < self.config.ai_retry_count => {
                    let repair_number = repair_attempt + 1;
                    log::warn!(
                        "💣 AI output invalid, repair {repair_number}/{} [{}]: {}",
                        self.config.ai_retry_count,
                        job.folder.display(),
                        error.message
                    );
                    let repair_request = build_repair_request(
                        &self.config.ai_model,
                        &self.config.ai_prompt_cache_prefix,
                        agents,
                        &job.account_name,
                        &job.stem,
                        json_payload,
                        xml_payload,
                        attachment_inputs,
                        &response.output_json,
                        &error.message,
                    );
                    response = client
                        .create_response_with_retry(&repair_request, self.config.ai_retry_count)
                        .await?;
                }
                Err(error) => return Err(anyhow!(error.message)),
            }
        }

        Err(anyhow!("AI response validation loop ended unexpectedly"))
    }

    async fn build_attachment_inputs(
        &self,
        client: &OpenAiClient,
        job: &EmailJob,
        parsed_metadata: &ParsedEmailMetadata,
    ) -> Result<Vec<AttachmentSource>> {
        if self.config.ai_max_attachments_per_email == 0 {
            return Err(anyhow!(
                "`aiMaxAttachmentsPerEmail` must be >= 1 in per-attachment mode"
            ));
        }

        let attachment = parsed_metadata
            .attachments
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("parse metadata contains no attachment"))?;

        let file_name = job
            .attachment_path
            .as_deref()
            .and_then(|path| path.file_name())
            .and_then(|value| value.to_str())
            .map(|value| value.to_string())
            .unwrap_or_else(|| attachment.original_name.clone());

        if let Some(extracted_text) = attachment.extracted_text.as_ref() {
            let note = attachment.extracted_text_method.as_deref().map(|method| {
                match attachment.extracted_text_truncated.unwrap_or(false) {
                    true => format!(
                        "Attachment text extracted via {} and truncated before AI enrichment",
                        method
                    ),
                    false => format!("Attachment text extracted via {}", method),
                }
            });
            return Ok(vec![AttachmentSource::PromptOnly(AttachmentPromptInput {
                file_name,
                mime_type: attachment.mime_type.clone(),
                size: attachment.size,
                ingestion_mode: "pre_extracted_text".to_string(),
                extracted_text: Some(truncate_chars(extracted_text, ATTACHMENT_TEXT_MAX_CHARS)),
                note,
            })]);
        }

        let Some(path) = job.attachment_path.as_deref() else {
            return Ok(vec![AttachmentSource::PromptOnly(AttachmentPromptInput {
                file_name,
                mime_type: attachment.mime_type.clone(),
                size: attachment.size,
                ingestion_mode: "metadata_only".to_string(),
                extracted_text: None,
                note: Some(
                    "Attachment file missing from parse-ia output; summarize only from available metadata"
                        .to_string(),
                ),
            })]);
        };

        let metadata = fs::metadata(path)
            .with_context(|| format!("unable to read metadata {}", path.display()))?;
        let mime_type = guess_mime_type(path);
        let size = metadata.len();

        if size > self.config.ai_max_attachment_bytes as u64 {
            return Ok(vec![AttachmentSource::PromptOnly(AttachmentPromptInput {
                file_name,
                mime_type,
                size,
                ingestion_mode: "skipped".to_string(),
                extracted_text: None,
                note: Some("Attachment skipped because it exceeded aiMaxAttachmentBytes".to_string()),
            })]);
        }

        if is_text_like(path) {
            let text = read_attachment_text(path)?;
            return Ok(vec![AttachmentSource::PromptOnly(AttachmentPromptInput {
                file_name,
                mime_type,
                size,
                ingestion_mode: "inline_text".to_string(),
                extracted_text: Some(truncate_chars(&text, ATTACHMENT_TEXT_MAX_CHARS)),
                note: None,
            })]);
        }

        let is_pdf = path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.eq_ignore_ascii_case("pdf"))
            .unwrap_or(false);
        let is_image = mime_type.starts_with("image/");

        if is_image && self.config.ai_send_raw_images {
            let uploaded = client.upload_file(path).await?;
            return Ok(vec![AttachmentSource::PromptAndFile {
                prompt: AttachmentPromptInput {
                    file_name,
                    mime_type,
                    size,
                    ingestion_mode: "raw_file".to_string(),
                    extracted_text: None,
                    note: Some("Attachment uploaded as raw file for model inspection".to_string()),
                },
                file_id: uploaded.file_id,
            }]);
        }

        if is_pdf {
            return Ok(vec![AttachmentSource::PromptOnly(AttachmentPromptInput {
                file_name,
                mime_type,
                size,
                ingestion_mode: "metadata_only".to_string(),
                extracted_text: None,
                note: Some(
                    "PDF not uploaded as raw file; AI enrichment relies on parse-ia extracted text when available"
                        .to_string(),
                ),
            })]);
        }

        Ok(vec![AttachmentSource::PromptOnly(AttachmentPromptInput {
            file_name,
            mime_type,
            size,
            ingestion_mode: "metadata_only".to_string(),
            extracted_text: None,
            note: Some(
                "Attachment not sent as raw file; summarize only if supported by available metadata"
                    .to_string(),
            ),
        })])
    }

    fn validate_model_output(
        &self,
        validator: &jsonschema::Validator,
        value: &Value,
        folder_taxonomy: &BTreeMap<String, Vec<String>>,
    ) -> Result<(), ValidationFailure> {
        validator
            .validate(value)
            .map_err(|error| ValidationFailure {
                message: format!("response JSON schema validation failed: {error}"),
            })?;

        let issues = collect_classification_issues(value, folder_taxonomy).map_err(|error| {
            ValidationFailure {
                message: error.to_string(),
            }
        })?;
        if issues.is_empty() {
            return Ok(());
        }

        Err(ValidationFailure {
            message: issues.join("; "),
        })
    }

    fn write_error_file(&self, job: &EmailJob, error: &anyhow::Error) -> Result<()> {
        let payload = ErrorFile {
            error: format!("{error:#}"),
            failed_at: Utc::now().to_rfc3339(),
        };
        let path = error_sidecar_path(&job.folder, &job.stem);
        fs::write(&path, serde_json::to_string_pretty(&payload)?)
            .with_context(|| format!("unable to write {}", path.display()))?;
        Ok(())
    }
}

fn collect_classification_issues(
    value: &Value,
    folder_taxonomy: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<String>> {
    let attachments = value
        .get("attachment_summaries")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("model response missing `attachment_summaries`"))?;

    let mut issues = Vec::new();
    for attachment in attachments {
        let file_name = attachment
            .get("file_name")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        let main_folder = attachment
            .get("main_folder")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                anyhow!("model response missing `main_folder` for attachment `{file_name}`")
            })?;
        let sub_folder = attachment
            .get("sub_folder")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                anyhow!("model response missing `sub_folder` for attachment `{file_name}`")
            })?;

        let allowed_subfolders = folder_taxonomy.get(main_folder).ok_or_else(|| {
            anyhow!(
                "model returned unsupported main_folder `{main_folder}` for attachment `{file_name}`"
            )
        })?;

        if !allowed_subfolders.iter().any(|value| value == sub_folder) {
            issues.push(format!(
                "model returned unsupported sub_folder `{sub_folder}` for main_folder `{main_folder}` on attachment `{file_name}`; allowed values are: {}",
                allowed_subfolders.join(", ")
            ));
        }
    }

    Ok(issues)
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
        let mut attachment_by_stem = BTreeMap::new();

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
            if let Some(stem) = path.file_stem().and_then(|value| value.to_str()) {
                attachment_by_stem.insert(stem.to_string(), path.clone());
            }
        }

        for (stem, json_path) in json_by_stem {
            let Some(xml_path) = xml_by_stem.get(&stem).cloned() else {
                continue;
            };
            let attachment_path = attachment_by_stem.get(&stem).cloned();
            jobs.push(EmailJob {
                account_name: account.name.clone(),
                folder: folder.clone(),
                stem,
                json_path,
                xml_path,
                attachment_path,
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

fn build_repair_request(
    model: &str,
    prompt_cache_prefix: &str,
    agents: &AgentsSpec,
    account_name: &str,
    email_stem: &str,
    json_payload: &str,
    xml_payload: &str,
    attachments: &[AttachmentSource],
    invalid_output_json: &Value,
    validation_error: &str,
) -> ResponsesRequest {
    let current_json = serde_json::to_string_pretty(invalid_output_json)
        .unwrap_or_else(|_| invalid_output_json.to_string());

    ResponsesRequest {
        model: model.to_string(),
        instructions: build_repair_instructions(agents),
        input: vec![ResponseInputItem {
            role: "user".to_string(),
            content: vec![ResponseContentItem::InputText {
                text: format!(
                    "{}\nValidation errors to fix:\n- {}\n\nCurrent invalid JSON:\n```json\n{}\n```\n",
                    build_primary_prompt(
                        agents,
                        account_name,
                        email_stem,
                        json_payload,
                        xml_payload,
                        attachments,
                    ),
                    validation_error,
                    current_json
                ),
            }],
        }],
        store: false,
        prompt_cache_key: Some(format!(
            "{}:repair",
            build_prompt_cache_key(prompt_cache_prefix, model, agents)
        )),
        text: ResponseTextConfig {
            format: json_schema_format("email_enrichment_repair", agents.output_schema.clone()),
        },
    }
}

fn build_instructions(agents: &AgentsSpec) -> String {
    format!(
        "You are enriching an email dossier for indexing.\n\
Return only JSON matching the provided JSON Schema.\n\
Set `email_importance` to either `HAUTE` or `BASSE`.\n\
Set each attachment `importance` to either `HAUTE` or `BASSE`.\n\
For each attachment, choose exactly one `main_folder` and one `sub_folder` from the allowed taxonomy.\n\
For each attachment, the `sub_folder` must be valid for the selected `main_folder`.\n\
For each attachment, set `proposed_file_name` using the format `yyyy-mm-dd <emetteur-short> <motif>` and preserve the original file extension when known.\n\
Use the email date for `yyyy-mm-dd` when available.\n\
Infer `<emetteur-short>` from the attachment content first whenever possible, otherwise from the email body and headers.\n\
Treat the current local `file_name` only as a weak clue, because it may already contain a poor or generic label.\n\
If the attachment content or summary reveals a more specific documentary source than the current `file_name`, replace the generic label in `proposed_file_name` with that better source.\n\
Avoid generic sender labels such as `gestion`, `document`, `courrier`, `scan`, `piece jointe` or `email` unless the evidence is truly insufficient.\n\
Use a specific organization, brand, person, contract, property, administration, bank, insurer or dossier name when it can be justified from the available content.\n\
Use a concise but informative French reason for `<motif>` that helps distinguish the document from nearby files.\n\
Avoid lame generic motives such as `document`, `fichier`, `piece jointe`, `courrier` or `information` unless nothing more precise is supported.\n\
If evidence is insufficient, say so in the appropriate field and do not invent facts.\n\
Write all free-text JSON values in French.\n\
Specifically, `email_summary` and each attachment `summary` must be written in French.\n\
Keep attachment `main_folder` and `sub_folder` exactly as provided in the taxonomy, without translation.\n\
Follow the business instructions below.\n\n{}",
        agents.instructions
    )
}

fn build_repair_instructions(agents: &AgentsSpec) -> String {
    format!(
        "{}\n\nYou are correcting a previous JSON output.\nReturn the full corrected JSON object, not a patch.\nKeep all valid fields and attachment entries unless they must change to satisfy the taxonomy or schema.\nIf one attachment has an invalid `main_folder` / `sub_folder` pair, correct only the invalid classification while preserving the other useful fields.\nEvery attachment in `attachment_summaries` must remain present in the final JSON.",
        build_instructions(agents)
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
The existing attachment file names may be placeholders and should not be copied blindly into `proposed_file_name`.\n\
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
    let mut json = match serde_json::from_str::<Value>(value) {
        Ok(json) => json,
        Err(_) => return value.to_string(),
    };

    if let Some(attachments) = json.get_mut("attachments").and_then(Value::as_array_mut) {
        for attachment in attachments {
            if let Some(object) = attachment.as_object_mut() {
                object.remove("extracted_text");
                object.remove("extracted_text_method");
                object.remove("extracted_text_truncated");
            }
        }
    }

    serde_json::to_string(&json).unwrap_or_else(|_| value.to_string())
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
    hasher.update(AI_OUTPUT_LAYOUT_VERSION.as_bytes());
    hash_file(&mut hasher, &job.json_path)?;
    hash_file(&mut hasher, &job.xml_path)?;
    hash_file(&mut hasher, agents_path)?;
    if let Some(attachment) = job.attachment_path.as_deref() {
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
    let meta_path = meta_sidecar_path(&job.folder, &job.stem);
    if !output_path.exists() || !meta_path.exists() {
        return Ok(false);
    }

    let meta_payload = fs::read_to_string(&meta_path)
        .with_context(|| format!("unable to read {}", meta_path.display()))?;
    let meta: Value = serde_json::from_str(&meta_payload)
        .with_context(|| format!("unable to parse {}", meta_path.display()))?;
    Ok(meta.get("input_hash").and_then(Value::as_str) == Some(input_hash))
}

fn meta_sidecar_path(folder: &Path, stem: &str) -> PathBuf {
    folder.join(format!("{stem}.ai.meta.json"))
}

fn error_sidecar_path(folder: &Path, stem: &str) -> PathBuf {
    folder.join(format!("{stem}.ai.error.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

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

    #[test]
    fn collects_invalid_attachment_classification_issue() {
        let value = serde_json::json!({
            "attachment_summaries": [
                {
                    "file_name": "doc.docx",
                    "main_folder": "SCI_LES_ROSES",
                    "sub_folder": "AGO"
                }
            ]
        });
        let taxonomy = BTreeMap::from([(
            "SCI_LES_ROSES".to_string(),
            vec!["LEGAL".to_string(), "LOCATION".to_string()],
        )]);

        let issues = collect_classification_issues(&value, &taxonomy).unwrap();
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("allowed values are: LEGAL, LOCATION"));
    }

    #[test]
    fn repair_request_keeps_full_context_and_invalid_json() {
        let agents = AgentsSpec {
            instructions: "## Goal\nTest".to_string(),
            output_schema: serde_json::json!({"type":"object"}),
            folder_taxonomy: BTreeMap::from([(
                "SCI_LES_ROSES".to_string(),
                vec!["LEGAL".to_string()],
            )]),
            cache_fingerprint: "1234567890abcdef1234567890abcdef".to_string(),
        };
        let request = build_repair_request(
            "gpt-4o-mini",
            "ai-enrich",
            &agents,
            "Compte",
            "Sujet",
            "{\"attachments\":[]}",
            "<email-content />",
            &[],
            &serde_json::json!({"attachment_summaries":[{"file_name":"doc.docx"}]}),
            "invalid subfolder",
        );

        let text = match &request.input[0].content[0] {
            ResponseContentItem::InputText { text } => text,
            _ => panic!("expected input text"),
        };
        assert!(text.contains("Validation errors to fix"));
        assert!(text.contains("invalid subfolder"));
        assert!(text.contains("\"file_name\": \"doc.docx\""));
        assert_eq!(
            request.prompt_cache_key.as_deref(),
            Some("ai-enrich:gpt-4o-mini:1234567890abcdef:repair")
        );
    }
}
