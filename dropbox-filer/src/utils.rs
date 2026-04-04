use std::path::Path;

use anyhow::{Result, anyhow, bail};
use html_escape::decode_html_entities;
use regex::Regex;

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

pub fn ensure_original_extension(proposed_file_name: &str, original_file_name: &str) -> Result<String> {
    let sanitized = sanitize_filename(proposed_file_name);
    let original_extension = Path::new(original_file_name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.trim())
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

pub fn build_dropbox_file_path(
    root: &str,
    main_folder: &str,
    sub_folder: &str,
    year_folder: &str,
    file_name: &str,
) -> Result<String> {
    let root = normalize_dropbox_root(root)?;
    let main_folder = sanitize_path_segment(main_folder)?;
    let sub_folder = sanitize_path_segment(sub_folder)?;
    let year_folder = sanitize_path_segment(year_folder)?;
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
    segments.push(main_folder);
    segments.push(sub_folder);
    segments.push(year_folder);
    segments.push(file_name);

    Ok(format!("/{}", segments.join("/")))
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
        build_dropbox_file_path, ensure_original_extension, folder_prefixes, normalize_dropbox_root,
    };

    #[test]
    fn normalizes_root_folder() {
        assert_eq!(normalize_dropbox_root(" Mes docs/Impots ").unwrap(), "/Mes docs/Impots");
    }

    #[test]
    fn keeps_original_extension_when_missing() {
        assert_eq!(
            ensure_original_extension("2024-03-15 techvalley fin-contrat", "courrier.pdf").unwrap(),
            "2024-03-15 techvalley fin-contrat.pdf"
        );
    }

    #[test]
    fn replaces_wrong_extension_with_original_one() {
        assert_eq!(
            ensure_original_extension("2024-03-15 techvalley fin-contrat.txt", "courrier.pdf")
                .unwrap(),
            "2024-03-15 techvalley fin-contrat.pdf"
        );
    }

    #[test]
    fn builds_dropbox_path_from_root_and_tags() {
        assert_eq!(
            build_dropbox_file_path("/Archives", "DENIS", "IMPOTS", "2025", "avis.pdf").unwrap(),
            "/Archives/DENIS/IMPOTS/2025/avis.pdf"
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
}
