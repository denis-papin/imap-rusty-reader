use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use html_escape::decode_html_entities;
use html_escape::encode_double_quoted_attribute;
use log::{info, warn};
use mailparse::{DispositionType, MailHeaderMap, ParsedMail};
use regex::Regex;
use serde::Serialize;

use crate::metadata::embed_custom_metadata;
use crate::pdf_text::extract_pdf_text;
use crate::utils::{
    file_stem_or_name, make_unique_path, normalize_display_name, sanitize_filename,
};

const EMBEDDED_IMAGE_MAX_BYTES: usize = 30 * 1024;
const HTML_TO_MARKDOWN_MAX_CHARS: usize = 250_000;

#[derive(Debug)]
pub struct BackupParser {
    account_folder: PathBuf,
    parse_folder: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
struct ParsedEmailRecord {
    parsing_folder: String,
    email_folder: String,
    email_file_name: String,
    message_id: Option<String>,
    subject: String,
    expedition_date: Option<String>,
    author: HeaderContact,
    targets: Vec<HeaderContact>,
    cc: Vec<HeaderContact>,
    bcc: Vec<HeaderContact>,
    attachments: Vec<AttachmentRecord>,
}

#[derive(Debug, Clone, Serialize)]
struct HeaderContact {
    raw: String,
    name: Option<String>,
    address: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct AttachmentRecord {
    original_name: String,
    mime_type: String,
    size: usize,
    md5: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    extracted_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extracted_text_method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extracted_text_truncated: Option<bool>,
}

#[derive(Debug)]
struct ExtractedAttachment {
    path: Option<PathBuf>,
    record: AttachmentRecord,
}

#[derive(Debug, Clone)]
struct ParsedEmailContext {
    parsing_folder: String,
    email_folder: String,
    email_file_name: String,
    message_id: Option<String>,
    subject: String,
    expedition_date: Option<String>,
    author: HeaderContact,
    targets: Vec<HeaderContact>,
    cc: Vec<HeaderContact>,
    bcc: Vec<HeaderContact>,
}

impl BackupParser {
    pub fn new(email_folder: &str, parse_ia_folder: &str, account_name: &str) -> Self {
        let account_folder = Path::new(email_folder).join(account_name);
        let parse_folder = account_folder.join(parse_ia_folder);
        Self {
            account_folder,
            parse_folder,
        }
    }

    pub fn parse_account_backup(&self) -> Result<()> {
        if !self.account_folder.exists() {
            warn!(
                "💣 Account folder missing, skipped: {}",
                self.account_folder.display()
            );
            return Ok(());
        }

        fs::create_dir_all(&self.parse_folder)
            .with_context(|| format!("unable to create {}", self.parse_folder.display()))?;

        info!("🚀 Parse backup folder [{}]", self.account_folder.display());
        self.walk_folder(&self.account_folder)?;
        info!("🏁 Parse backup folder [{}]", self.account_folder.display());
        Ok(())
    }

    fn walk_folder(&self, folder: &Path) -> Result<()> {
        for entry in
            fs::read_dir(folder).with_context(|| format!("unable to read {}", folder.display()))?
        {
            let entry = entry?;
            let path = entry.path();

            if path == self.parse_folder {
                continue;
            }

            if path.is_dir() {
                self.walk_folder(&path)?;
                continue;
            }

            let is_eml = path
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| value.eq_ignore_ascii_case("eml"))
                .unwrap_or(false);
            if is_eml {
                self.parse_email(&path);
            }
        }
        Ok(())
    }

    fn parse_email(&self, email_path: &Path) {
        if let Err(error) = self.parse_email_inner(email_path) {
            warn!(
                "💣 Parse email failed [{}]: {error:#}",
                email_path.display()
            );
        }
    }

    fn parse_email_inner(&self, email_path: &Path) -> Result<()> {
        info!("😎 Parse email [{}]", email_path.display());

        let relative_email_path =
            email_path
                .strip_prefix(&self.account_folder)
                .with_context(|| {
                    format!(
                        "unable to compute relative path for {}",
                        email_path.display()
                    )
                })?;

        let relative_parent = relative_email_path
            .parent()
            .unwrap_or_else(|| Path::new(""));
        let email_stem = file_stem_or_name(email_path);
        let parsing_folder_name = folder_name_or_default(relative_parent, "");
        let email_output_folder = self.parse_folder.join(relative_parent).join(&email_stem);
        if email_output_folder.exists() {
            info!(
                "😎 Skip email already parsed [{}]",
                email_output_folder.display()
            );
            return Ok(());
        }
        let raw = fs::read(email_path)
            .with_context(|| format!("unable to read {}", email_path.display()))?;
        let parsed = mailparse::parse_mail(&raw)
            .with_context(|| format!("unable to parse {}", email_path.display()))?;

        let attachments = self.extract_attachments(&parsed, &email_output_folder)?;
        if attachments.is_empty() {
            info!("😎 No attachment found [{}]", email_path.display());
            return Ok(());
        }

        let context = ParsedEmailContext {
            parsing_folder: parsing_folder_name,
            email_folder: email_stem.clone(),
            email_file_name: email_path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_string(),
            message_id: first_header(&parsed, "Message-ID"),
            subject: normalize_subject(first_header(&parsed, "Subject").unwrap_or_default()),
            expedition_date: parse_iso_timestamp(first_header(&parsed, "Date")),
            author: parse_single_contact(first_header(&parsed, "From")),
            targets: parse_contact_list(first_header(&parsed, "To")),
            cc: parse_contact_list(first_header(&parsed, "Cc")),
            bcc: parse_contact_list(first_header(&parsed, "Bcc")),
        };

        let mut written_sidecars = 0usize;
        for attachment in &attachments {
            match self.write_attachment_bundle(&parsed, &email_output_folder, &context, attachment) {
                Ok(()) => written_sidecars += 1,
                Err(error) => warn!(
                    "💣 Unable to write attachment metadata [{} / {}]: {error:#}",
                    email_path.display(),
                    attachment.record.original_name
                ),
            }
        }

        if written_sidecars == 0 {
            self.cleanup_failed_email_folder(&email_output_folder);
            anyhow::bail!(
                "no attachment metadata bundle could be written for {}",
                email_path.display()
            );
        }

        Ok(())
    }

    fn write_attachment_bundle(
        &self,
        parsed: &ParsedMail<'_>,
        email_output_folder: &Path,
        context: &ParsedEmailContext,
        attachment: &ExtractedAttachment,
    ) -> Result<()> {
        fs::create_dir_all(email_output_folder)
            .with_context(|| format!("unable to create {}", email_output_folder.display()))?;

        let record = ParsedEmailRecord {
            parsing_folder: context.parsing_folder.clone(),
            email_folder: context.email_folder.clone(),
            email_file_name: context.email_file_name.clone(),
            message_id: context.message_id.clone(),
            subject: context.subject.clone(),
            expedition_date: context.expedition_date.clone(),
            author: context.author.clone(),
            targets: context.targets.clone(),
            cc: context.cc.clone(),
            bcc: context.bcc.clone(),
            attachments: vec![attachment.record.clone()],
        };

        let payload = serde_json::to_string_pretty(&record)?;
        let stem = attachment_bundle_stem(&attachment.record.original_name);
        let json_path = email_output_folder.join(format!("{}.json", stem));
        fs::write(&json_path, &payload)
            .with_context(|| format!("unable to write {}", json_path.display()))?;
        self.write_email_xml(parsed, email_output_folder, &record, &payload, &stem)?;
        if let Err(error) = self.embed_metadata_into_attachment(attachment, &json_path) {
            warn!(
                "💣 Unable to embed custom metadata into attachment [{}]: {error:#}",
                attachment.record.original_name
            );
        }

        Ok(())
    }

    fn extract_attachments(
        &self,
        parsed: &ParsedMail<'_>,
        attachments_folder: &Path,
    ) -> Result<Vec<ExtractedAttachment>> {
        let mut attachments = Vec::new();
        self.collect_attachments(parsed, attachments_folder, &mut attachments)?;
        Ok(attachments)
    }

    fn collect_attachments(
        &self,
        part: &ParsedMail<'_>,
        attachments_folder: &Path,
        attachments: &mut Vec<ExtractedAttachment>,
    ) -> Result<()> {
        if part.subparts.is_empty() {
            if let Some(file_name) = attachment_name(part) {
                let safe_name = sanitize_filename(&file_name);
                let final_name = if safe_name.is_empty() {
                    "attachment.bin".to_string()
                } else {
                    safe_name
                };
                match part.get_body_raw() {
                    Ok(bytes) => {
                        if should_skip_embedded_image(part, bytes.len()) {
                            info!("😎 Skip embedded image [{}]", file_name);
                            return Ok(());
                        }

                        fs::create_dir_all(attachments_folder).with_context(|| {
                            format!("unable to create {}", attachments_folder.display())
                        })?;
                        let target_path = make_unique_path(attachments_folder.join(&final_name));
                        if let Err(error) = fs::write(&target_path, &bytes)
                            .with_context(|| format!("unable to write {}", target_path.display()))
                        {
                            warn!(
                                "💣 Unable to persist attachment bytes [{}]: {error:#}",
                                target_path.display()
                            );
                            attachments.push(ExtractedAttachment {
                                path: None,
                                record: self.build_attachment_record(
                                    &final_name,
                                    None,
                                    bytes.len(),
                                    &part.ctype.mimetype,
                                    Some(&bytes),
                                ),
                            });
                            return Ok(());
                        }

                        attachments.push(ExtractedAttachment {
                            path: Some(target_path.clone()),
                            record: self.build_attachment_record(
                                target_path
                                    .file_name()
                                    .and_then(|value| value.to_str())
                                    .unwrap_or(&final_name),
                                Some(&target_path),
                                bytes.len(),
                                &part.ctype.mimetype,
                                Some(&bytes),
                            ),
                        });
                    }
                    Err(error) => {
                        warn!(
                            "💣 Unable to extract attachment body [{}]: {error:#}",
                            final_name
                        );
                        attachments.push(ExtractedAttachment {
                            path: None,
                            record: self.build_attachment_record(
                                &final_name,
                                None,
                                0,
                                &part.ctype.mimetype,
                                None,
                            ),
                        });
                    }
                }
            }
            return Ok(());
        }

        for subpart in &part.subparts {
            self.collect_attachments(subpart, attachments_folder, attachments)?;
        }

        Ok(())
    }

    fn build_attachment_record(
        &self,
        file_name: &str,
        path: Option<&Path>,
        size: usize,
        mime_type: &str,
        bytes: Option<&[u8]>,
    ) -> AttachmentRecord {
        let mut record = AttachmentRecord {
            original_name: file_name.to_string(),
            mime_type: mime_type.to_string(),
            size,
            md5: bytes
                .map(|value| format!("{:x}", md5::compute(value)))
                .unwrap_or_default(),
            extracted_text: None,
            extracted_text_method: None,
            extracted_text_truncated: None,
        };

        let is_pdf = Path::new(file_name)
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.eq_ignore_ascii_case("pdf"))
            .unwrap_or(false);
        if !is_pdf || bytes.is_none() {
            return record;
        }

        let Some(path) = path else {
            return record;
        };

        match extract_pdf_text(path) {
            Ok(Some(extraction)) => {
                info!(
                    "😎 Extract PDF text [{}] via {}",
                    path.display(),
                    extraction.method
                );
                record.extracted_text = Some(extraction.text);
                record.extracted_text_method = Some(extraction.method.to_string());
                record.extracted_text_truncated = Some(extraction.truncated);
            }
            Ok(None) => {
                info!("😎 No PDF text extracted [{}]", path.display());
            }
            Err(error) => {
                warn!(
                    "💣 PDF text extraction failed [{}]: {error:#}",
                    path.display()
                );
            }
        }

        record
    }

    fn embed_metadata_into_attachment(
        &self,
        attachment: &ExtractedAttachment,
        json_path: &Path,
    ) -> Result<()> {
        let Some(path) = attachment.path.as_deref() else {
            return Ok(());
        };

        let payload = fs::read_to_string(json_path)
            .with_context(|| format!("unable to read {}", json_path.display()))?;

        if path.is_file() {
            embed_custom_metadata(path, &payload)
                .with_context(|| format!("unable to inject doka metadata into {}", path.display()))?;
        }

        Ok(())
    }

    fn write_email_xml(
        &self,
        parsed: &ParsedMail<'_>,
        email_output_folder: &Path,
        record: &ParsedEmailRecord,
        json_payload: &str,
        stem: &str,
    ) -> Result<()> {
        let mut xml = String::new();
        xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        xml.push_str("<email-content");
        push_xml_attr(&mut xml, "parsing-folder", &record.parsing_folder);
        push_xml_attr(&mut xml, "email-folder", &record.email_folder);
        push_xml_attr(&mut xml, "email-file-name", &record.email_file_name);
        if let Some(message_id) = &record.message_id {
            push_xml_attr(&mut xml, "message-id", message_id);
        }
        if let Some(expedition_date) = &record.expedition_date {
            push_xml_attr(&mut xml, "expedition-date", expedition_date);
        }
        push_xml_attr(&mut xml, "subject", &record.subject);
        xml.push_str(">\n");

        xml.push_str("  <doka-custom format=\"json\"><![CDATA[");
        xml.push_str(&wrap_cdata(json_payload));
        xml.push_str("]]></doka-custom>\n");
        xml.push_str("  <mime-structure>\n");
        append_part_xml(parsed, "1", 2, &mut xml)?;
        xml.push_str("  </mime-structure>\n");
        xml.push_str("</email-content>\n");

        let xml_path = email_output_folder.join(format!("{}.xml", stem));
        fs::write(&xml_path, xml).with_context(|| format!("unable to write {}", xml_path.display()))
    }

    fn cleanup_failed_email_folder(&self, email_output_folder: &Path) {
        if !email_output_folder.exists() {
            return;
        }

        match fs::remove_dir_all(email_output_folder) {
            Ok(()) => info!(
                "😎 Cleanup failed email folder [{}]",
                email_output_folder.display()
            ),
            Err(error) => warn!(
                "💣 Unable to cleanup failed email folder [{}]: {error:#}",
                email_output_folder.display()
            ),
        }
    }
}

fn attachment_bundle_stem(file_name: &str) -> String {
    Path::new(file_name)
        .file_stem()
        .or_else(|| Path::new(file_name).file_name())
        .and_then(|value| value.to_str())
        .map(sanitize_filename)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "attachment".to_string())
}

fn first_header(parsed: &ParsedMail<'_>, header: &str) -> Option<String> {
    parsed
        .headers
        .get_first_value(header)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_single_contact(raw: Option<String>) -> HeaderContact {
    match raw {
        Some(raw) => parse_address(&raw),
        None => HeaderContact {
            raw: String::new(),
            name: None,
            address: None,
        },
    }
}

fn parse_contact_list(raw: Option<String>) -> Vec<HeaderContact> {
    raw.map(|value| {
        mailparse::addrparse(&value)
            .map(|addresses| {
                addresses
                    .iter()
                    .filter_map(|address| match address {
                        mailparse::MailAddr::Single(info) => Some(HeaderContact {
                            raw: format_address(info.display_name.as_deref(), &info.addr),
                            name: info
                                .display_name
                                .as_deref()
                                .map(normalize_display_name)
                                .filter(|value| !value.is_empty()),
                            address: Some(info.addr.clone()),
                        }),
                        mailparse::MailAddr::Group(group) => {
                            let raw = format!(
                                "{}: {}",
                                group.group_name,
                                group
                                    .addrs
                                    .iter()
                                    .map(|info| {
                                        format_address(info.display_name.as_deref(), &info.addr)
                                    })
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            );
                            Some(HeaderContact {
                                raw,
                                name: Some(group.group_name.clone()),
                                address: None,
                            })
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|_| vec![fallback_contact(&value)])
    })
    .unwrap_or_default()
}

fn parse_address(raw: &str) -> HeaderContact {
    parse_contact_list(Some(raw.to_string()))
        .into_iter()
        .next()
        .unwrap_or(HeaderContact {
            raw: raw.trim().to_string(),
            name: raw
                .trim()
                .split_once('@')
                .map(|_| normalize_display_name(raw.trim()))
                .filter(|value| !value.is_empty()),
            address: raw
                .trim()
                .split_once('@')
                .map(|_| raw.trim().to_lowercase())
                .filter(|value| !value.is_empty()),
        })
}

fn fallback_contact(raw: &str) -> HeaderContact {
    HeaderContact {
        raw: raw.trim().to_string(),
        name: None,
        address: None,
    }
}

fn format_address(display_name: Option<&str>, address: &str) -> String {
    let display_name = display_name
        .map(normalize_display_name)
        .filter(|value| !value.is_empty());
    match display_name {
        Some(display_name) => format!("{} <{}>", display_name, address),
        None => address.to_string(),
    }
}

fn attachment_name(part: &ParsedMail<'_>) -> Option<String> {
    let disposition = part.get_content_disposition();
    disposition
        .params
        .get("filename")
        .cloned()
        .or_else(|| part.ctype.params.get("name").cloned())
        .map(|value| normalize_display_name(value.trim_matches('"')))
        .filter(|value| !value.is_empty())
}

fn should_skip_embedded_image(part: &ParsedMail<'_>, size: usize) -> bool {
    let file_name = attachment_name(part);
    let is_image = part.ctype.mimetype.starts_with("image/");
    let disposition = part.get_content_disposition();
    let is_inline = matches!(disposition.disposition, DispositionType::Inline);
    let has_content_id = part.headers.get_first_value("Content-ID").is_some();
    let is_small = size <= EMBEDDED_IMAGE_MAX_BYTES;
    let looks_like_temp_asset = file_name
        .as_deref()
        .map(looks_like_embedded_temp_file)
        .unwrap_or(false);

    if !is_image && !looks_like_temp_asset {
        return false;
    }

    is_inline || has_content_id || is_small || looks_like_temp_asset
}

fn looks_like_embedded_temp_file(file_name: &str) -> bool {
    let lower = file_name.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return false;
    }

    let has_temp_prefix = lower.starts_with("tmp")
        || lower.starts_with("image")
        || lower.starts_with("part")
        || lower.starts_with("att")
        || lower.starts_with("inline");
    let has_temp_marker = lower.contains(".tmp")
        || lower.contains("unnamed")
        || lower.contains("unknown")
        || lower.contains("noname");
    let has_weird_numeric_suffix = Regex::new(r"\.(tmp|dat|bin)\.\d+$")
        .expect("invalid regex")
        .is_match(&lower);
    let has_embedded_image_name = Regex::new(r"^(tmp[0-9a-f]+|image\d+|part\d+(\.\d+)*|att\d+)")
        .expect("invalid regex")
        .is_match(&lower);

    has_temp_prefix || has_temp_marker || has_weird_numeric_suffix || has_embedded_image_name
}

fn folder_name_or_default(path: &Path, fallback: &str) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(|value| value.to_string())
        .unwrap_or_else(|| fallback.to_string())
}

fn normalize_subject(subject: String) -> String {
    let trimmed = subject.trim();
    trimmed
        .strip_prefix("🔴 ")
        .or_else(|| trimmed.strip_prefix("🔵 "))
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

fn parse_iso_timestamp(raw: Option<String>) -> Option<String> {
    let raw = raw?;
    let timestamp = mailparse::dateparse(&raw).ok()?;
    let datetime = DateTime::<Utc>::from_timestamp(timestamp, 0)?;
    Some(datetime.to_rfc3339())
}

fn append_part_xml(
    part: &ParsedMail<'_>,
    path: &str,
    depth: usize,
    xml: &mut String,
) -> Result<bool> {
    if is_attachment_part(part) {
        return Ok(false);
    }

    if part
        .ctype
        .mimetype
        .eq_ignore_ascii_case("multipart/alternative")
    {
        return append_alternative_part_xml(part, path, depth, xml);
    }

    if part.ctype.mimetype.eq_ignore_ascii_case("message/rfc822") {
        let raw = part.get_body_raw()?;
        let nested = mailparse::parse_mail(&raw)?;
        indent(xml, depth);
        xml.push_str("<part");
        push_xml_attr(xml, "path", path);
        push_xml_attr(xml, "kind", "message");
        push_xml_attr(xml, "mime-type", &part.ctype.mimetype);
        push_common_part_attrs(xml, part);
        xml.push_str(">\n");
        let nested_written = append_part_xml(&nested, &format!("{path}.1"), depth + 1, xml)?;
        if !nested_written {
            indent(xml, depth + 1);
            xml.push_str("<content format=\"raw-message\"><![CDATA[");
            xml.push_str(&wrap_cdata(&String::from_utf8_lossy(&raw)));
            xml.push_str("]]></content>\n");
        }
        indent(xml, depth);
        xml.push_str("</part>\n");
        return Ok(true);
    }

    if !part.subparts.is_empty() {
        let mut children = String::new();
        let mut child_count = 0usize;
        for (index, subpart) in part.subparts.iter().enumerate() {
            if append_part_xml(
                subpart,
                &format!("{path}.{}", index + 1),
                depth + 1,
                &mut children,
            )? {
                child_count += 1;
            }
        }

        if child_count == 0 {
            return Ok(false);
        }

        indent(xml, depth);
        xml.push_str("<part");
        push_xml_attr(xml, "path", path);
        push_xml_attr(xml, "kind", "multipart");
        push_xml_attr(xml, "mime-type", &part.ctype.mimetype);
        push_common_part_attrs(xml, part);
        xml.push_str(">\n");
        xml.push_str(&children);
        indent(xml, depth);
        xml.push_str("</part>\n");
        return Ok(true);
    }

    if is_textual_content_part(part) {
        let content = part.get_body()?;
        indent(xml, depth);
        xml.push_str("<part");
        push_xml_attr(xml, "path", path);
        push_xml_attr(xml, "kind", "body");
        push_xml_attr(xml, "mime-type", &part.ctype.mimetype);
        push_common_part_attrs(xml, part);
        xml.push_str(">\n");
        indent(xml, depth + 1);
        let (format, content) = if part.ctype.mimetype.eq_ignore_ascii_case("text/html") {
            ("markdown", html_to_markdown(&content))
        } else {
            ("plain", content)
        };
        xml.push_str("<content");
        push_xml_attr(xml, "format", format);
        if part.ctype.mimetype.eq_ignore_ascii_case("text/html") {
            push_xml_attr(xml, "source-format", "html");
        }
        xml.push_str("><![CDATA[");
        xml.push_str(&wrap_cdata(&content));
        xml.push_str("]]></content>\n");
        indent(xml, depth);
        xml.push_str("</part>\n");
        return Ok(true);
    }

    Ok(false)
}

fn append_alternative_part_xml(
    part: &ParsedMail<'_>,
    path: &str,
    depth: usize,
    xml: &mut String,
) -> Result<bool> {
    if let Some((index, plain_part)) = part.subparts.iter().enumerate().find(|(_, subpart)| {
        !is_attachment_part(subpart) && subpart.ctype.mimetype.eq_ignore_ascii_case("text/plain")
    }) {
        return append_part_xml(plain_part, &format!("{path}.{}", index + 1), depth, xml);
    }

    if let Some((index, html_part)) = part.subparts.iter().enumerate().find(|(_, subpart)| {
        !is_attachment_part(subpart) && subpart.ctype.mimetype.eq_ignore_ascii_case("text/html")
    }) {
        return append_part_xml(html_part, &format!("{path}.{}", index + 1), depth, xml);
    }

    for (index, subpart) in part.subparts.iter().enumerate() {
        if append_part_xml(subpart, &format!("{path}.{}", index + 1), depth, xml)? {
            return Ok(true);
        }
    }

    Ok(false)
}

fn is_attachment_part(part: &ParsedMail<'_>) -> bool {
    let disposition = part.get_content_disposition();
    if matches!(disposition.disposition, DispositionType::Attachment) {
        return true;
    }

    attachment_name(part).is_some() && !is_textual_content_part(part)
}

fn is_textual_content_part(part: &ParsedMail<'_>) -> bool {
    part.ctype.mimetype.eq_ignore_ascii_case("text/plain")
        || part.ctype.mimetype.eq_ignore_ascii_case("text/html")
}

fn push_common_part_attrs(xml: &mut String, part: &ParsedMail<'_>) {
    if let Some(charset) = part.ctype.params.get("charset") {
        push_xml_attr(xml, "charset", charset);
    }
    if let Some(boundary) = part.ctype.params.get("boundary") {
        push_xml_attr(xml, "boundary", boundary);
    }
    if let Some(transfer_encoding) = part.headers.get_first_value("Content-Transfer-Encoding") {
        push_xml_attr(xml, "transfer-encoding", transfer_encoding.trim());
    }
    let disposition = part.get_content_disposition();
    match disposition.disposition {
        DispositionType::Attachment => push_xml_attr(xml, "disposition", "attachment"),
        DispositionType::Inline => push_xml_attr(xml, "disposition", "inline"),
        DispositionType::FormData => push_xml_attr(xml, "disposition", "form-data"),
        DispositionType::Extension(ref value) => push_xml_attr(xml, "disposition", value),
    }
    if let Some(content_id) = part.headers.get_first_value("Content-ID") {
        push_xml_attr(xml, "content-id", content_id.trim());
    }
}

fn push_xml_attr(xml: &mut String, name: &str, value: &str) {
    xml.push(' ');
    xml.push_str(name);
    xml.push_str("=\"");
    xml.push_str(&encode_double_quoted_attribute(value));
    xml.push('"');
}

fn indent(xml: &mut String, depth: usize) {
    for _ in 0..depth {
        xml.push_str("  ");
    }
}

fn wrap_cdata(value: &str) -> String {
    value.replace("]]>", "]]]]><![CDATA[>")
}

fn html_to_markdown(value: &str) -> String {
    let truncated = if value.len() > HTML_TO_MARKDOWN_MAX_CHARS {
        &value[..HTML_TO_MARKDOWN_MAX_CHARS]
    } else {
        value
    };

    let without_noise = strip_html_blocks(truncated);
    let with_line_breaks = Regex::new(r"(?is)<\s*br\s*/?\s*>")
        .expect("invalid regex")
        .replace_all(&without_noise, "\n");
    let with_block_breaks =
        Regex::new(r"(?is)</\s*(p|div|section|article|tr|table|li|ul|ol|h[1-6])\s*>")
            .expect("invalid regex")
            .replace_all(&with_line_breaks, "\n\n");
    let with_bullets = Regex::new(r"(?is)<\s*li\b[^>]*>")
        .expect("invalid regex")
        .replace_all(&with_block_breaks, "- ");
    let with_links =
        Regex::new(r#"(?is)<\s*a\b[^>]*href\s*=\s*["']([^"']+)["'][^>]*>(.*?)</\s*a\s*>"#)
            .expect("invalid regex")
            .replace_all(&with_bullets, "[$2]($1)");
    let without_tags = Regex::new(r"(?is)<[^>]+>")
        .expect("invalid regex")
        .replace_all(&with_links, " ");

    let decoded = decode_html_entities(&without_tags).into_owned();
    let normalized_newlines = Regex::new(r"\r\n?")
        .expect("invalid regex")
        .replace_all(&decoded, "\n");
    let collapsed_spaces = Regex::new(r"[ \t]+")
        .expect("invalid regex")
        .replace_all(&normalized_newlines, " ");
    let collapsed_blank_lines = Regex::new(r"\n{3,}")
        .expect("invalid regex")
        .replace_all(&collapsed_spaces, "\n\n");
    let mut markdown = collapsed_blank_lines.trim().to_string();

    if value.len() > HTML_TO_MARKDOWN_MAX_CHARS {
        markdown.push_str("\n\n[message truncated during HTML to Markdown conversion]");
    }

    markdown
}

fn strip_html_blocks(value: &str) -> String {
    let mut cleaned = value.to_string();
    for tag in ["script", "style", "head", "title", "svg", "noscript"] {
        let pattern = format!(r"(?is)<\s*{tag}\b[^>]*>.*?</\s*{tag}\s*>");
        cleaned = Regex::new(&pattern)
            .expect("invalid regex")
            .replace_all(&cleaned, " ")
            .into_owned();
    }

    cleaned = Regex::new(r"(?is)<\s*meta\b[^>]*>")
        .expect("invalid regex")
        .replace_all(&cleaned, " ")
        .into_owned();

    Regex::new(r"(?is)<!--.*?-->")
        .expect("invalid regex")
        .replace_all(&cleaned, " ")
        .into_owned()
}
