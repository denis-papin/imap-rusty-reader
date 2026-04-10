use anyhow::{Context, Result, anyhow, bail};
use reqwest::Client;
use serde::Deserialize;

use crate::config::Config;

const DEFAULT_OAUTH_TOKEN_URL: &str = "https://api.dropboxapi.com/oauth2/token";

#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    #[allow(dead_code)]
    token_type: Option<String>,
    #[allow(dead_code)]
    expires_in: Option<u64>,
}

pub async fn resolve_access_token(config: &Config) -> Result<String> {
    if let Some(token) = first_non_empty(&[
        std::env::var("DROPBOX_ACCESS_TOKEN").ok(),
        config.dropbox_access_token.clone(),
    ]) {
        return Ok(token);
    }

    let app_key = first_non_empty(&[
        std::env::var("DROPBOX_APP_KEY").ok(),
        config.dropbox_app_key.clone(),
    ]);
    let app_secret = first_non_empty(&[
        std::env::var("DROPBOX_APP_SECRET").ok(),
        config.dropbox_app_secret.clone(),
    ]);
    let refresh_token = first_non_empty(&[
        std::env::var("DROPBOX_REFRESH_TOKEN").ok(),
        config.dropbox_refresh_token.clone(),
    ]);
    let oauth_token_url = first_non_empty(&[
        std::env::var("DROPBOX_OAUTH_TOKEN_URL").ok(),
        config.dropbox_oauth_token_url.clone(),
    ])
    .unwrap_or_else(|| DEFAULT_OAUTH_TOKEN_URL.to_string());

    match (app_key, app_secret, refresh_token) {
        (Some(app_key), Some(app_secret), Some(refresh_token)) => {
            fetch_access_token(&oauth_token_url, &app_key, &app_secret, &refresh_token).await
        }
        (Some(_), Some(_), None) => bail!(
            "Dropbox app key/secret detected, but no refresh token was provided. Set `DROPBOX_REFRESH_TOKEN` or `dropboxRefreshToken`."
        ),
        (Some(_), None, _) | (None, Some(_), _) => bail!(
            "Dropbox app authentication requires both app key and app secret. Set `DROPBOX_APP_KEY` + `DROPBOX_APP_SECRET` or `dropboxAppKey` + `dropboxAppSecret`."
        ),
        _ => bail!(
            "Dropbox authentication is missing. Provide either `DROPBOX_ACCESS_TOKEN` (or `dropboxAccessToken`) or `DROPBOX_APP_KEY` + `DROPBOX_APP_SECRET` + `DROPBOX_REFRESH_TOKEN`."
        ),
    }
}

async fn fetch_access_token(
    oauth_token_url: &str,
    app_key: &str,
    app_secret: &str,
    refresh_token: &str,
) -> Result<String> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .with_context(|| "unable to build Dropbox OAuth client")?;
    let response = client
        .post(oauth_token_url)
        .basic_auth(app_key, Some(app_secret))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await
        .with_context(|| "unable to call Dropbox /oauth2/token endpoint")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!(
            "Dropbox OAuth token refresh failed with status {}: {}",
            status,
            compact_body(&body)
        );
    }

    let payload = response
        .json::<OAuthTokenResponse>()
        .await
        .with_context(|| "unable to parse Dropbox OAuth token response")?;
    if payload.access_token.trim().is_empty() {
        return Err(anyhow!(
            "Dropbox OAuth token response did not contain an access token"
        ));
    }
    Ok(payload.access_token)
}

fn first_non_empty(values: &[Option<String>]) -> Option<String> {
    values
        .iter()
        .flatten()
        .map(|value| value.trim())
        .find(|value| !value.is_empty())
        .map(str::to_string)
}

fn compact_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        "<empty body>".to_string()
    } else {
        trimmed.chars().take(400).collect()
    }
}
