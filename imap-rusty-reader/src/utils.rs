use anyhow::Result;
use html_escape::decode_html_entities;
use quoted_printable::ParseMode;
use regex::Regex;

#[derive(Debug, Clone, Copy)]
pub enum EmailSep {
    Brackets,
    Parenthesis,
}

#[derive(Debug, Clone, Default)]
pub struct ContactInfo {
    pub casual: String,
    pub email: String,
}

/// Applies the same filename sanitation rules as the Java code: HTML entity\n/// decoding, removal of invisible characters, and stripping of disallowed symbols.
pub fn sanitize_filename(name: &str) -> String {
    let decoded = decode_html_entities(name).to_string();
    let without_invisible = decoded.replace(['\r', '\n', '\t'], "");
    let invalid = Regex::new(r#"[:\/*?|!#$%^<>"]"#).expect("invalid regex");
    invalid
        .replace_all(&without_invisible, "")
        .trim()
        .to_string()
}

/// Extracts a casual display name and a normalized email address from a contact\n/// string such as `Name <mail@host>` or `Name (mail@host)`.
pub fn extract_contact_info(contact: &str, sep: EmailSep) -> ContactInfo {
    let (sep1, sep2) = match sep {
        EmailSep::Brackets => ('<', '>'),
        EmailSep::Parenthesis => ('(', ')'),
    };

    let pos = contact.rfind(sep1);
    let pos2 = contact.rfind(sep2);
    let casual = pos
        .map(|index| contact[..index].trim().to_string())
        .unwrap_or_default();
    let email = match (pos, pos2) {
        (Some(start), Some(end)) if start < end => contact[start + 1..end].trim().to_lowercase(),
        _ => contact.trim().to_lowercase(),
    };

    ContactInfo {
        casual: normalize_display_name(&casual),
        email,
    }
}

/// Normalizes a display name by trimming quotes, decoding broken Q-encoding,\n/// and repairing common mojibake patterns.
pub fn normalize_display_name(name: &str) -> String {
    let trimmed = name.trim().trim_matches('\'').trim_matches('"').trim();
    repair_mojibake(&decode_broken_q_encoding(trimmed).unwrap_or_else(|_| trimmed.to_string()))
        .trim()
        .to_string()
}

/// Repairs pseudo-RFC2047 strings such as `=utf-8Q...=` that appear in the\n/// historical dataset but are not strictly valid MIME encoded words.
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

/// Tries to recover strings where UTF-8 bytes were interpreted as Latin-1,\n/// producing patterns like `gÃ©nÃ©rale`.
pub fn repair_mojibake(value: &str) -> String {
    if value.is_empty() || (!value.contains('Ã') && !value.contains('Â')) {
        return value.to_string();
    }

    match String::from_utf8(value.as_bytes().iter().map(|b| *b).collect::<Vec<_>>()) {
        Ok(_) => {}
        Err(_) => {}
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

#[cfg(test)]
mod tests {
    use super::{normalize_display_name, sanitize_filename};

    #[test]
    /// Verifies that the Rust sanitation keeps the same output shape as the\n    /// reference Java implementation for forbidden filename characters.
    fn sanitizes_like_java() {
        assert_eq!(sanitize_filename(" A:B/C? "), "ABC");
    }

    #[test]
    /// Verifies that the mojibake repair heuristic fixes a common broken French\n    /// UTF-8 example.
    fn repairs_mojibake() {
        assert_eq!(
            normalize_display_name("Direction gÃ©nÃ©rale"),
            "Direction générale"
        );
    }

    #[test]
    /// Verifies that a broken Q-encoded contact name is normalized into readable\n    /// Unicode before folder creation.
    fn decodes_broken_q_encoding() {
        assert_eq!(
            normalize_display_name("=utf-8QChlo=C3=A9_SAUNIER="),
            "Chloé SAUNIER"
        );
    }
}
