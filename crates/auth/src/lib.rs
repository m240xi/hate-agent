//! hate-auth: where credentials live and how they refresh.
//!
//! API keys sit in the OS keychain (`keyring`); OAuth material (Nous
//! Portal) is stored as a single JSON blob per provider, also in the
//! keychain. `~/.config/hate-agent/config.toml` holds only non-secret
//! settings. Refresh tokens are single-use — every rotation is persisted
//! *before* the new pair is used, or the session gets revoked.

#![deny(unsafe_code)]

mod context;
mod keychain;
mod nous;
mod source;
mod store;

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
