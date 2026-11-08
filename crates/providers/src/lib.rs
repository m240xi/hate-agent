//! hate-providers: wire adapters behind `hate_core::Provider`.
//!
//! Each adapter is one file, owns its vendor's request/response shapes,
//! and normalizes everything to `Delta` at the boundary. Nothing above
//! this crate knows which vendor is in use.

#![deny(unsafe_code)]

pub mod anthropic;
pub mod mock;
pub mod openai_compat;
pub mod retry;
mod sse;
mod wire;

pub use sse::SharedCredential;
pub use wire::{ProviderKind, ProviderSpec, build, warn_if_unknown};
