//! Claude subscription OAuth: authorization-code + PKCE login and
//! refresh-token rotation against Anthropic's consumer OAuth endpoints.
//!
//! Wire facts (the same flow Claude Code uses — the client id below is
//! its public OAuth client):
//! - Authorize: `GET https://claude.ai/oauth/authorize` with
//!   `code=true`, `response_type=code`, the console callback as
//!   `redirect_uri`, Claude Code's current scope set, and an S256 PKCE
//!   challenge. `state` must be 43 chars (base64url of 32 bytes) or the
//!   page rejects the request with "Invalid request format".
//! - The callback page displays a code the user pastes back — often as
//!   `code#state`; both forms are accepted and `state` is verified.
//! - Exchange/refresh: `POST https://console.anthropic.com/v1/oauth/token`
//!   (JSON body, not form). Refresh tokens are **single-use** — the
//!   rotated pair is persisted *before* the access token is used, same
//!   rule as Portal.
//! - API calls then use `Authorization: Bearer` plus
//!   `anthropic-beta: oauth-2025-04-20` instead of `x-api-key` — the
//!   adapter branches on [`CredentialSource::is_oauth`].

use std::sync::Arc;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hate_core::provider::{BoxFuture, CredentialSource, ProviderError};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::store::{AuthError, AuthStore, CredentialBlob, OAuthTokens};

/// Public OAuth client id shared by Claude Code — identifies the app
/// in Anthropic's logs; no secret exists for this grant.
pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
/// Browser authorization endpoint.
pub const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
/// Code exchange + refresh endpoint.
pub const TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
/// Where the browser lands after approval; it displays the code to
/// paste back rather than redirecting to a local server.
pub const REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
/// Scopes requested at authorize time — matches Claude Code 2.1.x's own
/// set. The endpoint rejects requests whose scope list diverges from
/// the client's registered set.
pub const SCOPES: &str = "org:create_api_key user:profile user:inference \
user:sessions:claude_code user:mcp_servers user:file_upload";

/// `state` must be `base64url(32 random bytes)` (43 chars) like Claude
/// Code's own generator: `claude.ai/oauth/authorize` rejects shorter
/// values with `Invalid request format` even though RFC 6749 allows
/// them. This is an empirically confirmed server quirk.
const STATE_BYTES: usize = 32;

const REFRESH_SKEW_SECS: u64 = 60;
const DEFAULT_TOKEN_TTL_SECS: u64 = 8 * 3600;

/// Everything the caller needs between `begin_login` and
/// `complete_login`. Opaque to the UI — it round-trips through app
/// state, never through the page.
#[derive(Debug, Clone)]
pub struct ClaudePending {
    /// PKCE verifier — secret until the exchange.
    pub verifier: String,
    /// CSRF state echoed back by the callback page.
    pub state: String,
}

/// What `begin_login` hands back: the URL to open plus the pending
/// material to complete with.
#[derive(Debug, Clone)]
pub struct ClaudeLogin {
    /// Full authorize URL to open in the system browser.
    pub authorize_url: String,
    /// Pending PKCE/state material for [`ClaudeAuth::complete_login`].
    pub pending: ClaudePending,
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
}

/// Claude OAuth client bound to a credential store. `provider` is the
/// config/keychain name the token bundle lives under (`"claude"`).
pub struct ClaudeAuth {
    http: reqwest::Client,
    store: Arc<dyn AuthStore>,
    provider: String,
}

impl ClaudeAuth {
    /// Client storing credentials under `"claude"`.
    pub fn new(store: Arc<dyn AuthStore>) -> Self {
        Self::for_provider(store, "claude")
    }

    /// Client storing credentials under an explicit provider name
    /// (`"claude"` or `"anthropic"` — both resolve here).
    pub fn for_provider(store: Arc<dyn AuthStore>, provider: &str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("hate-agent/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            store,
            provider: provider.to_string(),
        }
    }

    /// Step 1: build the authorize URL + PKCE material. Purely local —
    /// no network until [`Self::complete_login`].
    ///
    /// Wire constraints (verified against live rejections):
    /// - `state` must be 43 chars (see [`STATE_BYTES`]) or the page
    ///   shows "Authorization failed / Invalid request format".
    /// - Query values use form encoding (`+` for spaces), matching
    ///   Claude Code's own URL builder.
    pub fn begin_login(&self) -> ClaudeLogin {
        let verifier = b64url(&rand_bytes(32));
        let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
        let state = b64url(&rand_bytes(STATE_BYTES));
        let url = format!(
            "{AUTHORIZE_URL}?code=true&client_id={CLIENT_ID}&response_type=code\
             &redirect_uri={}&scope={}&code_challenge={challenge}\
             &code_challenge_method=S256&state={state}",
            form_encoding(REDIRECT_URI),
            form_encoding(SCOPES),
        );
        ClaudeLogin {
            authorize_url: url,
            pending: ClaudePending { verifier, state },
        }
    }

    /// Step 2: exchange the pasted code for tokens and persist them.
    ///
    /// Accepts the bare code or the `code#state` form the callback page
    /// prints; a returned state that doesn't match aborts the exchange.
    ///
    /// # Errors
    /// [`AuthError::OAuth`] on state mismatch, transport, or rejection;
    /// [`AuthError::Store`] if persisting fails.
    pub async fn complete_login(
        &self,
        pending: &ClaudePending,
        pasted: &str,
    ) -> Result<CredentialBlob, AuthError> {
        let code = parse_pasted(pasted, &pending.state)?;

        let body = serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "state": pending.state,
            "client_id": CLIENT_ID,
            "redirect_uri": REDIRECT_URI,
            "code_verifier": pending.verifier,
        });
        let resp = self
            .http
            .post(TOKEN_URL)
            .json(&body)
            .send()
            .await
            .map_err(|e| AuthError::OAuth(format!("token exchange: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let detail = resp.text().await.unwrap_or_default();
            let snippet: String = detail.chars().take(200).collect();
            return Err(AuthError::OAuth(format!(
                "token exchange rejected (HTTP {status}): {snippet}"
            )));
        }
        let parsed: TokenResponse = resp
            .json()
            .await
            .map_err(|e| AuthError::OAuth(format!("token decode: {e}")))?;
        let tokens = tokens_from_response(&parsed, None);
        let blob = CredentialBlob::OAuth {
            tokens: tokens.clone(),
        };
        self.store.set(&self.provider, &blob)?;
        Ok(blob)
    }

    /// Load stored tokens, refreshing inside the skew window. The
    /// rotated refresh token is persisted *inside* the refresh call —
    /// callers must not delay the store.
    ///
    /// # Errors
    /// [`AuthError::NeedsLogin`] when no credential exists or the
    /// refresh token was rejected; [`AuthError::OAuth`]/
    /// [`AuthError::Store`] otherwise.
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

    /// Remove stored credentials (logout).
    ///
    /// # Errors
    /// [`AuthError::Store`] on keychain failure.
    pub fn logout(&self) -> Result<(), AuthError> {
        self.store.delete(&self.provider)
    }

    /// Refresh an expired/expiring access token. The rotated bundle is
    /// written to the store *before* returning — a crash between refresh
    /// and persist would orphan the session (single-use refresh tokens).
    async fn refresh(&self, tokens: &OAuthTokens) -> Result<OAuthTokens, AuthError> {
        let body = serde_json::json!({
            "client_id": CLIENT_ID,
            "grant_type": "refresh_token",
            "refresh_token": tokens.refresh_token,
        });
        let resp = self
            .http
            .post(TOKEN_URL)
            .json(&body)
            .send()
            .await
            .map_err(|e| AuthError::OAuth(format!("refresh request: {e}")))?;

        let status = resp.status().as_u16();
        if status == 200 {
            let parsed: TokenResponse = resp
                .json()
                .await
                .map_err(|e| AuthError::OAuth(format!("refresh decode: {e}")))?;
            // Carry forward fields the response omits — most importantly
            // the refresh token itself when it isn't rotated this call.
            let refreshed = tokens_from_response(&parsed, Some(tokens));
            let blob = CredentialBlob::OAuth {
                tokens: refreshed.clone(),
            };
            self.store.set(&self.provider, &blob)?;
            return Ok(refreshed);
        }

        if matches!(status, 400 | 401) {
            let detail = resp.text().await.unwrap_or_default();
            let snippet: String = detail.chars().take(200).collect();
            if snippet.contains("invalid_grant") {
                self.store.delete(&self.provider).ok();
                return Err(AuthError::NeedsLogin);
            }
            return Err(AuthError::OAuth(format!(
                "refresh rejected (HTTP {status}): {snippet}"
            )));
        }
        if status >= 500 {
            return Err(AuthError::OAuth(format!(
                "Anthropic OAuth temporarily unavailable (HTTP {status})"
            )));
        }
        Err(AuthError::OAuth(format!(
            "refresh failed with HTTP {status}"
        )))
    }
}

/// `CredentialSource` backed by Claude OAuth: bearer is the access
/// token; `is_oauth` tells the Anthropic adapter to send
/// `Authorization: Bearer` + the oauth beta flag instead of `x-api-key`.
pub struct ClaudeCredentialSource {
    auth: Arc<ClaudeAuth>,
}

impl ClaudeCredentialSource {
    /// Wrap a shared [`ClaudeAuth`].
    pub fn new(auth: Arc<ClaudeAuth>) -> Self {
        Self { auth }
    }
}

impl CredentialSource for ClaudeCredentialSource {
    fn bearer(&self) -> BoxFuture<'_, Result<String, ProviderError>> {
        let auth = Arc::clone(&self.auth);
        Box::pin(async move {
            let tokens = auth.tokens().await.map_err(|e| match e {
                AuthError::NeedsLogin => ProviderError::Auth(
                    "Claude login required — run `hate auth login claude`".into(),
                ),
                other => ProviderError::Auth(other.to_string()),
            })?;
            Ok(tokens.access_token)
        })
    }

    fn is_oauth(&self) -> bool {
        true
    }
}

/// Split the pasted `code` or `code#state` form and verify the state
/// echo. Returns the bare authorization code.
fn parse_pasted<'a>(pasted: &'a str, expected_state: &str) -> Result<&'a str, AuthError> {
    let (code, returned_state) = match pasted.split_once('#') {
        Some((c, s)) => (c.trim(), Some(s.trim())),
        None => (pasted.trim(), None),
    };
    if code.is_empty() {
        return Err(AuthError::OAuth("empty authorization code".to_string()));
    }
    if let Some(s) = returned_state
        && s != expected_state
    {
        return Err(AuthError::OAuth(
            "state mismatch — start the login again".to_string(),
        ));
    }
    Ok(code)
}

fn tokens_from_response(body: &TokenResponse, prior: Option<&OAuthTokens>) -> OAuthTokens {
    let now = now_secs();
    OAuthTokens {
        access_token: body.access_token.clone(),
        refresh_token: body
            .refresh_token
            .clone()
            .or_else(|| prior.map(|p| p.refresh_token.clone()))
            .unwrap_or_default(),
        expires_at: Some(now + body.expires_in.unwrap_or(DEFAULT_TOKEN_TTL_SECS)),
        inference_base_url: None,
        agent_key: None,
        agent_key_expires_at: None,
        scope: body
            .scope
            .clone()
            .or_else(|| prior.and_then(|p| p.scope.clone())),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn rand_bytes(n: usize) -> Vec<u8> {
    (0..n).map(|_| fastrand::u8(..)).collect()
}

fn b64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Form-encoding for query values (RFC 3986 unreserved set kept;
/// space → `+`, matching `URLSearchParams`/Claude Code). Only the
/// characters that actually appear need escaping.
fn form_encoding(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemoryStore;

    #[test]
    fn begin_login_builds_valid_authorize_url() {
        let auth = ClaudeAuth::new(Arc::new(MemoryStore::new()));
        let login = auth.begin_login();
        let url = &login.authorize_url;
        assert!(url.starts_with(AUTHORIZE_URL));
        for needle in [
            "code=true",
            "response_type=code",
            &format!("client_id={CLIENT_ID}"),
            "redirect_uri=https%3A%2F%2Fconsole.anthropic.com%2Foauth%2Fcode%2Fcallback",
            "code_challenge_method=S256",
            &format!("state={}", login.pending.state),
        ] {
            assert!(url.contains(needle), "url missing {needle}: {url}");
        }
        // Challenge must be the S256 of the verifier.
        let expected = b64url(&Sha256::digest(login.pending.verifier.as_bytes()));
        assert!(url.contains(&format!("code_challenge={expected}")));
        assert!(login.pending.verifier.len() >= 43);
        // claude.ai rejects shorter states with "Invalid request format".
        assert_eq!(
            login.pending.state.len(),
            43,
            "state must match Claude Code's 32-byte base64url form"
        );
        // Form encoding: scopes join with `+`, not `%20`.
        assert!(url.contains("scope=org%3Acreate_api_key+user%3Aprofile"));
    }

    #[test]
    fn parse_pasted_accepts_bare_and_matching_state() {
        assert_eq!(parse_pasted("abc123", "st").unwrap(), "abc123");
        assert_eq!(parse_pasted("  abc123  ", "st").unwrap(), "abc123");
        assert_eq!(parse_pasted("abc123#st", "st").unwrap(), "abc123");
        assert_eq!(parse_pasted("abc123#st ", "st").unwrap(), "abc123");
    }

    #[test]
    fn parse_pasted_rejects_mismatch_and_empty() {
        assert!(parse_pasted("abc#wrong", "st").is_err());
        assert!(parse_pasted("", "st").is_err());
        assert!(parse_pasted("   ", "st").is_err());
        assert!(parse_pasted("#st", "st").is_err());
    }
}
