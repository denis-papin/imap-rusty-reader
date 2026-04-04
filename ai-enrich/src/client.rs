use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use regex::Regex;
use reqwest::StatusCode;
use reqwest::multipart::{Form, Part};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug)]
pub struct OpenAiClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    request_delay: Duration,
    last_request_at: Mutex<Option<Instant>>,
}

#[derive(Debug, Clone)]
pub struct ResponseEnvelope {
    pub id: Option<String>,
    pub output_json: Value,
    pub usage: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct UploadedFile {
    pub file_id: String,
}

#[derive(Debug, Clone)]
struct ApiError {
    operation: &'static str,
    status: StatusCode,
    body: String,
    retryable: bool,
    retry_after: Option<Duration>,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let retry_hint = if self.retryable {
            "retryable"
        } else {
            "non-retryable"
        };
        write!(
            f,
            "OpenAI {} failed ({}, {}): {}",
            self.operation, self.status, retry_hint, self.body
        )
    }
}

impl std::error::Error for ApiError {}

#[derive(Debug, Serialize)]
pub struct ResponsesRequest {
    pub model: String,
    pub instructions: String,
    pub input: Vec<ResponseInputItem>,
    pub store: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    pub text: ResponseTextConfig,
}

#[derive(Debug, Serialize)]
pub struct ResponseTextConfig {
    pub format: Value,
}

#[derive(Debug, Serialize)]
pub struct ResponseInputItem {
    pub role: String,
    pub content: Vec<ResponseContentItem>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum ResponseContentItem {
    #[serde(rename = "input_text")]
    InputText { text: String },
    #[serde(rename = "input_file")]
    InputFile { file_id: String },
}

impl OpenAiClient {
    pub fn new(
        api_key: String,
        base_url: Option<&str>,
        timeout_seconds: u64,
        request_delay_ms: u64,
    ) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_seconds))
            .build()
            .with_context(|| "unable to build OpenAI HTTP client")?;
        let base_url = base_url
            .unwrap_or("https://api.openai.com/v1")
            .trim_end_matches('/')
            .to_string();

        Ok(Self {
            http,
            api_key,
            base_url,
            request_delay: Duration::from_millis(request_delay_ms),
            last_request_at: Mutex::new(None),
        })
    }

    pub async fn upload_file(&self, path: &Path) -> Result<UploadedFile> {
        self.wait_for_request_slot().await;
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("attachment.bin")
            .to_string();
        let bytes = fs::read(path)
            .with_context(|| format!("unable to read attachment {}", path.display()))?;

        let part = Part::bytes(bytes).file_name(file_name);
        let form = Form::new().text("purpose", "user_data").part("file", part);
        let url = format!("{}/files", self.base_url);
        let response = self
            .http
            .post(url)
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .with_context(|| format!("OpenAI file upload failed for {}", path.display()))?;

        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .text()
            .await
            .with_context(|| "unable to read OpenAI file upload response body")?;

        if !status.is_success() {
            return Err(classify_api_error("file upload", status, &headers, body).into());
        }

        let payload: Value = serde_json::from_str(&body)
            .with_context(|| "unable to parse OpenAI file upload response JSON")?;
        let file_id = payload
            .get("id")
            .and_then(Value::as_str)
            .map(|value| value.to_string())
            .ok_or_else(|| anyhow!("OpenAI file upload response missing id"))?;
        Ok(UploadedFile { file_id })
    }

    pub async fn create_response_with_retry(
        &self,
        request: &ResponsesRequest,
        retry_count: usize,
    ) -> Result<ResponseEnvelope> {
        let mut attempt = 0usize;
        loop {
            match self.create_response(request).await {
                Ok(response) => return Ok(response),
                Err(error) if error.retryable && attempt < retry_count => {
                    attempt += 1;
                    let delay = error.retry_after.unwrap_or_else(|| {
                        let delay_seconds = attempt as u64;
                        Duration::from_secs(delay_seconds)
                    });
                    log::warn!("💣 OpenAI call failed, retry {attempt}/{retry_count}: {error}");
                    tokio::time::sleep(delay).await;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    async fn create_response(
        &self,
        request: &ResponsesRequest,
    ) -> std::result::Result<ResponseEnvelope, ApiError> {
        self.wait_for_request_slot().await;
        let url = format!("{}/responses", self.base_url);
        let response = self
            .http
            .post(url)
            .bearer_auth(&self.api_key)
            .json(request)
            .send()
            .await
            .map_err(|error| ApiError {
                operation: "Responses API",
                status: StatusCode::REQUEST_TIMEOUT,
                body: format!("request transport error: {error:#}"),
                retryable: true,
                retry_after: None,
            })?;

        let status = response.status();
        let headers = response.headers().clone();
        let body = response.text().await.map_err(|error| ApiError {
            operation: "Responses API",
            status,
            body: format!("unable to read response body: {error:#}"),
            retryable: matches!(
                status,
                StatusCode::TOO_MANY_REQUESTS
                    | StatusCode::INTERNAL_SERVER_ERROR
                    | StatusCode::BAD_GATEWAY
                    | StatusCode::SERVICE_UNAVAILABLE
                    | StatusCode::GATEWAY_TIMEOUT
            ),
            retry_after: None,
        })?;

        if !status.is_success() {
            return Err(classify_api_error("Responses API", status, &headers, body));
        }

        let payload: Value = serde_json::from_str(&body).map_err(|error| ApiError {
            operation: "Responses API",
            status,
            body: format!("unable to parse OpenAI Responses API JSON body: {error:#}"),
            retryable: false,
            retry_after: None,
        })?;
        let output_json = extract_output_json(&payload).map_err(|error| ApiError {
            operation: "Responses API",
            status,
            body: format!("unable to extract JSON payload: {error:#}"),
            retryable: false,
            retry_after: None,
        })?;

        Ok(ResponseEnvelope {
            id: payload
                .get("id")
                .and_then(Value::as_str)
                .map(|value| value.to_string()),
            output_json,
            usage: payload.get("usage").cloned(),
        })
    }

    async fn wait_for_request_slot(&self) {
        if self.request_delay.is_zero() {
            return;
        }

        let delay = {
            let mut last_request_at = self
                .last_request_at
                .lock()
                .expect("OpenAiClient request delay mutex poisoned");
            let now = Instant::now();
            let delay = match *last_request_at {
                Some(previous) => {
                    let elapsed = now.saturating_duration_since(previous);
                    self.request_delay.checked_sub(elapsed)
                }
                None => None,
            };
            *last_request_at = Some(now);
            delay
        };

        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
            let mut last_request_at = self
                .last_request_at
                .lock()
                .expect("OpenAiClient request delay mutex poisoned");
            *last_request_at = Some(Instant::now());
        }
    }
}

fn classify_api_error(
    operation: &'static str,
    status: StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: String,
) -> ApiError {
    let payload = serde_json::from_str::<Value>(&body).ok();
    let error = payload
        .as_ref()
        .and_then(|value| value.get("error"))
        .cloned()
        .unwrap_or(Value::Null);
    let error_code = error.get("code").and_then(Value::as_str);
    let error_type = error.get("type").and_then(Value::as_str);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or(body.as_str());

    let retryable = match status {
        StatusCode::TOO_MANY_REQUESTS => {
            error_code != Some("insufficient_quota") && error_type != Some("insufficient_quota")
        }
        StatusCode::INTERNAL_SERVER_ERROR
        | StatusCode::BAD_GATEWAY
        | StatusCode::SERVICE_UNAVAILABLE
        | StatusCode::GATEWAY_TIMEOUT
        | StatusCode::REQUEST_TIMEOUT => true,
        _ => false,
    };

    let retry_after = parse_retry_after(headers, message);

    ApiError {
        operation,
        status,
        body,
        retryable,
        retry_after,
    }
}

fn parse_retry_after(headers: &reqwest::header::HeaderMap, message: &str) -> Option<Duration> {
    if let Some(value) = headers
        .get("retry-after-ms")
        .and_then(|value| value.to_str().ok())
    {
        if let Ok(milliseconds) = value.parse::<u64>() {
            return Some(Duration::from_millis(milliseconds));
        }
    }
    if let Some(value) = headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
    {
        if let Ok(seconds) = value.parse::<u64>() {
            return Some(Duration::from_secs(seconds));
        }
    }

    let regex = Regex::new(r"Please try again in (\d+)(ms|s)").expect("invalid regex");
    let captures = regex.captures(message)?;
    let amount = captures.get(1)?.as_str().parse::<u64>().ok()?;
    let unit = captures.get(2)?.as_str();
    match unit {
        "ms" => Some(Duration::from_millis(amount)),
        "s" => Some(Duration::from_secs(amount)),
        _ => None,
    }
}

fn extract_output_json(payload: &Value) -> Result<Value> {
    if let Some(text) = payload.get("output_text").and_then(Value::as_str) {
        return serde_json::from_str(text)
            .with_context(|| "unable to parse top-level output_text as JSON");
    }

    let items = payload
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("OpenAI response missing output array"))?;
    for item in items {
        let Some(content_items) = item.get("content").and_then(Value::as_array) else {
            continue;
        };
        for content in content_items {
            if let Some(text) = content.get("text").and_then(Value::as_str) {
                if let Ok(json_payload) = serde_json::from_str::<Value>(text) {
                    return Ok(json_payload);
                }
            }
            if let Some(arguments) = content.get("arguments").and_then(Value::as_str) {
                if let Ok(json_payload) = serde_json::from_str::<Value>(arguments) {
                    return Ok(json_payload);
                }
            }
        }
    }

    Err(anyhow!(
        "OpenAI response did not contain a parsable JSON result in output_text/content"
    ))
}

pub fn json_schema_format(name: &str, schema: Value) -> Value {
    json!({
        "type": "json_schema",
        "name": name,
        "strict": true,
        "schema": schema,
    })
}
