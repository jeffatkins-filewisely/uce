use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ContextBucket {
    Known,
    Candidate,
    Unknown,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Rule {
    pub id: String,
    pub app_keywords: Vec<String>,
    pub title_keywords: Vec<String>,
    /// When non-empty, every substring must appear in the window title (AND). Empty uses `title_keywords` (OR).
    #[serde(default)]
    pub title_keywords_all: Vec<String>,
    pub priority: u8,
    pub cooldown_secs: u64,
    /// Logical workflow: `ccc`, `tesla_epc`, `parts_trader`, `ops_trax`, `generic`, etc. (metadata + UI)
    #[serde(default)]
    pub workflow_kind: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ContextClassification {
    pub bucket: ContextBucket,
    pub rule_id: String,
    pub reason: String,
    pub cooldown_secs: u64,
    pub action_allowed: bool,
    pub rule_source: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WatchContext {
    pub source_app: String,
    pub window_title: String,
    pub matched: bool,
    pub matched_rule: String,
    /// Stable id: `ccc`, `tesla_epc`, `parts_trader`, `ops_trax`, `generic`, `unknown`, …
    pub workflow_kind: String,
    pub preferred_capture_mode: String,
    pub bucket: ContextBucket,
    pub action_allowed: bool,
    pub changed: bool,
    pub in_cooldown: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct RecentMemory {
    pub last_matched_rule: String,
    pub last_window_title: String,
    pub last_bucket: String,
    pub last_context_key: String,
    pub last_capture_time_unix_ms: Option<i64>,
    pub last_reaction_time_unix_ms: Option<i64>,
    pub last_in_cooldown: bool,
    pub candidate_counts: HashMap<String, u32>,
    pub approved_contexts_version: Option<String>,
    pub approved_contexts_last_loaded_unix_ms: Option<i64>,
}
