//! Nous Portal OAuth: device-code login and refresh-token rotation.
//!
//! Wire facts (verified against `hermes-agent` source):
//! - Device code: `POST {portal}/api/oauth/device/code` (form:
//!   `client_id`, `scope`).
//! - Token poll: `POST {portal}/api/oauth/token` (form:
//!   `grant_type=urn:ietf:params:oauth:grant-type:device_code`,
//!   `client_id`, `device_code`). `authorization_pending` → keep polling;
//!   `slow_down` → +1s interval (cap 30s).
//! - Refresh: `POST {portal}/api/oauth/token` with header
//!   `x-nous-refresh-token: <rt>` and form `grant_type=refresh_token`.
//! - Refresh tokens are **single-use**: every response's new
//!   `refresh_token` is persisted to the store *before* the access token
//!   is used, or the next refresh revokes the whole session.
//! - Portal may return `inference_base_url`; it is host-allowlisted
//!   before use so a compromised/MITM'd response can't redirect bearer
//!   tokens to an arbitrary host.

use std::sync::Arc;
use std::time::{Duration, Instant};

use hate_core::provider::{BoxFuture, CredentialSource, ProviderError};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::store::{AuthError, AuthStore, CredentialBlob, OAuthTokens};

/// Default Portal base (web app + OAuth endpoints).
pub const DEFAULT_PORTAL_URL: &str = "https://portal.nousresearch.com";
/// Paid-tier inference endpoint (OpenAI-compatible).
pub const DEFAULT_INFERENCE_URL: &str = "https://inference-api.nousresearch.com/v1";
/// Free-tier (`nous/welcome`) inference endpoint.
pub const DEFAULT_WELCOME_URL: &str = "https://welcome-api.nousresearch.com/v1";
/// OAuth client id. The Portal does not enforce a secret for the
/// device-code grant; this identifies the app in its logs.
pub const CLIENT_ID: &str = "hermes-cli";
/// Scope requested at device-code time.
pub const SCOPE: &str = "inference:invoke";

const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const DEFAULT_POLL_INTERVAL_SECS: u64 = 5;
const POLL_INTERVAL_CAP_SECS: u64 = 30;
const EDGE_BACKOFF_CAP_SECS: u64 = 60;
const REFRESH_SKEW_SECS: u64 = 60;
const DEFAULT_TOKEN_TTL_SECS: u64 = 3600;

/// What the UI/CLI needs to show the user during device login.
#[derive(Debug, Clone)]
pub struct DeviceFlowInfo {
    /// Code the user types into the Portal page.
    pub user_code: String,
    /// Page the user opens to approve.
    pub verification_url: String,
    /// URL with the code pre-filled, when the Portal provides one.
    pub verification_url_complete: Option<String>,
    /// Seconds until the device code expires.
    pub expires_in: u64,
    /// Server-suggested poll interval, seconds.
    pub interval: u64,
    /// Opaque device code carried between begin/complete. Not `pub`:
    /// callers display the user-facing fields and hand the whole struct
    /// back to [`PortalAuth::complete_device_flow`].
    device_code: String,
}

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    interval: Option<u64>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    inference_base_url: Option<String>,
    #[serde(default)]
    agent_key: Option<String>,
    #[serde(default)]
    agent_key_expires_at: Option<u64>,
    #[serde(default)]
    agent_key_expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct OAuthErrorBody {
    #[serde(default)]
    error: Option<String>,
}

/// Portal OAuth client bound to a credential store.
pub struct PortalAuth {
    http: reqwest::Client,
    portal_base: String,
    store: Arc<dyn AuthStore>,
    /// Keychain/config key for this provider (`"nous"`).
    provider: String,
}

impl PortalAuth {
    /// Client against the production Portal.
    pub fn new(store: Arc<dyn AuthStore>) -> Self {
        Self::with_portal(store, DEFAULT_PORTAL_URL)
    }

    /// Client against a custom Portal base (tests, staging, self-host).
    pub fn with_portal(store: Arc<dyn AuthStore>, portal_base: &str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("hate-agent/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            portal_base: portal_base.trim_end_matches('/').to_string(),
            store,
            provider: "nous".to_string(),
        }
    }

    /// Step 1 of device login: ask the Portal for a device code.
    ///
    /// # Errors
    /// [`AuthError::OAuth`] on transport or malformed response.
    pub async fn begin_device_flow(&self) -> Result<DeviceFlowInfo, AuthError> {
        let resp = self
            .http
            .post(format!("{}/api/oauth/device/code", self.portal_base))
            .form(&[("client_id", CLIENT_ID), ("scope", SCOPE)])
            .send()
            .await
            .map_err(|e| AuthError::OAuth(format!("device code request: {e}")))?;
        if !resp.status().is_success() {
            return Err(AuthError::OAuth(format!(
                "device code endpoint returned HTTP {}",
                resp.status()
            )));
        }
        let body: DeviceCodeResponse = resp
            .json()
            .await
            .map_err(|e| AuthError::OAuth(format!("device code decode: {e}")))?;
        Ok(DeviceFlowInfo {
            user_code: body.user_code,
            verification_url: body.verification_uri,
            verification_url_complete: body.verification_uri_complete,
            expires_in: body.expires_in.unwrap_or(900),
            interval: body
                .interval
                .unwrap_or(DEFAULT_POLL_INTERVAL_SECS)
                .clamp(1, POLL_INTERVAL_CAP_SECS),
            device_code: body.device_code,
        })
    }

    /// Step 2: poll until the user approves, the code expires, or `cancel`
    /// fires. On success the token bundle is persisted immediately.
    ///
    /// # Errors
    /// [`AuthError::OAuth`] on protocol failure/expiry,
    /// [`AuthError::Store`] if persisting fails.
    pub async fn complete_device_flow(
        &self,
        info: &DeviceFlowInfo,
        cancel: &CancellationToken,
    ) -> Result<CredentialBlob, AuthError> {
        let tokens = self.poll_for_tokens(info, cancel).await?;
        let blob = CredentialBlob::OAuth {
            tokens: tokens.clone(),
        };
        self.store.set(&self.provider, &blob)?;
        Ok(blob)
    }

    /// Full login: begin + poll + store. Returns the flow info *before*
    /// polling starts via the `on_info` callback so callers can display
    /// the user code while the poll runs.
    ///
    /// # Errors
    /// Same as [`Self::begin_device_flow`] + [`Self::complete_device_flow`].
    pub async fn login(
        &self,
        cancel: &CancellationToken,
        on_info: impl FnOnce(&DeviceFlowInfo),
    ) -> Result<CredentialBlob, AuthError> {
        let info = self.begin_device_flow().await?;
        on_info(&info);
        self.complete_device_flow(&info, cancel).await
    }

    /// Load stored tokens, refreshing when the access token is inside the
    /// skew window. Rotated refresh tokens are persisted *inside* the
    /// refresh call before returning — callers must not delay the store.
    ///
    /// # Errors
    /// [`AuthError::NeedsLogin`] when no credential exists or the refresh
    /// token was rejected; [`AuthError::OAuth`]/[`AuthError::Store`]
    /// otherwise.
    pub async fn tokens(&self) -> Result<OAuthTokens, AuthError> {
        let blob = self
            .store
            .get(&self.provider)?
            .ok_or(AuthError::NeedsLogin)?;
        let CredentialBlob::OAuth { tokens } = blob else {
            return Err(AuthError::NeedsLogin);
        };
        let needs_refresh = tokens
            .expires_at
            .is_some_and(|exp| exp <= now_secs() + REFRESH_SKEW_SECS);
        if !needs_refresh {
            return Ok(tokens);
        }
        self.refresh(&tokens).await
    }

    /// Validated inference base URL: stored override if allowlisted,
    /// else the paid-tier default.
    ///
    /// # Errors
    /// [`AuthError::OAuth`] if the stored URL fails the allowlist.
    pub fn inference_base_url(&self) -> Result<String, AuthError> {
        match self.store.get(&self.provider)? {
            Some(CredentialBlob::OAuth { tokens }) => match tokens.inference_base_url.as_deref() {
                Some(url) => validate_inference_url(url),
                None => Ok(DEFAULT_INFERENCE_URL.to_string()),
            },
            _ => Ok(DEFAULT_INFERENCE_URL.to_string()),
        }
    }

    /// Remove stored Nous credentials (logout).
    ///
    /// # Errors
    /// [`AuthError::Store`] on keychain failure.
    pub fn logout(&self) -> Result<(), AuthError> {
        self.store.delete(&self.provider)
    }

    async fn poll_for_tokens(
        &self,
        info: &DeviceFlowInfo,
        cancel: &CancellationToken,
    ) -> Result<OAuthTokens, AuthError> {
        let deadline = Instant::now() + Duration::from_secs(info.expires_in.max(1));
        let mut interval = info.interval.max(1);
        let mut edge_backoff = 0u64;

        loop {
            if cancel.is_cancelled() {
                return Err(AuthError::OAuth("device login cancelled".to_string()));
            }
            if Instant::now() >= deadline {
                return Err(AuthError::OAuth(
                    "timed out waiting for device authorization".to_string(),
                ));
            }

            let resp = self
                .http
                .post(format!("{}/api/oauth/token", self.portal_base))
                .form(&[
                    ("grant_type", DEVICE_GRANT),
                    ("client_id", CLIENT_ID),
                    ("device_code", info.device_code.as_str()),
                ])
                .send()
                .await;

            let Ok(resp) = resp else {
                edge_backoff = next_edge_backoff(edge_backoff, interval);
                cancellable_sleep(
                    Duration::from_secs(edge_backoff.min(remaining(deadline))),
                    cancel,
                )
                .await?;
                continue;
            };

            if resp.status().is_success() {
                let body: TokenResponse = resp
                    .json()
                    .await
                    .map_err(|e| AuthError::OAuth(format!("token decode: {e}")))?;
                return Ok(tokens_from_response(&body, None));
            }

            let status = resp.status().as_u16();
            let retry_after = retry_after_secs(&resp);
            let mitigated = resp.headers().get("x-vercel-mitigated").is_some();
            let error_code = resp
                .json::<OAuthErrorBody>()
                .await
                .ok()
                .and_then(|b| b.error);

            // RFC 8628 pending/slow_down are JSON errors on a 400.
            match error_code.as_deref() {
                Some("authorization_pending") => {
                    edge_backoff = 0;
                    cancellable_sleep(
                        Duration::from_secs(interval.min(remaining(deadline).max(1))),
                        cancel,
                    )
                    .await?;
                    continue;
                }
                Some("slow_down") => {
                    edge_backoff = 0;
                    interval = (interval + 1).min(POLL_INTERVAL_CAP_SECS);
                    cancellable_sleep(Duration::from_secs(interval), cancel).await?;
                    continue;
                }
                Some(other) => {
                    return Err(AuthError::OAuth(format!(
                        "device authorization failed: {other}"
                    )));
                }
                None => {}
            }

            // Edge/WAF mitigation or transient service failure: back off
            // and keep polling — the user may still be approving.
            if status == 408 || status == 429 || status >= 500 || (status == 403 && mitigated) {
                edge_backoff = match retry_after {
                    Some(secs) => secs.clamp(1, EDGE_BACKOFF_CAP_SECS),
                    None => next_edge_backoff(edge_backoff, interval),
                };
                cancellable_sleep(
                    Duration::from_secs(edge_backoff.min(remaining(deadline))),
                    cancel,
                )
                .await?;
                continue;
            }

            return Err(AuthError::OAuth(format!(
                "token endpoint returned HTTP {status}"
            )));
        }
    }

    /// Refresh an expired/expiring access token. On success the rotated
    /// bundle is written to the store *before* returning — a crash between
    /// refresh and persist would orphan the session.
    ///
    /// # Errors
    /// [`AuthError::NeedsLogin`] on `invalid_grant` (re-login required);
    /// [`AuthError::OAuth`] on other failures.
    async fn refresh(&self, tokens: &OAuthTokens) -> Result<OAuthTokens, AuthError> {
        let resp = self
            .http
            .post(format!("{}/api/oauth/token", self.portal_base))
            .header("x-nous-refresh-token", &tokens.refresh_token)
            .form(&[("grant_type", "refresh_token"), ("client_id", CLIENT_ID)])
            .send()
            .await
            .map_err(|e| AuthError::OAuth(format!("refresh request: {e}")))?;

        let status = resp.status().as_u16();
        if status == 200 {
            let body: TokenResponse = resp
                .json()
                .await
                .map_err(|e| AuthError::OAuth(format!("refresh decode: {e}")))?;
            // Carry forward fields the refresh response omits — most
            // importantly the refresh token itself when the Portal does
            // not rotate it on this call.
            let refreshed = tokens_from_response(&body, Some(tokens));
            let blob = CredentialBlob::OAuth {
                tokens: refreshed.clone(),
            };
            self.store.set(&self.provider, &blob)?;
            return Ok(refreshed);
        }

        if matches!(status, 400 | 401) {
            let code = resp
                .json::<OAuthErrorBody>()
                .await
                .ok()
                .and_then(|b| b.error);
            if code.as_deref() == Some("invalid_grant") {
                self.store.delete(&self.provider).ok();
                return Err(AuthError::NeedsLogin);
            }
            return Err(AuthError::OAuth(format!(
                "refresh rejected: {}",
                code.unwrap_or_else(|| format!("HTTP {status}"))
            )));
        }
        if status >= 500 {
            return Err(AuthError::OAuth(format!(
                "Portal temporarily unavailable (HTTP {status})"
            )));
        }
        // Vercel edge mitigation (403/429 + x-vercel-mitigated) — keep
        // credentials, tell the caller to retry.
        if matches!(status, 403 | 429) {
            return Err(AuthError::OAuth(format!(
                "Portal edge challenged refresh (HTTP {status}); retry shortly"
            )));
        }
        Err(AuthError::OAuth(format!(
            "refresh failed with HTTP {status}"
        )))
    }
}

/// `CredentialSource` backed by Portal OAuth: bearer prefers the
/// short-lived `agent_key` when present and unexpired, else the access
/// token. Refresh happens inside [`PortalAuth::tokens`].
pub struct NousCredentialSource {
    auth: Arc<PortalAuth>,
}

impl NousCredentialSource {
    /// Wrap a shared [`PortalAuth`].
    pub fn new(auth: Arc<PortalAuth>) -> Self {
        Self { auth }
    }
}

impl CredentialSource for NousCredentialSource {
    fn bearer(&self) -> BoxFuture<'_, Result<String, ProviderError>> {
        let auth = Arc::clone(&self.auth);
        Box::pin(async move {
            let tokens = auth.tokens().await.map_err(|e| match e {
                AuthError::NeedsLogin => ProviderError::Auth("Nous login required".to_string()),
                other => ProviderError::Auth(other.to_string()),
            })?;
            if let Some(key) = tokens.agent_key.as_deref() {
                let usable = tokens
                    .agent_key_expires_at
                    .is_none_or(|exp| exp > now_secs());
                if usable {
                    return Ok(key.to_string());
                }
            }
            Ok(tokens.access_token)
        })
    }
}

fn tokens_from_response(body: &TokenResponse, prior: Option<&OAuthTokens>) -> OAuthTokens {
    let now = now_secs();
    OAuthTokens {
        access_token: body.access_token.clone(),
        // Prefer the rotated token; keep the prior only when the response
        // omits one (some deployments don't rotate every call).
        refresh_token: body
            .refresh_token
            .clone()
            .or_else(|| prior.map(|p| p.refresh_token.clone()))
            .unwrap_or_default(),
        expires_at: Some(now + body.expires_in.unwrap_or(DEFAULT_TOKEN_TTL_SECS)),
        inference_base_url: body
            .inference_base_url
            .clone()
            .or_else(|| prior.and_then(|p| p.inference_base_url.clone())),
        agent_key: body
            .agent_key
            .clone()
            .or_else(|| prior.and_then(|p| p.agent_key.clone())),
        agent_key_expires_at: body.agent_key_expires_at.or_else(|| {
            body.agent_key_expires_in
                .map(|ttl| now + ttl)
                .or_else(|| prior.and_then(|p| p.agent_key_expires_at))
        }),
        scope: body
            .scope
            .clone()
            .or_else(|| prior.and_then(|p| p.scope.clone())),
    }
}

/// Host allowlist for Portal-supplied inference URLs. Permits the Nous
/// domains plus localhost for development; everything else is rejected
/// so a hostile token response can't exfiltrate the bearer.
fn validate_inference_url(url: &str) -> Result<String, AuthError> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|e| AuthError::OAuth(format!("invalid inference_base_url: {e}")))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| AuthError::OAuth("inference_base_url has no host".to_string()))?;
    let local = matches!(host, "localhost" | "127.0.0.1" | "::1");
    let nous = host == "nousresearch.com" || host.ends_with(".nousresearch.com");
    if !local && !nous {
        return Err(AuthError::OAuth(format!(
            "inference_base_url host {host} is not allowlisted"
        )));
    }
    let scheme_ok = parsed.scheme() == "https" || (local && parsed.scheme() == "http");
    if !scheme_ok {
        return Err(AuthError::OAuth(format!(
            "inference_base_url must be https (got {})",
            parsed.scheme()
        )));
    }
    Ok(url.trim_end_matches('/').to_string())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn remaining(deadline: Instant) -> u64 {
    deadline
        .saturating_duration_since(Instant::now())
        .as_secs()
        .max(1)
}

fn next_edge_backoff(current: u64, interval: u64) -> u64 {
    if current == 0 {
        (interval * 2).max(5)
    } else {
        (current * 2).min(EDGE_BACKOFF_CAP_SECS)
    }
}

fn retry_after_secs(resp: &reqwest::Response) -> Option<u64> {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

/// Sleep in ≤1s ticks so cancellation lands within a second.
async fn cancellable_sleep(dur: Duration, cancel: &CancellationToken) -> Result<(), AuthError> {
    let deadline = Instant::now() + dur;
    loop {
        if cancel.is_cancelled() {
            return Err(AuthError::OAuth("device login cancelled".to_string()));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        tokio::time::sleep(remaining.min(Duration::from_secs(1))).await;
    }
}
