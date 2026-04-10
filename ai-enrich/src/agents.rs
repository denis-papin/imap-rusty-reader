use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const IMPORTANCE_VALUES: &[&str] = &["HAUTE", "BASSE"];

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
        Some(value) => {
            inject_folder_enums_and_harden(normalize_output_schema(value), &folder_taxonomy)
        }
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
        "required": ["email_summary", "email_importance", "attachment_summaries"],
        "properties": {
            "email_summary": {
                "type": "string",
                "description": "A concise summary of the email in a few words."
            },
            "email_importance": {
                "type": "string",
                "enum": IMPORTANCE_VALUES,
                "description": "Whether the email itself is important over time."
            },
            "attachment_summaries": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["file_name", "mime_type", "summary", "importance", "proposed_file_name", "main_folder", "sub_folder"],
                    "properties": {
                        "file_name": { "type": "string" },
                        "mime_type": { "type": ["string", "null"] },
                        "summary": { "type": "string" },
                        "importance": { "type": "string", "enum": IMPORTANCE_VALUES },
                        "proposed_file_name": {
                            "type": "string",
                            "description": "Suggested attachment filename using the format yyyy-mm-dd <emetteur-short> <motif> while preserving the original extension when known."
                        },
                        "main_folder": {
                            "type": "string",
                            "enum": main_folders,
                            "description": "First-level filing folder for this attachment."
                        },
                        "sub_folder": {
                            "type": "string",
                            "enum": sub_folders,
                            "description": "Second-level filing folder valid for the selected main folder for this attachment."
                        }
                    }
                }
            }
        }
    })
}

fn normalize_output_schema(mut schema: Value) -> Value {
    let top_level_main_folder = schema.pointer("/properties/main_folder").cloned();
    let top_level_sub_folder = schema.pointer("/properties/sub_folder").cloned();

    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        properties.remove("main_folder");
        properties.remove("sub_folder");
    }

    if let Some(required) = schema.get_mut("required").and_then(Value::as_array_mut) {
        required.retain(|value| {
            value.as_str() != Some("main_folder") && value.as_str() != Some("sub_folder")
        });
    }

    let Some(item_properties) = schema
        .pointer_mut("/properties/attachment_summaries/items/properties")
        .and_then(Value::as_object_mut)
    else {
        return schema;
    };

    item_properties.remove("confidence");
    item_properties
        .entry("main_folder".to_string())
        .or_insert_with(|| top_level_main_folder.unwrap_or_else(|| json!({ "type": "string" })));
    item_properties
        .entry("sub_folder".to_string())
        .or_insert_with(|| top_level_sub_folder.unwrap_or_else(|| json!({ "type": "string" })));

    if let Some(required) = schema
        .pointer_mut("/properties/attachment_summaries/items/required")
        .and_then(Value::as_array_mut)
    {
        required.retain(|value| value.as_str() != Some("confidence"));
        ensure_required_field(required, "main_folder");
        ensure_required_field(required, "sub_folder");
    }

    schema
}

fn inject_folder_enums_and_harden(
    mut schema: Value,
    folder_taxonomy: &BTreeMap<String, Vec<String>>,
) -> Value {
    harden_objects(&mut schema);
    let main_folders = folder_taxonomy.keys().cloned().collect::<Vec<_>>();
    let sub_folders = all_subfolders(folder_taxonomy);
    if let Some(email_importance) = schema.pointer_mut("/properties/email_importance") {
        if let Some(items) = email_importance.as_object_mut() {
            items.insert("enum".to_string(), json!(IMPORTANCE_VALUES));
        }
    }
    if let Some(main_folder) =
        schema.pointer_mut("/properties/attachment_summaries/items/properties/main_folder")
    {
        if let Some(items) = main_folder.as_object_mut() {
            items.insert("enum".to_string(), json!(main_folders));
        }
    }
    if let Some(sub_folder) =
        schema.pointer_mut("/properties/attachment_summaries/items/properties/sub_folder")
    {
        if let Some(items) = sub_folder.as_object_mut() {
            items.insert("enum".to_string(), json!(sub_folders));
        }
    }
    if let Some(attachment_importance) =
        schema.pointer_mut("/properties/attachment_summaries/items/properties/importance")
    {
        if let Some(items) = attachment_importance.as_object_mut() {
            items.insert("enum".to_string(), json!(IMPORTANCE_VALUES));
        }
    }
    inject_attachment_pair_constraints(&mut schema, folder_taxonomy);
    schema
}

fn inject_attachment_pair_constraints(
    schema: &mut Value,
    folder_taxonomy: &BTreeMap<String, Vec<String>>,
) {
    let Some(item_schema) = schema
        .pointer("/properties/attachment_summaries/items")
        .cloned()
    else {
        return;
    };
    let item_properties = item_schema
        .get("properties")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let item_required = item_schema
        .get("required")
        .cloned()
        .unwrap_or_else(|| json!([]));

    let constraints = folder_taxonomy
        .iter()
        .map(|(main_folder, subfolders)| {
            let mut branch_properties = item_properties.clone();
            if let Some(properties) = branch_properties.as_object_mut() {
                properties.insert("main_folder".to_string(), json!({ "enum": [main_folder] }));
                properties.insert("sub_folder".to_string(), json!({ "enum": subfolders }));
            }

            json!({
                "type": "object",
                "additionalProperties": false,
                "required": item_required,
                "properties": branch_properties
            })
        })
        .collect::<Vec<_>>();

    if let Some(item_schema) = schema.pointer_mut("/properties/attachment_summaries/items")
        && let Some(item_object) = item_schema.as_object_mut()
    {
        item_object.insert("anyOf".to_string(), Value::Array(constraints));
        item_object.remove("allOf");
    }
}

fn ensure_required_field(required: &mut Vec<Value>, field: &str) {
    if !required.iter().any(|value| value.as_str() == Some(field)) {
        required.push(Value::String(field.to_string()));
    }
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
        "Importance Rules",
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
                "email_importance": { "type": "string" },
                "attachment_summaries": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "main_folder": { "type": "string" },
                            "sub_folder": { "type": "string" }
                        }
                    }
                }
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
            hardened
                .pointer("/properties/email_importance/enum")
                .unwrap(),
            &serde_json::json!(["HAUTE", "BASSE"])
        );
        assert_eq!(
            hardened
                .pointer("/properties/attachment_summaries/items/properties/main_folder/enum")
                .unwrap(),
            &serde_json::json!(["DENIS", "TRAVAIL"])
        );
        assert_eq!(
            hardened.get("additionalProperties").unwrap(),
            &Value::Bool(false)
        );
        let required = hardened.get("required").and_then(Value::as_array).unwrap();
        assert_eq!(required.len(), 2);
        assert!(
            required
                .iter()
                .any(|value| value.as_str() == Some("attachment_summaries"))
        );
        assert!(
            required
                .iter()
                .any(|value| value.as_str() == Some("email_importance"))
        );
    }

    #[test]
    fn migrates_legacy_top_level_classification_to_attachment_level() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["main_folder", "sub_folder", "attachment_summaries"],
            "properties": {
                "main_folder": {
                    "type": "string"
                },
                "sub_folder": {
                    "type": "string"
                },
                "attachment_summaries": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["file_name", "confidence"],
                        "properties": {
                            "file_name": { "type": "string" },
                            "confidence": { "type": "number" }
                        }
                    }
                }
            }
        });

        let normalized = normalize_output_schema(schema);
        assert!(normalized.pointer("/properties/main_folder").is_none());
        assert!(normalized.pointer("/properties/sub_folder").is_none());
        assert!(
            normalized
                .pointer("/properties/attachment_summaries/items/properties/confidence")
                .is_none()
        );
        let required = normalized
            .pointer("/properties/attachment_summaries/items/required")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(required.len(), 3);
        assert!(
            required
                .iter()
                .any(|value| value.as_str() == Some("file_name"))
        );
        assert!(
            required
                .iter()
                .any(|value| value.as_str() == Some("main_folder"))
        );
        assert!(
            required
                .iter()
                .any(|value| value.as_str() == Some("sub_folder"))
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
                        "main_folder": { "type": "string" }
                    }
                }
            }
        });

        let taxonomy = BTreeMap::from([(
            "DENIS".to_string(),
            vec!["LEGAL".to_string(), "FACTURES".to_string()],
        )]);
        let hardened = inject_folder_enums_and_harden(schema, &taxonomy);
        let required = hardened
            .pointer("/properties/attachment/required")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(required.len(), 2);
        assert!(
            required
                .iter()
                .any(|value| value.as_str() == Some("file_name"))
        );
        assert!(
            required
                .iter()
                .any(|value| value.as_str() == Some("main_folder"))
        );
        assert_eq!(
            hardened
                .pointer("/properties/attachment/properties/main_folder/type")
                .unwrap(),
            &serde_json::json!(["string", "null"])
        );
    }

    #[test]
    fn injects_pair_constraints_for_attachment_taxonomy() {
        let taxonomy = BTreeMap::from([
            (
                "DENIS".to_string(),
                vec!["AUDI".to_string(), "LEGAL".to_string()],
            ),
            (
                "SCI_LES_ROSES".to_string(),
                vec!["LOCATION".to_string(), "LEGAL".to_string()],
            ),
        ]);
        let schema = default_output_schema(&taxonomy);
        let hardened = inject_folder_enums_and_harden(schema, &taxonomy);

        let any_of = hardened
            .pointer("/properties/attachment_summaries/items/anyOf")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(any_of.len(), 2);
        assert!(any_of.iter().any(|entry| {
            entry.pointer("/properties/main_folder/enum") == Some(&json!(["DENIS"]))
                && entry.pointer("/properties/sub_folder/enum") == Some(&json!(["AUDI", "LEGAL"]))
                && entry.pointer("/additionalProperties") == Some(&json!(false))
        }));
        assert!(any_of.iter().any(|entry| {
            entry.pointer("/properties/main_folder/enum") == Some(&json!(["SCI_LES_ROSES"]))
                && entry.pointer("/properties/sub_folder/enum")
                    == Some(&json!(["LOCATION", "LEGAL"]))
                && entry.pointer("/additionalProperties") == Some(&json!(false))
        }));
    }
}
