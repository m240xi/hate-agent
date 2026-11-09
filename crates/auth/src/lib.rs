//! hate-auth: where credentials live and how they refresh.
//!
//! API keys sit in the OS keychain (`keyring`); OAuth material (Nous
//! Portal) is stored as a single JSON blob per provider, also in the
//! keychain. `~/.config/hate-agent/config.toml` holds only non-secret
//! settings. Refresh tokens are single-use — every rotation is persisted
//! *before* the new pair is used, or the session gets revoked.

#![deny(unsafe_code)]

mod claude;
mod composite;
mod context;
mod file_store;
mod keychain;
mod nous;
mod source;
mod store;

pub use claude::{
    AUTHORIZE_URL as CLAUDE_AUTHORIZE_URL, CLIENT_ID as CLAUDE_CLIENT_ID, ClaudeAuth,
    ClaudeCredentialSource, ClaudeLogin, ClaudePending, REDIRECT_URI as CLAUDE_REDIRECT_URI,
    SCOPES as CLAUDE_SCOPES, TOKEN_URL as CLAUDE_TOKEN_URL,
};
pub use composite::CompositeStore;
pub use context::{AuthContext, CredStatus};
pub use keychain::KeyringStore;
pub use nous::{
    CLIENT_ID, DEFAULT_INFERENCE_URL, DEFAULT_PORTAL_URL, DEFAULT_WELCOME_URL, DeviceFlowInfo,
    NousCredentialSource, PortalAuth, SCOPE,
};
pub use source::{ApiKeySource, EnvKeySource};
pub use store::{
    AuthError, AuthStore, Config, CredentialBlob, MemoryStore, OAuthTokens, ProviderConfig,
};
