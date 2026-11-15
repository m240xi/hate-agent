#![deny(unsafe_code)]

mod claude;
mod composite;
mod context;
mod file_store;
mod keychain;
mod nous;
pub mod presets;
mod source;
mod store;

pub use claude::{
    AUTHORIZE_URL as CLAUDE_AUTHORIZE_URL, CLIENT_ID as CLAUDE_CLIENT_ID, ClaudeAuth,
    ClaudeCredentialSource, ClaudeLogin, ClaudePending, REDIRECT_URI as CLAUDE_REDIRECT_URI,
    SCOPES as CLAUDE_SCOPES, TOKEN_URL as CLAUDE_TOKEN_URL,
};
pub use composite::CompositeStore;
pub use context::{AuthContext, CredStatus, env_key_present, env_vars_for};
pub use keychain::KeyringStore;
pub use nous::{
    CLIENT_ID, DEFAULT_INFERENCE_URL, DEFAULT_PORTAL_URL, DEFAULT_WELCOME_URL, DeviceFlowInfo,
    NousCredentialSource, PortalAuth, SCOPE,
};
pub use source::{ApiKeySource, EnvKeySource};
pub use store::{
    AuthError, AuthStore, Config, CredentialBlob, MemoryStore, OAuthTokens, ProviderConfig,
};
