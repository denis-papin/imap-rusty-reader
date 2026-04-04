use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

use crate::utils::{escape_non_ascii_json, folder_prefixes, parent_dropbox_folder};

const SIMPLE_UPLOAD_MAX_BYTES: u64 = 150 * 1024 * 1024;
const UPLOAD_SESSION_CHUNK_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub struct DropboxClient {
    http: Client,
    access_token: String,
    api_base_url: String,
    content_base_url: String,
    ensured_folders: Mutex<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct UploadSessionStartResponse {
    session_id: String,
}

impl DropboxClient {
    pub fn new(
        access_token: String,
        api_base_url: Option<&str>,
        content_base_url: Option<&str>,
        timeout_seconds: u64,
    ) -> Result<Self> {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_seconds))
            .build()
            .with_context(|| "unable to build HTTP client for Dropbox")?;
        Ok(Self {
            http,
            access_token,
            api_base_url: api_base_url
                .unwrap_or("https://api.dropboxapi.com")
                .trim_end_matches('/')
                .to_string(),
            content_base_url: content_base_url
                .unwrap_or("https://content.dropboxapi.com")
                .trim_end_matches('/')
                .to_string(),
            ensured_folders: Mutex::new(Vec::new()),
        })
    }

    pub async fn ensure_parent_folders(&self, file_path: &str) -> Result<()> {
        let parent = parent_dropbox_folder(file_path)?;
        for folder in folder_prefixes(&parent)? {
            let should_create = {
                let mut guard = self
                    .ensured_folders
                    .lock()
                    .map_err(|_| anyhow::anyhow!("dropbox folder cache mutex poisoned"))?;
                if guard.iter().any(|value| value == &folder) {
                    false
                } else {
                    guard.push(folder.clone());
                    true
                }
            };

            if should_create {
                self.create_folder(&folder).await?;
            }
        }
        Ok(())
    }

    pub async fn upload_file(&self, local_path: &Path, remote_path: &str) -> Result<()> {
        let metadata = std::fs::metadata(local_path)
            .with_context(|| format!("unable to read metadata {}", local_path.display()))?;
        if metadata.len() <= SIMPLE_UPLOAD_MAX_BYTES {
            self.upload_small_file(local_path, remote_path).await
        } else {
            self.upload_large_file(local_path, remote_path).await
        }
    }

    pub async fn file_exists(&self, path: &str) -> Result<bool> {
        let url = format!("{}/2/files/get_metadata", self.api_base_url);
        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.access_token)
            .json(&json!({
                "path": path,
                "include_deleted": false
            }))
            .send()
            .await
            .with_context(|| format!("unable to check metadata for `{path}`"))?;

        if response.status().is_success() {
            return Ok(true);
        }

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if status.as_u16() == 409 && body.contains("not_found") {
            return Ok(false);
        }

        bail!(
            "dropbox get_metadata failed [{}] status {}: {}",
            path,
            status,
            compact_body(&body)
        );
    }

    async fn create_folder(&self, path: &str) -> Result<()> {
        let url = format!("{}/2/files/create_folder_v2", self.api_base_url);
        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.access_token)
            .json(&json!({
                "path": path,
                "autorename": false
            }))
            .send()
            .await
            .with_context(|| format!("unable to create dropbox folder `{path}`"))?;

        if response.status().is_success() {
            return Ok(());
        }

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if status.as_u16() == 409 && body.contains("conflict") {
            return Ok(());
        }

        bail!(
            "dropbox create_folder_v2 failed [{}] status {}: {}",
            path,
            status,
            compact_body(&body)
        );
    }

    async fn upload_small_file(&self, local_path: &Path, remote_path: &str) -> Result<()> {
        let bytes = std::fs::read(local_path)
            .with_context(|| format!("unable to read {}", local_path.display()))?;
        let arg = escape_non_ascii_json(&serde_json::to_string(&json!({
            "path": remote_path,
            "mode": "add",
            "autorename": false,
            "mute": true,
            "strict_conflict": false
        }))?);
        let url = format!("{}/2/files/upload", self.content_base_url);
        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.access_token)
            .header("Content-Type", "application/octet-stream")
            .header("Dropbox-API-Arg", arg)
            .body(bytes)
            .send()
            .await
            .with_context(|| format!("unable to upload {}", local_path.display()))?;

        self.ensure_upload_success(response, remote_path).await
    }

    async fn upload_large_file(&self, local_path: &Path, remote_path: &str) -> Result<()> {
        let mut file =
            File::open(local_path).with_context(|| format!("unable to open {}", local_path.display()))?;
        let mut buffer = vec![0u8; UPLOAD_SESSION_CHUNK_BYTES];
        let mut bytes_read = file
            .read(&mut buffer)
            .with_context(|| format!("unable to read {}", local_path.display()))?;
        if bytes_read == 0 {
            return self.upload_small_file(local_path, remote_path).await;
        }
        buffer.truncate(bytes_read);

        let start_arg = escape_non_ascii_json(&serde_json::to_string(&json!({ "close": false }))?);
        let start_url = format!("{}/2/files/upload_session/start", self.content_base_url);
        let start_response = self
            .http
            .post(&start_url)
            .bearer_auth(&self.access_token)
            .header("Content-Type", "application/octet-stream")
            .header("Dropbox-API-Arg", start_arg)
            .body(buffer)
            .send()
            .await
            .with_context(|| format!("unable to start upload session for {}", local_path.display()))?;
        let status = start_response.status();
        let body = start_response.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!(
                "dropbox upload_session/start failed [{}] status {}: {}",
                remote_path,
                status,
                compact_body(&body)
            );
        }
        let start = serde_json::from_str::<UploadSessionStartResponse>(&body)
            .with_context(|| "unable to parse upload session start response")?;

        let mut offset = bytes_read as u64;
        loop {
            let mut chunk = vec![0u8; UPLOAD_SESSION_CHUNK_BYTES];
            bytes_read = file
                .read(&mut chunk)
                .with_context(|| format!("unable to read {}", local_path.display()))?;
            if bytes_read == 0 {
                break;
            }
            chunk.truncate(bytes_read);

            let is_last = (offset + bytes_read as u64) == std::fs::metadata(local_path)?.len();
            if is_last {
                let finish_arg = escape_non_ascii_json(&serde_json::to_string(&json!({
                    "cursor": {
                        "session_id": start.session_id,
                        "offset": offset
                    },
                    "commit": {
                        "path": remote_path,
                        "mode": "add",
                        "autorename": false,
                        "mute": true,
                        "strict_conflict": false
                    }
                }))?);
                let finish_url = format!("{}/2/files/upload_session/finish", self.content_base_url);
                let response = self
                    .http
                    .post(&finish_url)
                    .bearer_auth(&self.access_token)
                    .header("Content-Type", "application/octet-stream")
                    .header("Dropbox-API-Arg", finish_arg)
                    .body(chunk)
                    .send()
                    .await
                    .with_context(|| {
                        format!(
                            "unable to finish upload session for {}",
                            local_path.display()
                        )
                    })?;
                return self.ensure_upload_success(response, remote_path).await;
            }

            let append_arg = escape_non_ascii_json(&serde_json::to_string(&json!({
                "cursor": {
                    "session_id": start.session_id,
                    "offset": offset
                },
                "close": false
            }))?);
            let append_url = format!("{}/2/files/upload_session/append_v2", self.content_base_url);
            let response = self
                .http
                .post(&append_url)
                .bearer_auth(&self.access_token)
                .header("Content-Type", "application/octet-stream")
                .header("Dropbox-API-Arg", append_arg)
                .body(chunk)
                .send()
                .await
                .with_context(|| {
                    format!(
                        "unable to append upload session chunk for {}",
                        local_path.display()
                    )
                })?;
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if !status.is_success() {
                bail!(
                    "dropbox upload_session/append_v2 failed [{}] status {}: {}",
                    remote_path,
                    status,
                    compact_body(&body)
                );
            }

            offset += bytes_read as u64;
        }

        bail!("upload session finished without final commit for `{remote_path}`");
    }

    async fn ensure_upload_success(&self, response: reqwest::Response, remote_path: &str) -> Result<()> {
        if response.status().is_success() {
            return Ok(());
        }

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!(
            "dropbox upload failed [{}] status {}: {}",
            remote_path,
            status,
            compact_body(&body)
        );
    }
}

fn compact_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        "<empty body>".to_string()
    } else {
        trimmed.chars().take(400).collect()
    }
}
