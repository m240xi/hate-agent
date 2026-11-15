#![deny(unsafe_code)]

pub mod agent;
pub mod rpc;
pub mod runtime;
pub mod session;

pub use agent::{Agent, ModelChoice, StartError};
pub use runtime::{Installed, platform_key};
pub use session::{AcpSessionConfig, SIGN_IN_HINT, mcp_servers, spawn_acp_session};
