use std::collections::HashMap;
use std::fs;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use arrow_array::{ArrayRef, LargeStringArray, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use datafusion::arrow::util::display::array_value_to_string;
use datafusion::prelude::{ParquetReadOptions, SessionContext};
use parquet::arrow::ArrowWriter;

const TABLE_FILE_NAME: &str = "dropbox_uploads.parquet";
const TABLE_NAME: &str = "dropbox_uploads";

const COLUMN_LAYOUT_VERSION: &str = "layout_version";
const COLUMN_UPLOADED_AT: &str = "uploaded_at";
const COLUMN_MD5: &str = "md5";
const COLUMN_SOURCE_FILE_NAME: &str = "source_file_name";
const COLUMN_FINAL_FILE_NAME: &str = "final_file_name";
const COLUMN_DROPBOX_ROOT_FOLDER: &str = "dropbox_root_folder";
const COLUMN_DROPBOX_PATH: &str = "dropbox_path";
const COLUMN_MAIN_FOLDER: &str = "main_folder";
const COLUMN_SUB_FOLDER: &str = "sub_folder";
const COLUMN_YEAR_FOLDER: &str = "year_folder";
const COLUMN_ACCOUNT_NAME: &str = "account_name";
const COLUMN_EMAIL_DOSSIER_PATH: &str = "email_dossier_path";
const COLUMN_EMAIL_STEM: &str = "email_stem";
const COLUMN_ATTACHMENT_LOCAL_PATH: &str = "attachment_local_path";
const COLUMN_PARSE_JSON_PATH: &str = "parse_json_path";
const COLUMN_PARSE_JSON_CONTENT: &str = "parse_json_content";
const COLUMN_PARSE_XML_PATH: &str = "parse_xml_path";
const COLUMN_PARSE_XML_CONTENT: &str = "parse_xml_content";
const COLUMN_AI_JSON_PATH: &str = "ai_json_path";
const COLUMN_AI_JSON_CONTENT: &str = "ai_json_content";
const COLUMN_MESSAGE_ID: &str = "message_id";
const COLUMN_EMAIL_SUBJECT: &str = "email_subject";
const COLUMN_EXPEDITION_DATE: &str = "expedition_date";
const COLUMN_EMAIL_SUMMARY: &str = "email_summary";
const COLUMN_EMAIL_IMPORTANCE: &str = "email_importance";
const COLUMN_ATTACHMENT_SUMMARY: &str = "attachment_summary";
const COLUMN_ATTACHMENT_IMPORTANCE: &str = "attachment_importance";
const COLUMN_ATTACHMENT_CONFIDENCE: &str = "attachment_confidence";
const COLUMN_ATTACHMENT_MIME_TYPE: &str = "attachment_mime_type";

#[derive(Debug, Clone)]
pub struct DropboxArchiveTable {
    path: PathBuf,
    records: Vec<ArchiveRecord>,
    md5_to_path: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ArchiveRecord {
    pub layout_version: String,
    pub uploaded_at: String,
    pub md5: String,
    pub source_file_name: String,
    pub final_file_name: String,
    pub dropbox_root_folder: String,
    pub dropbox_path: String,
    pub main_folder: String,
    pub sub_folder: String,
    pub year_folder: String,
    pub account_name: String,
    pub email_dossier_path: String,
    pub email_stem: String,
    pub attachment_local_path: String,
    pub parse_json_path: String,
    pub parse_json_content: String,
    pub parse_xml_path: String,
    pub parse_xml_content: String,
    pub ai_json_path: String,
    pub ai_json_content: String,
    pub message_id: String,
    pub email_subject: String,
    pub expedition_date: String,
    pub email_summary: String,
    pub email_importance: String,
    pub attachment_summary: String,
    pub attachment_importance: String,
    pub attachment_confidence: String,
    pub attachment_mime_type: String,
}

impl DropboxArchiveTable {
    pub async fn load(root_folder: &Path) -> Result<Self> {
        let path = root_folder.join(TABLE_FILE_NAME);
        if !path.exists() {
            return Ok(Self {
                path,
                records: Vec::new(),
                md5_to_path: HashMap::new(),
            });
        }

        let records = load_records_with_datafusion(&path).await?;
        let md5_to_path = records
            .iter()
            .map(|record| (record.md5.clone(), record.dropbox_path.clone()))
            .collect::<HashMap<_, _>>();
        Ok(Self {
            path,
            records,
            md5_to_path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn contains_md5(&self, md5: &str) -> bool {
        self.md5_to_path.contains_key(md5)
    }

    pub fn existing_path_for_md5(&self, md5: &str) -> Option<&str> {
        self.md5_to_path.get(md5).map(String::as_str)
    }

    pub fn append(&mut self, record: ArchiveRecord) -> Result<()> {
        self.md5_to_path
            .insert(record.md5.clone(), record.dropbox_path.clone());
        self.records.push(record);
        Ok(())
    }

    pub fn persist(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("unable to create {}", parent.display()))?;
        }

        let schema = table_schema();
        let arrays = vec![
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.layout_version.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.uploaded_at.clone())
                    .collect(),
            ),
            large_string_array(self.records.iter().map(|record| record.md5.clone()).collect()),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.source_file_name.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.final_file_name.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.dropbox_root_folder.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.dropbox_path.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.main_folder.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.sub_folder.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.year_folder.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.account_name.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.email_dossier_path.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.email_stem.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.attachment_local_path.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.parse_json_path.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.parse_json_content.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.parse_xml_path.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.parse_xml_content.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.ai_json_path.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.ai_json_content.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.message_id.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.email_subject.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.expedition_date.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.email_summary.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.email_importance.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.attachment_summary.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.attachment_importance.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.attachment_confidence.clone())
                    .collect(),
            ),
            large_string_array(
                self.records
                    .iter()
                    .map(|record| record.attachment_mime_type.clone())
                    .collect(),
            ),
        ];
        let batch = RecordBatch::try_new(schema.clone(), arrays)
            .with_context(|| "unable to build parquet record batch")?;

        let tmp_path = self.path.with_extension("parquet.tmp");
        let file = File::create(&tmp_path)
            .with_context(|| format!("unable to create {}", tmp_path.display()))?;
        let mut writer = ArrowWriter::try_new(file, schema, None)
            .with_context(|| format!("unable to open parquet writer {}", tmp_path.display()))?;
        writer
            .write(&batch)
            .with_context(|| format!("unable to write {}", tmp_path.display()))?;
        writer
            .close()
            .with_context(|| format!("unable to finalize {}", tmp_path.display()))?;
        fs::rename(&tmp_path, &self.path).with_context(|| {
            format!(
                "unable to move parquet archive {} -> {}",
                tmp_path.display(),
                self.path.display()
            )
        })?;
        Ok(())
    }
}

async fn load_records_with_datafusion(path: &Path) -> Result<Vec<ArchiveRecord>> {
    let ctx = SessionContext::new();
    ctx.register_parquet(
        TABLE_NAME,
        &path.display().to_string(),
        ParquetReadOptions::default(),
    )
    .await
    .with_context(|| format!("unable to register parquet table {}", path.display()))?;
    let dataframe = ctx
        .sql(&format!("SELECT * FROM {TABLE_NAME}"))
        .await
        .with_context(|| format!("unable to query parquet table {}", path.display()))?;
    let batches = dataframe
        .collect()
        .await
        .with_context(|| format!("unable to collect parquet rows {}", path.display()))?;

    let mut records = Vec::new();
    for batch in batches {
        records.extend(read_batch(&batch)?);
    }
    Ok(records)
}

fn read_batch(batch: &RecordBatch) -> Result<Vec<ArchiveRecord>> {
    let layout_version = column_values(batch, COLUMN_LAYOUT_VERSION)?;
    let uploaded_at = column_values(batch, COLUMN_UPLOADED_AT)?;
    let md5 = column_values(batch, COLUMN_MD5)?;
    let source_file_name = column_values(batch, COLUMN_SOURCE_FILE_NAME)?;
    let final_file_name = column_values(batch, COLUMN_FINAL_FILE_NAME)?;
    let dropbox_root_folder = column_values(batch, COLUMN_DROPBOX_ROOT_FOLDER)?;
    let dropbox_path = column_values(batch, COLUMN_DROPBOX_PATH)?;
    let main_folder = column_values(batch, COLUMN_MAIN_FOLDER)?;
    let sub_folder = column_values(batch, COLUMN_SUB_FOLDER)?;
    let year_folder = column_values(batch, COLUMN_YEAR_FOLDER)?;
    let account_name = column_values(batch, COLUMN_ACCOUNT_NAME)?;
    let email_dossier_path = column_values(batch, COLUMN_EMAIL_DOSSIER_PATH)?;
    let email_stem = column_values(batch, COLUMN_EMAIL_STEM)?;
    let attachment_local_path = column_values(batch, COLUMN_ATTACHMENT_LOCAL_PATH)?;
    let parse_json_path = column_values(batch, COLUMN_PARSE_JSON_PATH)?;
    let parse_json_content = column_values(batch, COLUMN_PARSE_JSON_CONTENT)?;
    let parse_xml_path = column_values(batch, COLUMN_PARSE_XML_PATH)?;
    let parse_xml_content = column_values(batch, COLUMN_PARSE_XML_CONTENT)?;
    let ai_json_path = column_values(batch, COLUMN_AI_JSON_PATH)?;
    let ai_json_content = column_values(batch, COLUMN_AI_JSON_CONTENT)?;
    let message_id = column_values(batch, COLUMN_MESSAGE_ID)?;
    let email_subject = column_values(batch, COLUMN_EMAIL_SUBJECT)?;
    let expedition_date = column_values(batch, COLUMN_EXPEDITION_DATE)?;
    let email_summary = column_values(batch, COLUMN_EMAIL_SUMMARY)?;
    let email_importance = column_values(batch, COLUMN_EMAIL_IMPORTANCE)?;
    let attachment_summary = column_values(batch, COLUMN_ATTACHMENT_SUMMARY)?;
    let attachment_importance = column_values(batch, COLUMN_ATTACHMENT_IMPORTANCE)?;
    let attachment_confidence = column_values(batch, COLUMN_ATTACHMENT_CONFIDENCE)?;
    let attachment_mime_type = column_values(batch, COLUMN_ATTACHMENT_MIME_TYPE)?;

    let mut records = Vec::with_capacity(batch.num_rows());
    for index in 0..batch.num_rows() {
        records.push(ArchiveRecord {
            layout_version: layout_version[index].clone(),
            uploaded_at: uploaded_at[index].clone(),
            md5: md5[index].clone(),
            source_file_name: source_file_name[index].clone(),
            final_file_name: final_file_name[index].clone(),
            dropbox_root_folder: dropbox_root_folder[index].clone(),
            dropbox_path: dropbox_path[index].clone(),
            main_folder: main_folder[index].clone(),
            sub_folder: sub_folder[index].clone(),
            year_folder: year_folder[index].clone(),
            account_name: account_name[index].clone(),
            email_dossier_path: email_dossier_path[index].clone(),
            email_stem: email_stem[index].clone(),
            attachment_local_path: attachment_local_path[index].clone(),
            parse_json_path: parse_json_path[index].clone(),
            parse_json_content: parse_json_content[index].clone(),
            parse_xml_path: parse_xml_path[index].clone(),
            parse_xml_content: parse_xml_content[index].clone(),
            ai_json_path: ai_json_path[index].clone(),
            ai_json_content: ai_json_content[index].clone(),
            message_id: message_id[index].clone(),
            email_subject: email_subject[index].clone(),
            expedition_date: expedition_date[index].clone(),
            email_summary: email_summary[index].clone(),
            email_importance: email_importance[index].clone(),
            attachment_summary: attachment_summary[index].clone(),
            attachment_importance: attachment_importance[index].clone(),
            attachment_confidence: attachment_confidence[index].clone(),
            attachment_mime_type: attachment_mime_type[index].clone(),
        });
    }
    Ok(records)
}

fn column_values(batch: &RecordBatch, name: &str) -> Result<Vec<String>> {
    let index = batch
        .schema_ref()
        .index_of(name)
        .with_context(|| format!("missing parquet column `{name}`"))?;
    let column = batch.column(index);
    (0..column.len())
        .map(|row| {
            array_value_to_string(column.as_ref(), row).with_context(|| {
                format!("unable to decode parquet value `{name}` at row {row}")
            })
        })
        .collect()
}

fn table_schema() -> Arc<Schema> {
    Arc::new(Schema::new(
        [
            COLUMN_LAYOUT_VERSION,
            COLUMN_UPLOADED_AT,
            COLUMN_MD5,
            COLUMN_SOURCE_FILE_NAME,
            COLUMN_FINAL_FILE_NAME,
            COLUMN_DROPBOX_ROOT_FOLDER,
            COLUMN_DROPBOX_PATH,
            COLUMN_MAIN_FOLDER,
            COLUMN_SUB_FOLDER,
            COLUMN_YEAR_FOLDER,
            COLUMN_ACCOUNT_NAME,
            COLUMN_EMAIL_DOSSIER_PATH,
            COLUMN_EMAIL_STEM,
            COLUMN_ATTACHMENT_LOCAL_PATH,
            COLUMN_PARSE_JSON_PATH,
            COLUMN_PARSE_JSON_CONTENT,
            COLUMN_PARSE_XML_PATH,
            COLUMN_PARSE_XML_CONTENT,
            COLUMN_AI_JSON_PATH,
            COLUMN_AI_JSON_CONTENT,
            COLUMN_MESSAGE_ID,
            COLUMN_EMAIL_SUBJECT,
            COLUMN_EXPEDITION_DATE,
            COLUMN_EMAIL_SUMMARY,
            COLUMN_EMAIL_IMPORTANCE,
            COLUMN_ATTACHMENT_SUMMARY,
            COLUMN_ATTACHMENT_IMPORTANCE,
            COLUMN_ATTACHMENT_CONFIDENCE,
            COLUMN_ATTACHMENT_MIME_TYPE,
        ]
        .into_iter()
        .map(|name| Field::new(name, DataType::LargeUtf8, false))
        .collect::<Vec<_>>(),
    ))
}

fn large_string_array(values: Vec<String>) -> ArrayRef {
    Arc::new(LargeStringArray::from(values))
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{ArchiveRecord, DropboxArchiveTable};

    #[tokio::test]
    async fn persists_and_reloads_parquet_archive() {
        let dir = tempdir().unwrap();
        let mut table = DropboxArchiveTable::load(dir.path()).await.unwrap();
        assert_eq!(table.len(), 0);

        table
            .append(ArchiveRecord {
                layout_version: "3".to_string(),
                uploaded_at: "2026-04-04T12:00:00Z".to_string(),
                md5: "abc".to_string(),
                source_file_name: "source.pdf".to_string(),
                final_file_name: "final.pdf".to_string(),
                dropbox_root_folder: "/COBRA_TEST".to_string(),
                dropbox_path: "/COBRA_TEST/DENIS/FACTURES/2026/final.pdf".to_string(),
                main_folder: "DENIS".to_string(),
                sub_folder: "FACTURES".to_string(),
                year_folder: "2026".to_string(),
                account_name: "Test".to_string(),
                email_dossier_path: "/tmp/email".to_string(),
                email_stem: "Email".to_string(),
                attachment_local_path: "/tmp/source.pdf".to_string(),
                parse_json_path: "/tmp/email.json".to_string(),
                parse_json_content: "{\"a\":1}".to_string(),
                parse_xml_path: "/tmp/email.xml".to_string(),
                parse_xml_content: "<email />".to_string(),
                ai_json_path: "/tmp/email.ai.json".to_string(),
                ai_json_content: "{\"main_folder\":\"DENIS\"}".to_string(),
                message_id: "<id>".to_string(),
                email_subject: "Sujet".to_string(),
                expedition_date: "2026-04-04T10:00:00Z".to_string(),
                email_summary: "Resume".to_string(),
                email_importance: "HAUTE".to_string(),
                attachment_summary: "Piece jointe".to_string(),
                attachment_importance: "HAUTE".to_string(),
                attachment_confidence: "0.99".to_string(),
                attachment_mime_type: "application/pdf".to_string(),
            })
            .unwrap();
        table.persist().unwrap();

        let reloaded = DropboxArchiveTable::load(dir.path()).await.unwrap();
        assert_eq!(reloaded.len(), 1);
        assert!(reloaded.contains_md5("abc"));
        assert_eq!(
            reloaded.existing_path_for_md5("abc"),
            Some("/COBRA_TEST/DENIS/FACTURES/2026/final.pdf")
        );
    }
}
