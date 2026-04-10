use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use log::{info, warn};
use regex::Regex;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::config::Config;
use crate::dropbox::{DropboxClient, DropboxRemoteFile};
use crate::table::{ArchiveRecord, DropboxArchiveTable};
use crate::utils::{
    build_dropbox_file_path, compute_dropbox_content_hash, compute_dropbox_content_hash_bytes,
    normalize_dropbox_root,
};

const LEGACY_META_SUFFIX: &str = ".dropbox.json";
const ERROR_SUFFIX: &str = ".dropbox.error.json";
const TABLE_LAYOUT_VERSION: &str = "5";

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
    attachment_path: Option<PathBuf>,
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

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AiClassification {
    #[serde(default)]
    email_summary: String,
    #[serde(default)]
    email_importance: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    main_folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sub_folder: Option<String>,
    #[serde(default)]
    attachment_summaries: Vec<AiAttachmentSummary>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AiAttachmentSummary {
    file_name: String,
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    importance: String,
    proposed_file_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    main_folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sub_folder: Option<String>,
}

#[derive(Debug, Clone)]
struct AttachmentUploadPlan {
    source_file_name: String,
    local_path: PathBuf,
    md5: String,
    mime_type: String,
    attachment_summary: String,
    attachment_importance: String,
    main_folder: String,
    sub_folder: String,
    year_folder: String,
    final_file_name: String,
    dropbox_path: String,
    xml_file_name: String,
    xml_dropbox_path: String,
    enriched_xml_content: String,
}

#[derive(Debug, Clone, Serialize)]
struct ErrorFile {
    error: String,
    failed_at: String,
}

#[derive(Debug, Default, Clone)]
struct DropboxRemoteIndex {
    paths_by_content_hash: HashMap<String, Vec<String>>,
}

impl DropboxFiler {
    pub async fn run(&self) -> Result<()> {
        let normalized_root = normalize_dropbox_root(&self.dropbox_root_folder)?;
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

        let remote_files = client.list_files_recursive(&normalized_root).await?;
        let mut remote_index = DropboxRemoteIndex::from_remote_files(remote_files);
        info!(
            "😎 Dropbox remote index [{}] files={}",
            normalized_root,
            remote_index.len()
        );

        let mut processed = 0usize;
        let mut archive_dirty = false;
        for job in jobs {
            if let Some(limit) = self.limit
                && processed >= limit
            {
                break;
            }
            processed += 1;

            match self
                .process_job(
                    &client,
                    &mut archive,
                    &mut remote_index,
                    &normalized_root,
                    &job,
                )
                .await
            {
                Ok(job_dirty) => {
                    archive_dirty |= job_dirty;
                }
                Err(error) => {
                    warn!(
                        "💣 Dropbox filing failed [{}]: {error:#}",
                        job.folder.display()
                    );
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
        remote_index: &mut DropboxRemoteIndex,
        normalized_root: &str,
        job: &EmailJob,
    ) -> Result<bool> {
        let parsed_payload = fs::read_to_string(&job.json_path)
            .with_context(|| format!("unable to read {}", job.json_path.display()))?;
        let source_xml_payload = fs::read_to_string(&job.xml_path)
            .with_context(|| format!("unable to read {}", job.xml_path.display()))?;
        let ai_payload = fs::read_to_string(&job.ai_path)
            .with_context(|| format!("unable to read {}", job.ai_path.display()))?;
        let parsed = serde_json::from_str::<ParsedEmailMetadata>(&parsed_payload)
            .with_context(|| format!("unable to parse {}", job.json_path.display()))?;
        let ai = serde_json::from_str::<AiClassification>(&ai_payload)
            .with_context(|| format!("unable to parse {}", job.ai_path.display()))?;
        let ai = normalize_ai_classification(ai)?;
        let normalized_ai_payload = serde_json::to_string_pretty(&ai)
            .with_context(|| format!("unable to normalize {}", job.ai_path.display()))?;
        let Some(attachment_path) = job.attachment_path.as_deref() else {
            info!(
                "😎 Skip metadata-only bundle without local attachment [{}]",
                job.folder.display()
            );
            return Ok(false);
        };

        let plan = build_upload_plan(
            &normalized_root,
            attachment_path,
            &job.xml_path,
            &parsed,
            &ai,
            &source_xml_payload,
            &normalized_ai_payload,
        )?;

        let Some(plan) = plan else {
            info!("😎 No attachment left to file [{}]", job.folder.display());
            return Ok(false);
        };

        info!("📦 File attachments to Dropbox [{}]", job.folder.display());

        let dropbox_content_hash = compute_dropbox_content_hash(&plan.local_path)?;
        if archive.contains_md5(&plan.md5) {
            if let Some(existing_paths) = remote_index.paths_for_content_hash(&dropbox_content_hash)
            {
                info!(
                    "😎 Skip attachment already archived by MD5 + checksum Dropbox [{}] -> [{}]",
                    plan.local_path.display(),
                    existing_paths[0]
                );
                return Ok(false);
            }

            info!(
                "😎 MD5 already indexed but no matching checksum found on Dropbox, re-upload [{}] -> [{}]",
                plan.local_path.display(),
                plan.dropbox_path
            );
        }

        if self.dry_run {
            info!(
                "😎 Dry run upload [{}] -> [{}] and [{}]",
                plan.local_path.display(),
                plan.dropbox_path,
                plan.xml_dropbox_path
            );
            return Ok(false);
        }

        client.ensure_parent_folders(&plan.dropbox_path).await?;
        client
            .upload_file(&plan.local_path, &plan.dropbox_path)
            .await
            .with_context(|| format!("unable to upload {}", plan.local_path.display()))?;
        client
            .upload_bytes(plan.enriched_xml_content.as_bytes(), &plan.xml_dropbox_path)
            .await
            .with_context(|| {
                format!(
                    "unable to upload companion XML for {}",
                    plan.local_path.display()
                )
            })?;
        remote_index.add_file(plan.dropbox_path.clone(), dropbox_content_hash.clone());
        remote_index.add_file(
            plan.xml_dropbox_path.clone(),
            compute_dropbox_content_hash_bytes(plan.enriched_xml_content.as_bytes()),
        );
        archive.append(build_archive_record(
            job,
            normalized_root,
            &parsed_payload,
            &plan.enriched_xml_content,
            &normalized_ai_payload,
            &parsed,
            &ai,
            &plan,
            &dropbox_content_hash,
        )?)?;
        info!(
            "😎 Uploaded [{}] -> [{}] and [{}]",
            plan.local_path.display(),
            plan.dropbox_path,
            plan.xml_dropbox_path
        );

        Ok(true)
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

            if let Some(stem) = path.file_stem().and_then(|value| value.to_str()) {
                attachment_by_stem.insert(stem.to_string(), path.clone());
            }
        }

        for (stem, json_path) in json_by_stem {
            let Some(xml_path) = xml_by_stem.get(&stem).cloned() else {
                continue;
            };
            let Some(ai_path) = ai_by_stem.get(&stem).cloned() else {
                continue;
            };
            let attachment_path = attachment_by_stem.get(&stem).cloned();
            jobs.push(EmailJob {
                account_name: account_name.to_string(),
                folder: folder.clone(),
                stem,
                json_path,
                xml_path,
                ai_path,
                attachment_path,
            });
        }
    }

    Ok(jobs)
}

fn build_upload_plan(
    dropbox_root_folder: &str,
    attachment_path: &Path,
    xml_path: &Path,
    parsed: &ParsedEmailMetadata,
    ai: &AiClassification,
    source_xml_payload: &str,
    ai_payload: &str,
) -> Result<Option<AttachmentUploadPlan>> {
    if ai.attachment_summaries.is_empty() {
        return Ok(None);
    }
    if ai.attachment_summaries.len() != 1 {
        bail!(
            "expected exactly one attachment summary in per-attachment AI output, got {}",
            ai.attachment_summaries.len()
        );
    }

    let attachment = &ai.attachment_summaries[0];
    let parsed_attachment = parsed
        .attachments
        .first()
        .ok_or_else(|| anyhow!("parse metadata contains no attachment"))?;
    let source_file_name = attachment_path
        .file_name()
        .and_then(|value| value.to_str())
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| parsed_attachment.original_name.clone());
    let final_file_name = source_file_name.clone();
    let xml_file_name = xml_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow!("unable to resolve XML file name for {}", xml_path.display()))?
        .to_string();
    let main_folder = attachment.main_folder.clone().ok_or_else(|| {
        anyhow!(
            "AI output is missing `main_folder` for `{}`",
            attachment.file_name
        )
    })?;
    let sub_folder = attachment.sub_folder.clone().ok_or_else(|| {
        anyhow!(
            "AI output is missing `sub_folder` for `{}`",
            attachment.file_name
        )
    })?;
    let year_folder = resolve_year_folder(parsed, &[attachment])?;

    Ok(Some(AttachmentUploadPlan {
        source_file_name,
        local_path: attachment_path.to_path_buf(),
        md5: parsed_attachment.md5.clone(),
        mime_type: attachment
            .mime_type
            .clone()
            .or_else(|| Some(parsed_attachment.mime_type.clone()))
            .unwrap_or_default(),
        attachment_summary: attachment.summary.clone(),
        attachment_importance: attachment.importance.clone(),
        main_folder,
        sub_folder,
        year_folder,
        final_file_name: final_file_name.clone(),
        dropbox_path: build_dropbox_file_path(dropbox_root_folder, &final_file_name)?,
        xml_file_name: xml_file_name.clone(),
        xml_dropbox_path: build_dropbox_file_path(dropbox_root_folder, &xml_file_name)?,
        enriched_xml_content: embed_ai_payload_in_xml(source_xml_payload, ai_payload),
    }))
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
    dropbox_content_hash: &str,
) -> Result<ArchiveRecord> {
    Ok(ArchiveRecord {
        layout_version: TABLE_LAYOUT_VERSION.to_string(),
        uploaded_at: Utc::now().to_rfc3339(),
        md5: plan.md5.clone(),
        source_file_name: plan.source_file_name.clone(),
        final_file_name: plan.final_file_name.clone(),
        dropbox_root_folder: normalized_root.to_string(),
        dropbox_path: plan.dropbox_path.clone(),
        dropbox_content_hash: dropbox_content_hash.to_string(),
        xml_file_name: plan.xml_file_name.clone(),
        xml_dropbox_path: plan.xml_dropbox_path.clone(),
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
        attachment_mime_type: plan.mime_type.clone(),
    })
}

fn normalize_ai_classification(mut ai: AiClassification) -> Result<AiClassification> {
    let fallback_main_folder = ai.main_folder.clone();
    let fallback_sub_folder = ai.sub_folder.clone();

    for attachment in &mut ai.attachment_summaries {
        if attachment.main_folder.is_none() {
            attachment.main_folder = fallback_main_folder.clone();
        }
        if attachment.sub_folder.is_none() {
            attachment.sub_folder = fallback_sub_folder.clone();
        }

        if attachment.main_folder.is_none() {
            bail!(
                "AI output is missing `main_folder` for attachment `{}`",
                attachment.file_name
            );
        }
        if attachment.sub_folder.is_none() {
            bail!(
                "AI output is missing `sub_folder` for attachment `{}`",
                attachment.file_name
            );
        }
    }

    ai.main_folder = None;
    ai.sub_folder = None;
    Ok(ai)
}

impl DropboxRemoteIndex {
    fn from_remote_files(files: Vec<DropboxRemoteFile>) -> Self {
        let mut index = Self::default();
        for file in files {
            index.add_file(file.path_display, file.content_hash);
        }
        index
    }

    fn add_file(&mut self, path: String, content_hash: String) {
        self.paths_by_content_hash
            .entry(content_hash)
            .or_default()
            .push(path);
    }

    fn paths_for_content_hash(&self, content_hash: &str) -> Option<&Vec<String>> {
        self.paths_by_content_hash.get(content_hash)
    }

    fn len(&self) -> usize {
        self.paths_by_content_hash
            .values()
            .map(|paths| paths.len())
            .sum()
    }
}

fn embed_ai_payload_in_xml(xml_payload: &str, ai_payload: &str) -> String {
    let ai_block = format!(
        "  <ai-enrich><![CDATA[{}]]></ai-enrich>\n",
        wrap_cdata(ai_payload)
    );
    let regex =
        Regex::new(r"(?s)\s*<ai-enrich><!\[CDATA\[.*?\]\]></ai-enrich>\s*").expect("invalid regex");
    let cleaned = regex.replace_all(xml_payload, "\n").into_owned();

    if let Some(index) = cleaned.rfind("</email-content>") {
        let (head, tail) = cleaned.split_at(index);
        format!("{head}{ai_block}{tail}")
    } else {
        format!("{cleaned}\n{ai_block}")
    }
}

fn wrap_cdata(value: &str) -> String {
    value.replace("]]>", "]]]]><![CDATA[>")
}

fn resolve_year_folder(
    parsed: &ParsedEmailMetadata,
    attachments: &[&AiAttachmentSummary],
) -> Result<String> {
    let mut years = attachments
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
    regex
        .find(value)
        .map(|capture| capture.as_str().to_string())
}

fn error_path(job: &EmailJob) -> PathBuf {
    job.folder.join(format!("{}{}", job.stem, ERROR_SUFFIX))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        AiAttachmentSummary, AiClassification, DropboxRemoteIndex, ParsedAttachment,
        ParsedEmailMetadata, build_upload_plan, normalize_ai_classification,
    };
    use crate::dropbox::DropboxRemoteFile;

    #[test]
    fn builds_upload_plan_from_ai_and_parse_outputs() {
        let plan = build_upload_plan(
            "/Archives",
            &PathBuf::from("/tmp/2024-03-15 techvalley fin-contrat.pdf"),
            &PathBuf::from("/tmp/2024-03-15 techvalley fin-contrat.xml"),
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
                main_folder: None,
                sub_folder: None,
                attachment_summaries: vec![AiAttachmentSummary {
                    file_name: "courrier.pdf".to_string(),
                    mime_type: Some("application/pdf".to_string()),
                    summary: "contrat".to_string(),
                    importance: "HAUTE".to_string(),
                    proposed_file_name: "2024-03-15 techvalley fin-contrat".to_string(),
                    main_folder: Some("DENIS".to_string()),
                    sub_folder: Some("LEGAL".to_string()),
                }],
            },
            "<email-content></email-content>",
            "{\"attachment_summaries\":[{\"file_name\":\"courrier.pdf\",\"main_folder\":\"DENIS\",\"sub_folder\":\"LEGAL\"}]}",
        )
        .unwrap();

        let plan = plan.expect("expected one plan");
        assert_eq!(
            plan.dropbox_path,
            "/Archives/A_TRAITER/2024-03-15 techvalley fin-contrat.pdf"
        );
        assert_eq!(
            plan.xml_dropbox_path,
            "/Archives/A_TRAITER/2024-03-15 techvalley fin-contrat.xml"
        );
        assert!(plan.enriched_xml_content.contains("\"main_folder\":\"DENIS\""));
        assert!(plan.enriched_xml_content.contains("\"sub_folder\":\"LEGAL\""));
    }

    #[test]
    fn normalizes_legacy_top_level_ai_classification() {
        let normalized = normalize_ai_classification(AiClassification {
            email_summary: String::new(),
            email_importance: String::new(),
            main_folder: Some("DENIS".to_string()),
            sub_folder: Some("LEGAL".to_string()),
            attachment_summaries: vec![AiAttachmentSummary {
                file_name: "courrier.pdf".to_string(),
                mime_type: None,
                summary: String::new(),
                importance: "HAUTE".to_string(),
                proposed_file_name: "2024-03-15 test courrier.pdf".to_string(),
                main_folder: None,
                sub_folder: None,
            }],
        })
        .unwrap();

        assert!(normalized.main_folder.is_none());
        assert!(normalized.sub_folder.is_none());
        assert_eq!(
            normalized.attachment_summaries[0].main_folder.as_deref(),
            Some("DENIS")
        );
        assert_eq!(
            normalized.attachment_summaries[0].sub_folder.as_deref(),
            Some("LEGAL")
        );
    }

    #[test]
    fn indexes_dropbox_files_by_content_hash() {
        let index = DropboxRemoteIndex::from_remote_files(vec![
            DropboxRemoteFile {
                path_display: "/COBRA_TEST/A_TRAITER/a.pdf".to_string(),
                content_hash: "hash-a".to_string(),
            },
            DropboxRemoteFile {
                path_display: "/COBRA_TEST/Archives/a.pdf".to_string(),
                content_hash: "hash-a".to_string(),
            },
        ]);

        assert_eq!(index.len(), 2);
        assert_eq!(
            index.paths_for_content_hash("hash-a").map(Vec::len),
            Some(2)
        );
    }
}
