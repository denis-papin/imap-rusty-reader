use anyhow::{Result, bail};

/// Resolve a complete YAML credential value after YAML decoding.
pub fn resolve_secret(value: &str) -> Result<String> {
    resolve_with(value, |name| std::env::var(name).ok())
}

fn resolve_with(value: &str, lookup: impl FnOnce(&str) -> Option<String>) -> Result<String> {
    let Some(name) = value
        .strip_prefix("=${")
        .or_else(|| value.strip_prefix("${"))
        .and_then(|rest| rest.strip_suffix('}'))
    else {
        return Ok(value.to_owned());
    };

    if name.is_empty()
        || !name
            .bytes()
            .enumerate()
            .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()))
    {
        bail!("invalid environment variable name in credential reference");
    }

    match lookup(name) {
        Some(secret) if !secret.is_empty() => Ok(secret),
        _ => bail!("environment variable {name} is missing, empty or not valid UTF-8"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_once_and_preserves_special_characters() {
        for value in ["=${MAIL_2}", "${MAIL_2}"] {
            let secret = " # : ' \n${OTHER} ";
            assert_eq!(
                resolve_with(value, |name| {
                    assert_eq!(name, "MAIL_2");
                    Some(secret.to_owned())
                })
                .unwrap(),
                secret
            );
        }
        assert_eq!(
            resolve_with("abc${KEY}", |_| unreachable!()).unwrap(),
            "abc${KEY}"
        );
    }

    #[test]
    fn reports_unavailable_references() {
        for result in [None, Some(String::new())] {
            assert!(
                resolve_with("=${MAIL}", |_| result)
                    .unwrap_err()
                    .to_string()
                    .contains("MAIL")
            );
        }
        assert!(resolve_with("=${BAD-NAME}", |_| unreachable!()).is_err());
    }
}
