//! Token acquisition helpers.
//!
//! Matches the logic in `codeoid/src/terminal/client.ts#getToken`:
//!
//! * A raw JWT passes through untouched.
//! * A ZeroID API key (prefix `zid_sk_`) is exchanged for an access token
//!   via `POST {zeroid_url}/oauth2/token` with `grant_type=api_key`.

use serde::Deserialize;
use thiserror::Error;

/// All Codeoid session scopes — matches `ALL_SCOPES_STRING` on the TS side.
const ALL_SCOPES: &str = "session:create session:attach session:watch session:send session:interrupt session:approve session:destroy session:list fs:read";

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("no credential provided — set CODEOID_TOKEN or CODEOID_API_KEY")]
    Missing,

    #[error("cannot reach ZeroID at {url}: {source}")]
    ZeroidUnreachable {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("ZeroID rejected the API key ({status}): {body}")]
    ZeroidRejected { status: u16, body: String },

    #[error("ZeroID response was not valid JSON: {0}")]
    MalformedResponse(#[from] reqwest::Error),
}

pub type Result<T, E = AuthError> = std::result::Result<T, E>;

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
}

/// Resolve a usable access token from whichever of `token` / `api_key` is set.
///
/// * If `token` is `Some` and non-empty → return it (assumed to be a JWT).
/// * If `api_key` starts with `zid_sk_` → exchange it at `zeroid_url`.
/// * If `api_key` looks like a JWT already → return it unchanged.
/// * Otherwise: [`AuthError::Missing`].
pub async fn resolve_token(
    token: Option<&str>,
    api_key: Option<&str>,
    zeroid_url: &str,
) -> Result<String> {
    if let Some(t) = token.filter(|s| !s.is_empty()) {
        return Ok(t.to_string());
    }
    let Some(key) = api_key.filter(|s| !s.is_empty()) else {
        return Err(AuthError::Missing);
    };
    if !key.starts_with("zid_sk_") {
        // Already an access token — bypass the exchange.
        return Ok(key.to_string());
    }
    exchange_api_key(key, zeroid_url).await
}

async fn exchange_api_key(api_key: &str, zeroid_url: &str) -> Result<String> {
    let endpoint = format!("{}/oauth2/token", zeroid_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "grant_type": "api_key",
        "api_key": api_key,
        "scope": ALL_SCOPES,
    });

    let client = reqwest::Client::builder()
        .build()
        .expect("build reqwest client");

    let resp = client
        .post(&endpoint)
        .json(&body)
        .send()
        .await
        .map_err(|e| AuthError::ZeroidUnreachable {
            url: endpoint.clone(),
            source: e,
        })?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(AuthError::ZeroidRejected {
            status: status.as_u16(),
            body,
        });
    }

    let token: TokenResponse = resp.json().await?;
    Ok(token.access_token)
}
