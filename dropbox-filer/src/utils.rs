use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use html_escape::decode_html_entities;
use regex::Regex;
use sha2::{Digest, Sha256};

const DROPBOX_CONTENT_HASH_BLOCK_BYTES: usize = 4 * 1024 * 1024;

pub fn sanitize_filename(name: &str) -> String {
    let decoded = decode_html_entities(name).to_string();
    let without_invisible = decoded.replace(['\r', '\n', '\t'], "");
    let invalid = Regex::new(r#"[:\\/*?|!#$%^<>"]"#).expect("invalid regex");
    invalid
        .replace_all(&without_invisible, "")
        .trim()
        .to_string()
}

pub fn sanitize_path_segment(name: &str) -> Result<String> {
    let sanitized = sanitize_filename(name);
    if sanitized.is_empty() {
        bail!("path segment is empty after sanitation");
    }
    Ok(sanitized)
}

pub fn normalize_dropbox_root(root: &str) -> Result<String> {
    let trimmed = root.trim();
    if trimmed.is_empty() {
        bail!("dropbox root folder cannot be empty");
    }
    if trimmed == "/" {
        return Ok("/".to_string());
    }

    let segments = trimmed
        .split('/')
        .filter(|segment| !segment.trim().is_empty())
        .map(sanitize_path_segment)
        .collect::<Result<Vec<_>>>()?;
    if segments.is_empty() {
        bail!("dropbox root folder cannot be empty");
    }

    Ok(format!("/{}", segments.join("/")))
}

pub fn build_dropbox_file_path(root: &str, file_name: &str) -> Result<String> {
    let root = normalize_dropbox_root(root)?;
    let waiting_folder = sanitize_path_segment("A_TRAITER")?;
    let file_name = sanitize_path_segment(file_name)?;

    let mut segments = Vec::new();
    if root != "/" {
        segments.extend(
            root.trim_start_matches('/')
                .split('/')
                .filter(|segment| !segment.is_empty())
                .map(str::to_string),
        );
    }
    segments.push(waiting_folder);
    segments.push(file_name);

    Ok(format!("/{}", segments.join("/")))
}

pub fn ensure_original_extension(
    proposed_file_name: &str,
    original_file_name: &str,
) -> Result<String> {
    let sanitized = sanitize_filename(proposed_file_name);
    let original_extension = Path::new(original_file_name)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    if sanitized.is_empty() {
        return fallback_file_name(original_file_name);
    }

    let proposed_path = Path::new(&sanitized);
    let proposed_stem = proposed_path
        .file_stem()
        .or_else(|| proposed_path.file_name())
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("proposed file name is empty after sanitation"))?;

    match original_extension {
        Some(extension) => Ok(format!("{proposed_stem}.{extension}")),
        None => Ok(sanitized),
    }
}

pub fn parent_dropbox_folder(path: &str) -> Result<String> {
    let normalized = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let Some((parent, _)) = normalized.rsplit_once('/') else {
        bail!("invalid dropbox path `{normalized}`");
    };
    if parent.is_empty() {
        Ok("/".to_string())
    } else {
        Ok(parent.to_string())
    }
}

pub fn folder_prefixes(path: &str) -> Result<Vec<String>> {
    let normalized = normalize_dropbox_root(path)?;
    if normalized == "/" {
        return Ok(Vec::new());
    }

    let mut current = String::new();
    let mut prefixes = Vec::new();
    for segment in normalized.trim_start_matches('/').split('/') {
        current.push('/');
        current.push_str(segment);
        prefixes.push(current.clone());
    }
    Ok(prefixes)
}

pub fn escape_non_ascii_json(raw_json: &str) -> String {
    let mut escaped = String::with_capacity(raw_json.len());
    for ch in raw_json.chars() {
        if ch.is_ascii() {
            escaped.push(ch);
            continue;
        }

        let mut utf16 = [0u16; 2];
        for code_unit in ch.encode_utf16(&mut utf16) {
            escaped.push_str(&format!("\\u{code_unit:04x}"));
        }
    }
    escaped
}

pub fn compute_dropbox_content_hash(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("unable to open {}", path.display()))?;
    let mut buffer = vec![0u8; DROPBOX_CONTENT_HASH_BLOCK_BYTES];
    let mut block_digests = Vec::new();

    loop {
        let bytes_read = file
            .read(&mut buffer)
            .with_context(|| format!("unable to read {}", path.display()))?;
        if bytes_read == 0 {
            break;
        }

        let digest = Sha256::digest(&buffer[..bytes_read]);
        block_digests.extend_from_slice(&digest);
    }

    Ok(hex_sha256(&block_digests))
}

pub fn compute_dropbox_content_hash_bytes(bytes: &[u8]) -> String {
    let mut block_digests = Vec::new();
    for chunk in bytes.chunks(DROPBOX_CONTENT_HASH_BLOCK_BYTES) {
        let digest = Sha256::digest(chunk);
        block_digests.extend_from_slice(&digest);
    }
    hex_sha256(&block_digests)
}

fn hex_sha256(bytes: &[u8]) -> String {
    let final_digest = Sha256::digest(bytes);
    final_digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn fallback_file_name(original_file_name: &str) -> Result<String> {
    let sanitized = sanitize_filename(original_file_name);
    if sanitized.is_empty() {
        Ok("attachment.bin".to_string())
    } else {
        Ok(sanitized)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_dropbox_file_path, compute_dropbox_content_hash_bytes, ensure_original_extension,
        folder_prefixes, normalize_dropbox_root,
    };

    #[test]
    fn normalizes_root_folder() {
        assert_eq!(
            normalize_dropbox_root(" Mes docs/Impots ").unwrap(),
            "/Mes docs/Impots"
        );
    }

    #[test]
    fn builds_dropbox_path_from_root_and_tags() {
        assert_eq!(
            build_dropbox_file_path("/Archives", "avis.pdf").unwrap(),
            "/Archives/A_TRAITER/avis.pdf"
        );
    }

    #[test]
    fn keeps_original_extension_when_missing() {
        assert_eq!(
            ensure_original_extension("2024-03-15 bnpp releve detaille", "source.pdf").unwrap(),
            "2024-03-15 bnpp releve detaille.pdf"
        );
    }

    #[test]
    fn returns_folder_prefixes() {
        assert_eq!(
            folder_prefixes("/Archives/DENIS/IMPOTS").unwrap(),
            vec![
                "/Archives".to_string(),
                "/Archives/DENIS".to_string(),
                "/Archives/DENIS/IMPOTS".to_string()
            ]
        );
    }

    #[test]
    fn computes_dropbox_content_hash_for_bytes() {
        assert_eq!(
            compute_dropbox_content_hash_bytes(b"hello world"),
            "bc62d4b80d9e36da29c16c5d4d9f11731f36052c72401a76c23c0fb5a9b74423"
        );
    }
}
