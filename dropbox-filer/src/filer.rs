use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use log::{info, warn};
use regex::Regex;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::config::Config;
use crate::dropbox::DropboxClient;
use crate::table::{ArchiveRecord, DropboxArchiveTable};
use crate::utils::{build_dropbox_file_path, ensure_original_extension, normalize_dropbox_root};

const LEGACY_META_SUFFIX: &str = ".dropbox.json";
const ERROR_SUFFIX: &str = ".dropbox.error.json";
const TABLE_LAYOUT_VERSION: &str = "3";

#[derive(Debug, Clone)]
pub struct DropboxFiler {
    pub config: Config,
    pub dropbox_root_folder: String,
    pub access_token: String,
    pub force: bool,
    pub limit: Option<usize>,
    pub account_filter: Option<String>,
    pub dry_run: bool,
}

#[derive(Debug, Clone)]
struct EmailJob {
    account_name: String,
    folder: PathBuf,
    stem: String,
    json_path: PathBuf,
    xml_path: PathBuf,
    ai_path: PathBuf,
    attachments: Vec<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
struct ParsedEmailMetadata {
    #[serde(default)]
    email_folder: String,
    #[serde(default)]
    message_id: Option<String>,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    expedition_date: Option<String>,
    #[serde(default)]
    attachments: Vec<ParsedAttachment>,
}

#[derive(Debug, Clone, Deserialize)]
struct ParsedAttachment {
    original_name: String,
    #[serde(default)]
    mime_type: String,
    md5: String,
}

#[derive(Debug, Clone, Deserialize)]
struct AiClassification {
    #[serde(default)]
    email_summary: String,
    #[serde(default)]
    email_importance: String,
    main_folder: String,
    sub_folder: String,
    #[serde(default)]
    attachment_summaries: Vec<AiAttachmentSummary>,
}

#[derive(Debug, Clone, Deserialize)]
struct AiAttachmentSummary {
    file_name: String,
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    confidence: Option<f64>,
    #[serde(default)]
    importance: String,
    proposed_file_name: String,
}

#[derive(Debug, Clone)]
struct AttachmentUploadPlan {
    source_file_name: String,
    local_path: PathBuf,
    md5: String,
    mime_type: String,
    attachment_summary: String,
    attachment_importance: String,
    attachment_confidence: String,
    main_folder: String,
    sub_folder: String,
    year_folder: String,
    final_file_name: String,
    dropbox_path: String,
}

#[derive(Debug, Clone, Serialize)]
struct ErrorFile {
    error: String,
    failed_at: String,
}

impl DropboxFiler {
    pub async fn run(&self) -> Result<()> {
        let client = DropboxClient::new(
            self.access_token.clone(),
            self.config.dropbox_api_base_url.as_deref(),
            self.config.dropbox_content_base_url.as_deref(),
            self.config.dropbox_timeout_seconds,
        )?;
        let mut archive = DropboxArchiveTable::load(Path::new(&self.config.email_folder)).await?;
        info!(
            "😎 Dropbox archive table [{}] rows={}",
            archive.path().display(),
            archive.len()
        );
        if self.force {
            info!("😎 `--force` keeps MD5 deduplication from the Parquet archive table");
        }

        let jobs = self.discover_jobs()?;
        if jobs.is_empty() {
            info!("😎 No AI-enriched parse folders found for Dropbox filing");
            return Ok(());
        }

        let mut processed = 0usize;
        let mut archive_dirty = false;
        for job in jobs {
            if let Some(limit) = self.limit
                && processed >= limit
            {
                break;
            }
            processed += 1;

            match self.process_job(&client, &mut archive, &job).await {
                Ok(job_dirty) => {
                    archive_dirty |= job_dirty;
                }
                Err(error) => {
                warn!("💣 Dropbox filing failed [{}]: {error:#}", job.folder.display());
                self.write_error_file(&job, &error)?;
                }
            }
        }

        if !self.dry_run && archive_dirty {
            archive.persist()?;
            info!(
                "😎 Persist Dropbox archive table [{}] rows={}",
                archive.path().display(),
                archive.len()
            );
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
                &account.name,
                &self.config.ai_output_suffix,
            )?);
        }

        jobs.sort_by(|left, right| left.folder.cmp(&right.folder));
        Ok(jobs)
    }

    async fn process_job(
        &self,
        client: &DropboxClient,
        archive: &mut DropboxArchiveTable,
        job: &EmailJob,
    ) -> Result<bool> {
        let normalized_root = normalize_dropbox_root(&self.dropbox_root_folder)?;
        let parsed_payload = fs::read_to_string(&job.json_path)
            .with_context(|| format!("unable to read {}", job.json_path.display()))?;
        let xml_payload = fs::read_to_string(&job.xml_path)
            .with_context(|| format!("unable to read {}", job.xml_path.display()))?;
        let ai_payload = fs::read_to_string(&job.ai_path)
            .with_context(|| format!("unable to read {}", job.ai_path.display()))?;
        let parsed = serde_json::from_str::<ParsedEmailMetadata>(&parsed_payload)
            .with_context(|| format!("unable to parse {}", job.json_path.display()))?;
        let ai = serde_json::from_str::<AiClassification>(&ai_payload)
            .with_context(|| format!("unable to parse {}", job.ai_path.display()))?;
        let plans = build_upload_plans(&normalized_root, &job.attachments, &parsed, &ai)?;

        if plans.is_empty() {
            info!("😎 No attachment to file [{}]", job.folder.display());
            remove_error_file(job)?;
            return Ok(false);
        }

        info!("📦 File attachments to Dropbox [{}]", job.folder.display());
        let mut archive_dirty = false;

        for plan in plans {
            if archive.contains_md5(&plan.md5) {
                if let Some(existing_path) = archive.existing_path_for_md5(&plan.md5) {
                    if client.file_exists(existing_path).await? {
                        info!(
                            "😎 Skip attachment already archived by MD5 [{}] -> [{}]",
                            plan.local_path.display(),
                            existing_path
                        );
                        continue;
                    }

                    info!(
                        "😎 MD5 already indexed but missing on Dropbox, re-upload [{}] -> [{}]",
                        plan.local_path.display(),
                        plan.dropbox_path
                    );
                } else {
                    info!(
                        "😎 MD5 already indexed without path, re-upload [{}] -> [{}]",
                        plan.local_path.display(),
                        plan.dropbox_path
                    );
                }
            }

            if self.dry_run {
                info!(
                    "😎 Dry run upload [{}] -> [{}]",
                    plan.local_path.display(),
                    plan.dropbox_path
                );
                continue;
            }

            client.ensure_parent_folders(&plan.dropbox_path).await?;
            client
                .upload_file(&plan.local_path, &plan.dropbox_path)
                .await
                .with_context(|| format!("unable to upload {}", plan.local_path.display()))?;
            archive.append(build_archive_record(
                job,
                &normalized_root,
                &parsed_payload,
                &xml_payload,
                &ai_payload,
                &parsed,
                &ai,
                &plan,
            )?)?;
            archive_dirty = true;
            info!(
                "😎 Uploaded [{}] -> [{}]",
                plan.local_path.display(),
                plan.dropbox_path
            );
        }

        remove_error_file(job)?;
        Ok(archive_dirty)
    }

    fn write_error_file(&self, job: &EmailJob, error: &anyhow::Error) -> Result<()> {
        let payload = ErrorFile {
            error: format!("{error:#}"),
            failed_at: Utc::now().to_rfc3339(),
        };
        let path = error_path(job);
        fs::write(&path, serde_json::to_string_pretty(&payload)?)
            .with_context(|| format!("unable to write {}", path.display()))?;
        Ok(())
    }
}

fn discover_account_jobs(
    parse_folder: &Path,
    account_name: &str,
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
        let mut ai_by_stem = BTreeMap::new();
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
                || name.ends_with(LEGACY_META_SUFFIX)
                || name.ends_with(ERROR_SUFFIX)
            {
                continue;
            }

            if name.ends_with(ai_output_suffix) {
                let stem = name.trim_end_matches(ai_output_suffix).to_string();
                ai_by_stem.insert(stem, path.clone());
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
            let Some(ai_path) = ai_by_stem.get(&stem).cloned() else {
                continue;
            };
            jobs.push(EmailJob {
                account_name: account_name.to_string(),
                folder: folder.clone(),
                stem,
                json_path,
                xml_path,
                ai_path,
                attachments: attachments.clone(),
            });
        }
    }

    Ok(jobs)
}

fn build_upload_plans(
    dropbox_root_folder: &str,
    attachment_paths: &[PathBuf],
    parsed: &ParsedEmailMetadata,
    ai: &AiClassification,
) -> Result<Vec<AttachmentUploadPlan>> {
    let local_by_name = attachment_paths
        .iter()
        .map(|path| {
            let file_name = path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| {
                    anyhow!("unable to resolve attachment file name for {}", path.display())
                })?
                .to_string();
            Ok((file_name, path.clone()))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let parsed_by_name = parsed
        .attachments
        .iter()
        .map(|attachment| (attachment.original_name.clone(), attachment))
        .collect::<BTreeMap<_, _>>();

    let mut plans = Vec::new();
    let mut seen = HashSet::new();
    let year_folder = resolve_year_folder(parsed, ai)?;
    for attachment in &ai.attachment_summaries {
        let local_path = local_by_name.get(&attachment.file_name).ok_or_else(|| {
            anyhow!(
                "AI output references missing attachment `{}`",
                attachment.file_name
            )
        })?;
        let parsed_attachment = parsed_by_name.get(&attachment.file_name).ok_or_else(|| {
            anyhow!(
                "parse metadata is missing attachment `{}`",
                attachment.file_name
            )
        })?;
        let final_file_name =
            ensure_original_extension(&attachment.proposed_file_name, &attachment.file_name)?;
        let dropbox_path = build_dropbox_file_path(
            dropbox_root_folder,
            &ai.main_folder,
            &ai.sub_folder,
            &year_folder,
            &final_file_name,
        )?;
        seen.insert(attachment.file_name.clone());
        plans.push(AttachmentUploadPlan {
            source_file_name: attachment.file_name.clone(),
            local_path: local_path.clone(),
            md5: parsed_attachment.md5.clone(),
            mime_type: attachment
                .mime_type
                .clone()
                .or_else(|| Some(parsed_attachment.mime_type.clone()))
                .unwrap_or_default(),
            attachment_summary: attachment.summary.clone(),
            attachment_importance: attachment.importance.clone(),
            attachment_confidence: attachment
                .confidence
                .map(|value| value.to_string())
                .unwrap_or_default(),
            main_folder: ai.main_folder.clone(),
            sub_folder: ai.sub_folder.clone(),
            year_folder: year_folder.clone(),
            final_file_name,
            dropbox_path,
        });
    }

    let missing_from_ai = local_by_name
        .keys()
        .filter(|name| !seen.contains(*name))
        .cloned()
        .collect::<Vec<_>>();
    if !missing_from_ai.is_empty() {
        bail!(
            "AI output is missing attachment_summaries for: {}",
            missing_from_ai.join(", ")
        );
    }

    plans.sort_by(|left, right| left.source_file_name.cmp(&right.source_file_name));
    Ok(plans)
}

fn build_archive_record(
    job: &EmailJob,
    normalized_root: &str,
    parsed_payload: &str,
    xml_payload: &str,
    ai_payload: &str,
    parsed: &ParsedEmailMetadata,
    ai: &AiClassification,
    plan: &AttachmentUploadPlan,
) -> Result<ArchiveRecord> {
    Ok(ArchiveRecord {
        layout_version: TABLE_LAYOUT_VERSION.to_string(),
        uploaded_at: Utc::now().to_rfc3339(),
        md5: plan.md5.clone(),
        source_file_name: plan.source_file_name.clone(),
        final_file_name: plan.final_file_name.clone(),
        dropbox_root_folder: normalized_root.to_string(),
        dropbox_path: plan.dropbox_path.clone(),
        main_folder: plan.main_folder.clone(),
        sub_folder: plan.sub_folder.clone(),
        year_folder: plan.year_folder.clone(),
        account_name: job.account_name.clone(),
        email_dossier_path: job.folder.display().to_string(),
        email_stem: job.stem.clone(),
        attachment_local_path: plan.local_path.display().to_string(),
        parse_json_path: job.json_path.display().to_string(),
        parse_json_content: parsed_payload.to_string(),
        parse_xml_path: job.xml_path.display().to_string(),
        parse_xml_content: xml_payload.to_string(),
        ai_json_path: job.ai_path.display().to_string(),
        ai_json_content: ai_payload.to_string(),
        message_id: parsed.message_id.clone().unwrap_or_default(),
        email_subject: if parsed.subject.is_empty() {
            parsed.email_folder.clone()
        } else {
            parsed.subject.clone()
        },
        expedition_date: parsed.expedition_date.clone().unwrap_or_default(),
        email_summary: ai.email_summary.clone(),
        email_importance: ai.email_importance.clone(),
        attachment_summary: plan.attachment_summary.clone(),
        attachment_importance: plan.attachment_importance.clone(),
        attachment_confidence: plan.attachment_confidence.clone(),
        attachment_mime_type: plan.mime_type.clone(),
    })
}

fn resolve_year_folder(parsed: &ParsedEmailMetadata, ai: &AiClassification) -> Result<String> {
    let mut years = ai
        .attachment_summaries
        .iter()
        .filter_map(|attachment| extract_year(&attachment.proposed_file_name))
        .collect::<Vec<_>>();
    years.sort();
    years.dedup();

    match years.as_slice() {
        [year] => Ok(year.clone()),
        [] => {
            if let Some(expedition_date) = parsed.expedition_date.as_deref()
                && let Some(year) = extract_year(expedition_date)
            {
                Ok(year)
            } else {
                bail!(
                    "unable to determine year folder: missing year in `proposed_file_name` and `expedition_date`"
                )
            }
        }
        _ => {
            if let Some(expedition_date) = parsed.expedition_date.as_deref()
                && let Some(year) = extract_year(expedition_date)
            {
                Ok(year)
            } else {
                bail!(
                    "unable to determine a unique year folder from AI proposed file names and no valid `expedition_date` fallback exists: {}",
                    years.join(", ")
                )
            }
        }
    }
}

fn extract_year(value: &str) -> Option<String> {
    let regex = Regex::new(r"\b(19|20)\d{2}\b").expect("invalid regex");
    regex.find(value).map(|capture| capture.as_str().to_string())
}

fn error_path(job: &EmailJob) -> PathBuf {
    job.folder.join(format!("{}{}", job.stem, ERROR_SUFFIX))
}

fn remove_error_file(job: &EmailJob) -> Result<()> {
    let path = error_path(job);
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("unable to remove {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        AiAttachmentSummary, AiClassification, ParsedAttachment, ParsedEmailMetadata,
        build_upload_plans,
    };

    #[test]
    fn builds_upload_plan_from_ai_and_parse_outputs() {
        let plans = build_upload_plans(
            "/Archives",
            &[PathBuf::from("/tmp/courrier.pdf")],
            &ParsedEmailMetadata {
                email_folder: String::new(),
                message_id: None,
                subject: String::new(),
                expedition_date: Some("2023-03-15T12:34:56+00:00".to_string()),
                attachments: vec![ParsedAttachment {
                    original_name: "courrier.pdf".to_string(),
                    mime_type: "application/pdf".to_string(),
                    md5: "abc".to_string(),
                }],
            },
            &AiClassification {
                email_summary: String::new(),
                email_importance: String::new(),
                main_folder: "DENIS".to_string(),
                sub_folder: "LEGAL".to_string(),
                attachment_summaries: vec![AiAttachmentSummary {
                    file_name: "courrier.pdf".to_string(),
                    mime_type: Some("application/pdf".to_string()),
                    summary: "contrat".to_string(),
                    confidence: Some(0.91),
                    importance: "HAUTE".to_string(),
                    proposed_file_name: "2024-03-15 techvalley fin-contrat".to_string(),
                }],
            },
        )
        .unwrap();

        assert_eq!(plans.len(), 1);
        assert_eq!(
            plans[0].dropbox_path,
            "/Archives/DENIS/LEGAL/2024/2024-03-15 techvalley fin-contrat.pdf"
        );
    }
}
