//! Sensitive word filter models — DB row + request/response types.
//!
//! See `specs/010-sensitive-word-filter/data-model.md` for the full schema.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// DB row for a sensitive word entry.
#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct SensitiveWord {
    pub id: i64,
    pub word: String,
    pub match_mode: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ── Request types ──

#[derive(Debug, Deserialize)]
pub struct CreateSensitiveWordRequest {
    pub word: String,
    #[serde(default = "default_match_mode")]
    pub match_mode: String,
}

fn default_match_mode() -> String {
    "exact".to_string()
}

#[derive(Debug, Deserialize)]
pub struct UpdateSensitiveWordRequest {
    pub word: Option<String>,
    pub match_mode: Option<String>,
    pub enabled: Option<bool>,
}

// ── Response types ──

pub type SensitiveWordResponse = SensitiveWord;

#[derive(Debug, Serialize)]
pub struct SensitiveWordListResponse {
    pub total: u64,
    pub words: Vec<SensitiveWord>,
}
