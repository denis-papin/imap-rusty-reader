use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct AgentsSpec {
    pub instructions: String,
    pub output_schema: Value,
    pub folder_taxonomy: BTreeMap<String, Vec<String>>,
    pub cache_fingerprint: String,
}

pub fn load_agents_spec(path: &Path) -> Result<AgentsSpec> {
    let raw_markdown = fs::read_to_string(path)
        .with_context(|| format!("unable to read AGENTS file {}", path.display()))?;
    let folder_taxonomy = parse_folder_taxonomy(&raw_markdown)?;
    let parsed_schema = extract_json_block_for_heading(
        &raw_markdown,
        &["Output JSON Schema", "JSON Schema", "Output Schema"],
    )?;
    let output_schema = match parsed_schema {
        Some(value) => inject_folder_enums_and_harden(value, &folder_taxonomy),
        None => default_output_schema(&folder_taxonomy),
    };

    Ok(AgentsSpec {
        instructions: build_compact_instructions(&raw_markdown, &folder_taxonomy),
        output_schema,
        folder_taxonomy,
        cache_fingerprint: compute_agents_fingerprint(&raw_markdown),
    })
}

fn parse_folder_taxonomy(markdown: &str) -> Result<BTreeMap<String, Vec<String>>> {
    let value = extract_json_block_for_heading(
        markdown,
        &[
            "Folder Taxonomy",
            "Classification Taxonomy",
            "Allowed Folder Taxonomy",
        ],
    )?
    .ok_or_else(|| {
        anyhow!("AGENTS.md must define a `Folder Taxonomy` section containing a JSON object")
    })?;

    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("Folder Taxonomy JSON block must be an object"))?;
    let mut taxonomy = BTreeMap::new();
    for (main_folder, subfolders) in object {
        let items = subfolders
            .as_array()
            .ok_or_else(|| anyhow!("Folder Taxonomy values must be arrays"))?
            .iter()
            .filter_map(|item| item.as_str().map(|value| value.trim().to_string()))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        if items.is_empty() {
            bail!("Folder Taxonomy entry `{main_folder}` is empty");
        }
        taxonomy.insert(main_folder.trim().to_string(), items);
    }

    if taxonomy.is_empty() {
        bail!("Folder Taxonomy is empty");
    }

    Ok(taxonomy)
}

fn extract_json_block_for_heading(markdown: &str, headings: &[&str]) -> Result<Option<Value>> {
    let Some(section) = extract_markdown_section(markdown, headings) else {
        return Ok(None);
    };

    let regex = Regex::new(r"(?s)```(?:json)?\s*(\{.*?\}|\[.*?\])\s*```").expect("invalid regex");
    let Some(captures) = regex.captures(&section) else {
        return Ok(None);
    };
    let payload = captures
        .get(1)
        .map(|value| value.as_str())
        .unwrap_or_default();
    let value = serde_json::from_str::<Value>(payload)
        .with_context(|| "unable to parse JSON block from AGENTS.md")?;
    Ok(Some(value))
}

fn extract_markdown_section(markdown: &str, headings: &[&str]) -> Option<String> {
    let normalized = markdown.replace("\r\n", "\n");
    let mut lines = normalized.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        let Some(heading_name) = trimmed.strip_prefix("##") else {
            continue;
        };
        let heading_name = heading_name.trim_start_matches('#').trim();
        if !headings.iter().any(|candidate| *candidate == heading_name) {
            continue;
        }

        let mut section = Vec::new();
        for next_line in lines.by_ref() {
            if next_line.trim().starts_with("##") {
                break;
            }
            section.push(next_line);
        }
        return Some(section.join("\n").trim().to_string());
    }
    None
}

fn default_output_schema(folder_taxonomy: &BTreeMap<String, Vec<String>>) -> Value {
    let main_folders = folder_taxonomy.keys().cloned().collect::<Vec<_>>();
    let sub_folders = all_subfolders(folder_taxonomy);
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["email_summary", "main_folder", "sub_folder", "attachment_summaries"],
        "properties": {
            "email_summary": {
                "type": "string",
                "description": "A concise summary of the email in a few words."
            },
            "main_folder": {
                "type": "string",
                "enum": main_folders,
                "description": "First-level filing folder."
            },
            "sub_folder": {
                "type": "string",
                "enum": sub_folders,
                "description": "Second-level filing folder valid for the selected main folder."
            },
            "attachment_summaries": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["file_name", "mime_type", "summary", "confidence"],
                    "properties": {
                        "file_name": { "type": "string" },
                        "mime_type": { "type": ["string", "null"] },
                        "summary": { "type": "string" },
                        "confidence": { "type": ["number", "null"] }
                    }
                }
            }
        }
    })
}

fn inject_folder_enums_and_harden(
    mut schema: Value,
    folder_taxonomy: &BTreeMap<String, Vec<String>>,
) -> Value {
    harden_objects(&mut schema);
    let main_folders = folder_taxonomy.keys().cloned().collect::<Vec<_>>();
    let sub_folders = all_subfolders(folder_taxonomy);

    if let Some(main_folder) = schema.pointer_mut("/properties/main_folder") {
        if let Some(items) = main_folder.as_object_mut() {
            items.insert("enum".to_string(), json!(main_folders));
        }
    }
    if let Some(sub_folder) = schema.pointer_mut("/properties/sub_folder") {
        if let Some(items) = sub_folder.as_object_mut() {
            items.insert("enum".to_string(), json!(sub_folders));
        }
    }
    schema
}

fn all_subfolders(folder_taxonomy: &BTreeMap<String, Vec<String>>) -> Vec<String> {
    folder_taxonomy
        .values()
        .flat_map(|items| items.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn build_compact_instructions(
    markdown: &str,
    folder_taxonomy: &BTreeMap<String, Vec<String>>,
) -> String {
    let mut sections = Vec::new();
    for heading in [
        "Goal",
        "Output Rules",
        "Summary Rules",
        "Attachment Summary Rules",
        "Classification Rules",
        "Failure And Uncertainty Rules",
    ] {
        if let Some(section) = extract_markdown_section(markdown, &[heading]) {
            sections.push(format!("## {heading}\n{section}"));
        }
    }

    sections.push(format!(
        "## Folder Taxonomy\n```json\n{}\n```",
        serde_json::to_string_pretty(folder_taxonomy).unwrap_or_else(|_| "{}".to_string())
    ));
    sections.join("\n\n")
}

fn compute_agents_fingerprint(markdown: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(markdown.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn harden_objects(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let is_object_schema = map
                .get("type")
                .map(schema_type_contains_object)
                .unwrap_or(false);
            if is_object_schema && map.contains_key("properties") {
                if !map.contains_key("additionalProperties") {
                    map.insert("additionalProperties".to_string(), Value::Bool(false));
                }

                let original_required = map
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(ToString::to_string)
                            .collect::<std::collections::HashSet<_>>()
                    })
                    .unwrap_or_default();

                if let Some(properties) = map.get_mut("properties").and_then(Value::as_object_mut) {
                    let property_names = properties.keys().cloned().collect::<Vec<_>>();
                    for name in &property_names {
                        if !original_required.contains(name) {
                            if let Some(property_schema) = properties.get_mut(name) {
                                make_property_nullable(property_schema);
                            }
                        }
                    }

                    map.insert(
                        "required".to_string(),
                        Value::Array(property_names.into_iter().map(Value::String).collect()),
                    );
                }
            }

            for child in map.values_mut() {
                harden_objects(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                harden_objects(item);
            }
        }
        _ => {}
    }
}

fn schema_type_contains_object(value: &Value) -> bool {
    match value {
        Value::String(kind) => kind == "object",
        Value::Array(items) => items.iter().any(|item| item.as_str() == Some("object")),
        _ => false,
    }
}

fn make_property_nullable(value: &mut Value) {
    let Some(map) = value.as_object_mut() else {
        return;
    };

    if let Some(type_value) = map.get_mut("type") {
        match type_value {
            Value::String(current) => {
                if current != "null" {
                    *type_value = Value::Array(vec![
                        Value::String(current.clone()),
                        Value::String("null".to_string()),
                    ]);
                }
            }
            Value::Array(items) => {
                let has_null = items.iter().any(|item| item.as_str() == Some("null"));
                if !has_null {
                    items.push(Value::String("null".to_string()));
                }
            }
            _ => {}
        }
        return;
    }

    if let Some(any_of) = map.get_mut("anyOf").and_then(Value::as_array_mut) {
        let has_null = any_of
            .iter()
            .any(|item| item.get("type").and_then(Value::as_str) == Some("null"));
        if !has_null {
            any_of.push(json!({ "type": "null" }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tags_from_json_block() {
        let markdown = r#"
## Folder Taxonomy
```json
{"DENIS":["LEGAL","FACTURES"]}
```
"#;
        let taxonomy = parse_folder_taxonomy(markdown).unwrap();
        assert_eq!(taxonomy.get("DENIS").unwrap(), &vec!["LEGAL", "FACTURES"]);
    }

    #[test]
    fn injects_folder_enums_into_schema() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "main_folder": { "type": "string" },
                "sub_folder": { "type": "string" }
            }
        });

        let taxonomy = BTreeMap::from([
            (
                "DENIS".to_string(),
                vec!["LEGAL".to_string(), "FACTURES".to_string()],
            ),
            ("TRAVAIL".to_string(), vec!["LEGAL".to_string()]),
        ]);
        let hardened = inject_folder_enums_and_harden(schema, &taxonomy);
        assert_eq!(
            hardened.pointer("/properties/main_folder/enum").unwrap(),
            &serde_json::json!(["DENIS", "TRAVAIL"])
        );
        assert_eq!(
            hardened.get("additionalProperties").unwrap(),
            &Value::Bool(false)
        );
        assert_eq!(
            hardened.get("required").unwrap(),
            &serde_json::json!(["main_folder", "sub_folder"])
        );
    }

    #[test]
    fn makes_non_required_properties_nullable_and_required() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "attachment": {
                    "type": "object",
                    "required": ["file_name"],
                    "properties": {
                        "file_name": { "type": "string" },
                        "confidence": { "type": "number" }
                    }
                }
            }
        });

        let taxonomy = BTreeMap::from([(
            "DENIS".to_string(),
            vec!["LEGAL".to_string(), "FACTURES".to_string()],
        )]);
        let hardened = inject_folder_enums_and_harden(schema, &taxonomy);
        assert_eq!(
            hardened.pointer("/properties/attachment/required").unwrap(),
            &serde_json::json!(["confidence", "file_name"])
        );
        assert_eq!(
            hardened
                .pointer("/properties/attachment/properties/confidence/type")
                .unwrap(),
            &serde_json::json!(["number", "null"])
        );
    }
}
