use std::path::{Path, PathBuf};

use anyhow::Result;
use html_escape::decode_html_entities;
use quoted_printable::ParseMode;
use regex::Regex;

pub fn sanitize_filename(name: &str) -> String {
    let decoded = decode_html_entities(name).to_string();
    let without_invisible = decoded.replace(['\r', '\n', '\t'], "");
    let invalid = Regex::new(r#"[:\/*?|!#$%^<>"]"#).expect("invalid regex");
    invalid
        .replace_all(&without_invisible, "")
        .trim()
        .to_string()
}

pub fn normalize_display_name(name: &str) -> String {
    let trimmed = name.trim().trim_matches('\'').trim_matches('"').trim();
    repair_mojibake(&decode_broken_q_encoding(trimmed).unwrap_or_else(|_| trimmed.to_string()))
        .trim()
        .to_string()
}

fn decode_broken_q_encoding(value: &str) -> Result<String> {
    let regex = Regex::new(r"^=(?P<charset>[^?=]+)(?P<encoding>[QqBb])(?P<payload>.*)=$")?;
    let Some(captures) = regex.captures(value) else {
        return Ok(value.to_string());
    };

    let charset = captures
        .name("charset")
        .map(|m| m.as_str())
        .unwrap_or("utf-8");
    let encoding = captures.name("encoding").map(|m| m.as_str()).unwrap_or("Q");
    let payload = captures.name("payload").map(|m| m.as_str()).unwrap_or("");
    if !encoding.eq_ignore_ascii_case("q") {
        return Ok(value.to_string());
    }

    let raw = payload.replace('_', " ");
    let bytes = quoted_printable::decode(raw.as_bytes(), ParseMode::Robust)?;
    let decoded = match charset.to_ascii_lowercase().as_str() {
        "utf-8" | "utf8" => String::from_utf8_lossy(&bytes).into_owned(),
        _ => String::from_utf8_lossy(&bytes).into_owned(),
    };
    Ok(decoded)
}

pub fn repair_mojibake(value: &str) -> String {
    if value.is_empty() || (!value.contains('Ã') && !value.contains('Â')) {
        return value.to_string();
    }

    let repaired = String::from_utf8_lossy(
        &value
            .chars()
            .map(|ch| ch as u32)
            .filter_map(|code| u8::try_from(code).ok())
            .collect::<Vec<_>>(),
    )
    .to_string();

    if repaired.trim().is_empty() {
        value.to_string()
    } else {
        repaired
    }
}

pub fn file_stem_or_name(path: &Path) -> String {
    path.file_stem()
        .or_else(|| path.file_name())
        .and_then(|value| value.to_str())
        .map(sanitize_filename)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "email".to_string())
}

pub fn make_unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }

    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("file")
        .to_string();
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_string());

    let mut index = 1;
    loop {
        let candidate_name = match &extension {
            Some(extension) => format!("{}_{}.{}", stem, index, extension),
            None => format!("{}_{}", stem, index),
        };
        let candidate = parent.join(candidate_name);
        if !candidate.exists() {
            return candidate;
        }
        index += 1;
    }
}
