use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::provider::{Message, ThinkingLevel};
use crate::types::TokenUsage;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedSession {
    pub key: String,
    pub mode: String,
    pub provider: String,
    pub model: String,
    pub thinking: ThinkingLevel,
    pub workspace: Option<String>,
    pub title: String,
    pub messages: Vec<Message>,
    pub turn: u32,
    pub usage: TokenUsage,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub mode: String,
    pub title: String,
    pub provider: String,
    pub model: String,
    pub workspace: Option<String>,
    pub updated_at: u32,
}

pub struct PersistCtx {
    pub key: String,
    pub mode: String,
    pub workspace: Option<String>,
    pub journal: Arc<dyn SessionJournal>,
    pub restore: Option<PersistedSession>,
}

pub trait SessionJournal: Send + Sync {
    fn load(&self, key: &str) -> Option<PersistedSession>;
    fn save(&self, snap: &PersistedSession);
    fn delete(&self, key: &str);
    fn list(&self, mode: Option<&str>) -> Vec<SessionMeta>;
}
