#![deny(unsafe_code)]

pub mod anthropic;
pub mod mock;
pub mod openai_compat;
pub mod retry;
mod sse;
mod wire;

pub use sse::SharedCredential;
pub use wire::{ProviderKind, ProviderSpec, build, warn_if_unknown};
