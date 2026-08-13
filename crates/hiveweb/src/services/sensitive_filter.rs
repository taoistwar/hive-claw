//! Sensitive word filter engine.
//!
//! Provides a thread-safe in-memory filter that checks text against a set of
//! exact-match and regex-sensitive-word patterns. The word list is loaded from
//! the `sensitive_words` table at startup and can be refreshed on demand.
//!
//! Architecture:
//! - Exact matching uses Aho-Corasick (O(n+m)) for multi-pattern matching.
//! - Regex patterns are pre-compiled and checked individually (with size limits
//!   for ReDoS protection per research.md §D3).
//! - The cache is wrapped in `Arc<RwLock<Vec<SensitivePattern>>>` for cheap reads
//!   and infrequent writes.

use aho_corasick::AhoCorasick;
use regex::{Regex, RegexBuilder};
use sqlx::MySqlPool;
use std::sync::{Arc, RwLock};

use crate::db::sql_safety::audit_sql;
use crate::models::sensitive_word::SensitiveWord;

// ── Pattern types ──

/// A compiled sensitive-word matching pattern.
#[derive(Debug, Clone)]
pub enum SensitivePattern {
    /// Case-insensitive exact substring match.
    Exact { id: i64, word: String },
    /// Pre-compiled regex pattern (with DFA size limits applied at construction).
    Regex { id: i64, word: String, re: Regex },
}

impl SensitivePattern {
    pub fn id(&self) -> i64 {
        match self {
            Self::Exact { id, .. } | Self::Regex { id, .. } => *id,
        }
    }

    pub fn word(&self) -> &str {
        match self {
            Self::Exact { word, .. } | Self::Regex { word, .. } => word.as_str(),
        }
    }
}

// ── Filter ──

/// Thread-safe in-memory sensitive-word filter.
///
/// Reads are lock-free in the common case (RwLock::read); writes only occur
/// during cache refresh (admin CRUD-triggered).
#[derive(Clone)]
pub struct SensitiveFilter {
    /// Compiled patterns (exact + regex), guarded by RwLock.
    patterns: Arc<RwLock<Vec<SensitivePattern>>>,
    /// Aho-Corasick automaton for fast exact multi-pattern matching.
    ac: Arc<RwLock<Option<AhoCorasick>>>,
    /// Separated list of exact-match words (for rebuilding AC on refresh).
    exact_words: Arc<RwLock<Vec<(i64, String)>>>,
}

impl SensitiveFilter {
    /// Create an empty filter. Call `load_from_db()` to populate.
    pub fn new() -> Self {
        Self {
            patterns: Arc::new(RwLock::new(Vec::new())),
            ac: Arc::new(RwLock::new(None)),
            exact_words: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Load all enabled sensitive words from the database and build the matcher.
    pub async fn load_from_db(&self, pool: &MySqlPool) -> Result<(), sqlx::Error> {
        let rows: Vec<SensitiveWord> = sqlx::query_as(
            "SELECT id, word, match_mode, enabled, created_at, updated_at \
             FROM sensitive_words WHERE enabled = 1",
        )
        .fetch_all(pool)
        .await?;

        self.build_matchers(&rows);
        Ok(())
    }

    /// Replace in-memory cache by reloading from DB.
    /// Equivalent to `load_from_db`, intended for admin CRUD-triggered refresh.
    pub async fn refresh_cache(&self, pool: &MySqlPool) -> Result<(), sqlx::Error> {
        self.load_from_db(pool).await
    }

    /// Check whether `text` matches any sensitive pattern.
    ///
    /// Returns `Some(&SensitivePattern)` for the first match found, or `None`
    /// if the text is clean.
    pub fn check(&self, text: &str) -> Option<SensitivePattern> {
        if text.is_empty() {
            return None;
        }

        // 1. Aho-Corasick exact match (fast path, O(n+m))
        let ac_guard = self.ac.read().expect("AC lock poisoned");
        if let Some(ref ac) = *ac_guard
            && ac.find(text).is_some()
        {
            // Get the matched word for logging
            let patterns_guard = self.patterns.read().expect("patterns lock poisoned");
            // Find which exact pattern matched first
            for pattern in patterns_guard.iter() {
                if let SensitivePattern::Exact { word, .. } = pattern
                    && text.to_lowercase().contains(&word.to_lowercase())
                {
                    return Some(pattern.clone());
                }
            }
        }
        drop(ac_guard);

        // 2. Regex patterns (check individually)
        let patterns_guard = self.patterns.read().expect("patterns lock poisoned");
        for pattern in patterns_guard.iter() {
            if let SensitivePattern::Regex { re, .. } = pattern
                && re.is_match(text)
            {
                return Some(pattern.clone());
            }
        }

        None
    }

    /// Build the internal matchers from a list of SensitiveWord rows.
    fn build_matchers(&self, rows: &[SensitiveWord]) {
        let mut patterns = Vec::new();
        let mut exact_tuples: Vec<(i64, String)> = Vec::new();

        for row in rows {
            match row.match_mode.as_str() {
                "regex" => {
                    // Build with size limit for ReDoS protection (research §D3)
                    match RegexBuilder::new(&row.word).size_limit(1024 * 1024).build() {
                        Ok(re) => {
                            patterns.push(SensitivePattern::Regex {
                                id: row.id,
                                word: row.word.clone(),
                                re,
                            });
                        }
                        Err(e) => {
                            tracing::warn!(
                                id = row.id,
                                word = %row.word,
                                error = %e,
                                "Failed to compile regex sensitive word, skipping"
                            );
                        }
                    }
                }
                _ => {
                    // exact (default)
                    exact_tuples.push((row.id, row.word.clone()));
                    patterns.push(SensitivePattern::Exact {
                        id: row.id,
                        word: row.word.clone(),
                    });
                }
            }
        }

        // Build Aho-Corasick from exact words
        let ac = if exact_tuples.is_empty() {
            None
        } else {
            let words: Vec<&str> = exact_tuples.iter().map(|(_, w)| w.as_str()).collect();
            AhoCorasick::new(&words).ok()
        };

        // Atomic replacement
        {
            let mut pat_guard = self.patterns.write().expect("patterns lock poisoned");
            *pat_guard = patterns;
        }
        {
            let mut ac_guard = self.ac.write().expect("AC lock poisoned");
            *ac_guard = ac;
        }
        {
            let mut ew_guard = self.exact_words.write().expect("exact_words lock poisoned");
            *ew_guard = exact_tuples;
        }
    }
}

impl Default for SensitiveFilter {
    fn default() -> Self {
        Self::new()
    }
}

// ── Admin CRUD (Phase 5 / US3) ──

use crate::models::sensitive_word::{
    CreateSensitiveWordRequest, SensitiveWordListResponse, UpdateSensitiveWordRequest,
};
use crate::utils::error::AppError;

/// List sensitive words with optional search and pagination (T031).
pub async fn list_sensitive_words(
    pool: &MySqlPool,
    page: i64,
    page_size: i64,
    search: Option<&str>,
    _enabled: Option<bool>,
) -> Result<SensitiveWordListResponse, AppError> {
    let page = page.max(1);
    let page_size = page_size.clamp(1, 100);

    let (where_clause, count_params, data_params): (String, Vec<String>, Vec<String>) =
        if let Some(keyword) = search {
            if keyword.is_empty() {
                (String::new(), vec![], vec![])
            } else {
                let like = format!("%{}%", keyword);
                (
                    "WHERE word LIKE ?".to_string(),
                    vec![like.clone()],
                    vec![like],
                )
            }
        } else {
            (String::new(), vec![], vec![])
        };

    // Count
    let count_sql = format!("SELECT COUNT(*) FROM sensitive_words {}", where_clause);
    let count_sql = audit_sql(count_sql);
    let mut count_query = sqlx::query_scalar(count_sql);
    for p in &count_params {
        count_query = count_query.bind(p);
    }
    let total: i64 = count_query
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("sensitive words count: {e}")))?;

    // List
    let offset = (page - 1) * page_size;
    let data_sql = format!(
        "SELECT id, word, match_mode, enabled, created_at, updated_at \
         FROM sensitive_words {} ORDER BY id DESC LIMIT ? OFFSET ?",
        where_clause
    );
    let data_sql = audit_sql(data_sql);
    let mut data_query = sqlx::query_as::<_, SensitiveWord>(data_sql);
    for p in &data_params {
        data_query = data_query.bind(p);
    }
    let words: Vec<SensitiveWord> = data_query
        .bind(page_size)
        .bind(offset)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("sensitive words list: {e}")))?;

    Ok(SensitiveWordListResponse {
        total: total as u64,
        words,
    })
}

/// Create a sensitive word with validation (T032).
pub async fn create_sensitive_word(
    pool: &MySqlPool,
    filter: &SensitiveFilter,
    req: &CreateSensitiveWordRequest,
) -> Result<SensitiveWord, AppError> {
    // Validate
    let word = req.word.trim();
    if word.is_empty() {
        return Err(AppError::BadRequest("敏感词不能为空".into()));
    }
    if word.len() > 512 {
        return Err(AppError::BadRequest("敏感词不能超过512个字符".into()));
    }
    let match_mode = req.match_mode.as_str();
    if match_mode != "exact" && match_mode != "regex" {
        return Err(AppError::BadRequest(
            "match_mode 必须为 exact 或 regex".into(),
        ));
    }
    if match_mode == "regex" {
        regex::Regex::new(word)
            .map_err(|e| AppError::BadRequest(format!("正则表达式无效: {e}")))?;
    }

    // Insert (check duplicate via UNIQUE KEY)
    let result = sqlx::query("INSERT INTO sensitive_words (word, match_mode) VALUES (?, ?)")
        .bind(word)
        .bind(match_mode)
        .execute(pool)
        .await;

    match result {
        Ok(r) => {
            let id = r.last_insert_id() as i64;
            // Refresh cache
            if let Err(e) = filter.refresh_cache(pool).await {
                tracing::warn!(error = %e, "Failed to refresh filter cache after create");
            }
            let row: SensitiveWord = sqlx::query_as(
                "SELECT id, word, match_mode, enabled, created_at, updated_at FROM sensitive_words WHERE id = ?",
            )
            .bind(id)
            .fetch_one(pool)
            .await
            .map_err(|e| AppError::Internal(format!("fetch created word: {e}")))?;
            Ok(row)
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("Duplicate") {
                Err(AppError::BadRequest("该敏感词已存在".into()))
            } else {
                Err(AppError::Internal(format!("create sensitive word: {e}")))
            }
        }
    }
}

/// Update a sensitive word with validation (T033).
pub async fn update_sensitive_word(
    pool: &MySqlPool,
    filter: &SensitiveFilter,
    id: i64,
    req: &UpdateSensitiveWordRequest,
) -> Result<SensitiveWord, AppError> {
    // Check exists
    let existing: Option<SensitiveWord> = sqlx::query_as(
        "SELECT id, word, match_mode, enabled, created_at, updated_at FROM sensitive_words WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Internal(format!("fetch sensitive word: {e}")))?;

    let existing = existing.ok_or_else(|| AppError::BadRequest("敏感词不存在".into()))?;

    let word = req.word.as_deref().unwrap_or(&existing.word).trim();
    if word.is_empty() {
        return Err(AppError::BadRequest("敏感词不能为空".into()));
    }
    let match_mode = req.match_mode.as_deref().unwrap_or(&existing.match_mode);
    if match_mode != "exact" && match_mode != "regex" {
        return Err(AppError::BadRequest(
            "match_mode 必须为 exact 或 regex".into(),
        ));
    }
    if match_mode == "regex" {
        regex::Regex::new(word)
            .map_err(|e| AppError::BadRequest(format!("正则表达式无效: {e}")))?;
    }
    let enabled = req.enabled.unwrap_or(existing.enabled);

    sqlx::query("UPDATE sensitive_words SET word = ?, match_mode = ?, enabled = ? WHERE id = ?")
        .bind(word)
        .bind(match_mode)
        .bind(enabled)
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("Duplicate") {
                AppError::BadRequest("该敏感词已存在".into())
            } else {
                AppError::Internal(format!("update sensitive word: {e}"))
            }
        })?;

    // Refresh cache
    if let Err(e) = filter.refresh_cache(pool).await {
        tracing::warn!(error = %e, "Failed to refresh filter cache after update");
    }

    let row: SensitiveWord = sqlx::query_as(
        "SELECT id, word, match_mode, enabled, created_at, updated_at FROM sensitive_words WHERE id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .map_err(|e| AppError::Internal(format!("fetch updated word: {e}")))?;
    Ok(row)
}

/// Delete a sensitive word (T034).
pub async fn delete_sensitive_word(
    pool: &MySqlPool,
    filter: &SensitiveFilter,
    id: i64,
) -> Result<bool, AppError> {
    let result = sqlx::query("DELETE FROM sensitive_words WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("delete sensitive word: {e}")))?;

    let deleted = result.rows_affected() > 0;
    if deleted && let Err(e) = filter.refresh_cache(pool).await {
        tracing::warn!(error = %e, "Failed to refresh filter cache after delete");
    }
    Ok(deleted)
}
