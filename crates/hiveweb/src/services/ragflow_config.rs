//! RAGFlow configuration resolution.
//!
//! Values are loaded into process-global state in this order: `global_configs`,
//! the process environment populated from `.env`, then the centralized defaults
//! in this module. Database reads are limited to startup and cross-instance
//! change/reconnect synchronization; normal RAGFlow requests only read memory.

use std::collections::HashMap;
use std::env;
use std::sync::{LazyLock, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use futures::StreamExt;
use redis::RedisResult;
use serde_json::{json, Value};
use sqlx::MySqlPool;
use tokio::time::sleep;

use crate::cache::redis::RedisClient;
use crate::utils::error::AppError;

const RAGFLOW_CONFIG_CHANNEL: &str = "hiveweb:global-config:ragflow";
const PUBSUB_RECONNECT_DELAY: Duration = Duration::from_secs(1);
static INSTANCE_ID: LazyLock<String> = LazyLock::new(|| uuid::Uuid::new_v4().to_string());

const GLOBAL_RAGFLOW_BASE_URL: &str = "ragflow_base_url";
const GLOBAL_RAGFLOW_API_KEY: &str = "ragflow_api_key";
const GLOBAL_RAGFLOW_DATASET_IDS: &str = "ragflow_dataset_ids";
const GLOBAL_RAGFLOW_DOCUMENT_IDS: &str = "ragflow_document_ids";
const GLOBAL_RAGFLOW_PAGE: &str = "ragflow_page";
const GLOBAL_RAGFLOW_PAGE_SIZE: &str = "ragflow_page_size";
const GLOBAL_RAGFLOW_SIMILARITY_THRESHOLD: &str = "ragflow_similarity_threshold";
const GLOBAL_RAGFLOW_VECTOR_SIMILARITY_WEIGHT: &str = "ragflow_vector_similarity_weight";
const GLOBAL_RAGFLOW_TOP_K: &str = "ragflow_top_k";
const GLOBAL_RAGFLOW_RERANK_ID: &str = "ragflow_rerank_id";
const GLOBAL_RAGFLOW_KEYWORD: &str = "ragflow_keyword";
const GLOBAL_RAGFLOW_HIGHLIGHT: &str = "ragflow_highlight";
const GLOBAL_RAGFLOW_TIMEOUT_SECS: &str = "ragflow_timeout_secs";

const ENV_RAGFLOW_BASE_URL: &str = "RAGFLOW_BASE_URL";
const ENV_RAGFLOW_API_KEY: &str = "RAGFLOW_API_KEY";
const ENV_RAGFLOW_DATASET_IDS: &str = "RAGFLOW_DATASET_IDS";
const ENV_RAGFLOW_DOCUMENT_IDS: &str = "RAGFLOW_DOCUMENT_IDS";
const ENV_RAGFLOW_PAGE: &str = "RAGFLOW_PAGE";
const ENV_RAGFLOW_PAGE_SIZE: &str = "RAGFLOW_PAGE_SIZE";
const ENV_RAGFLOW_SIMILARITY_THRESHOLD: &str = "RAGFLOW_SIMILARITY_THRESHOLD";
const ENV_RAGFLOW_VECTOR_SIMILARITY_WEIGHT: &str = "RAGFLOW_VECTOR_SIMILARITY_WEIGHT";
const ENV_RAGFLOW_TOP_K: &str = "RAGFLOW_TOP_K";
const ENV_RAGFLOW_RERANK_ID: &str = "RAGFLOW_RERANK_ID";
const ENV_RAGFLOW_KEYWORD: &str = "RAGFLOW_KEYWORD";
const ENV_RAGFLOW_HIGHLIGHT: &str = "RAGFLOW_HIGHLIGHT";
const ENV_RAGFLOW_TIMEOUT_SECS: &str = "RAGFLOW_TIMEOUT_SECS";

const DEFAULT_RAGFLOW_BASE_URL: &str = "";
const DEFAULT_RAGFLOW_API_KEY: &str = "";
const DEFAULT_RAGFLOW_DATASET_IDS: &str = "";
const DEFAULT_RAGFLOW_DOCUMENT_IDS: &str = "";
const DEFAULT_RAGFLOW_PAGE: u32 = 1;
const DEFAULT_RAGFLOW_PAGE_SIZE: u32 = 6;
const DEFAULT_RAGFLOW_SIMILARITY_THRESHOLD: f32 = 0.2;
const DEFAULT_RAGFLOW_VECTOR_SIMILARITY_WEIGHT: f32 = 0.3;
const DEFAULT_RAGFLOW_TOP_K: u32 = 10;
const DEFAULT_RAGFLOW_RERANK_ID: Option<&str> = None;
const DEFAULT_RAGFLOW_KEYWORD: bool = true;
const DEFAULT_RAGFLOW_HIGHLIGHT: bool = false;
const DEFAULT_RAGFLOW_TIMEOUT_SECS: u64 = 30;

const GLOBAL_RAGFLOW_KEYS: &[&str] = &[
    GLOBAL_RAGFLOW_BASE_URL,
    GLOBAL_RAGFLOW_API_KEY,
    GLOBAL_RAGFLOW_DATASET_IDS,
    GLOBAL_RAGFLOW_DOCUMENT_IDS,
    GLOBAL_RAGFLOW_PAGE,
    GLOBAL_RAGFLOW_PAGE_SIZE,
    GLOBAL_RAGFLOW_SIMILARITY_THRESHOLD,
    GLOBAL_RAGFLOW_VECTOR_SIMILARITY_WEIGHT,
    GLOBAL_RAGFLOW_TOP_K,
    GLOBAL_RAGFLOW_RERANK_ID,
    GLOBAL_RAGFLOW_KEYWORD,
    GLOBAL_RAGFLOW_HIGHLIGHT,
    GLOBAL_RAGFLOW_TIMEOUT_SECS,
];

/// Display name per RAGFlow global config key, shown in the web-admin UI.
const GLOBAL_RAGFLOW_NAMES: &[(&str, &str)] = &[
    (GLOBAL_RAGFLOW_BASE_URL, "RAGFlow Base URL"),
    (GLOBAL_RAGFLOW_API_KEY, "RAGFlow API Key"),
    (GLOBAL_RAGFLOW_DATASET_IDS, "RAGFlow Dataset IDs"),
    (GLOBAL_RAGFLOW_DOCUMENT_IDS, "RAGFlow Document IDs"),
    (GLOBAL_RAGFLOW_PAGE, "RAGFlow Page"),
    (GLOBAL_RAGFLOW_PAGE_SIZE, "RAGFlow Page Size"),
    (GLOBAL_RAGFLOW_SIMILARITY_THRESHOLD, "RAGFlow Similarity Threshold"),
    (GLOBAL_RAGFLOW_VECTOR_SIMILARITY_WEIGHT, "RAGFlow Vector Similarity Weight"),
    (GLOBAL_RAGFLOW_TOP_K, "RAGFlow Top K"),
    (GLOBAL_RAGFLOW_RERANK_ID, "RAGFlow Rerank ID"),
    (GLOBAL_RAGFLOW_KEYWORD, "RAGFlow Keyword"),
    (GLOBAL_RAGFLOW_HIGHLIGHT, "RAGFlow Highlight"),
    (GLOBAL_RAGFLOW_TIMEOUT_SECS, "RAGFlow Timeout Secs"),
];

/// A seed row for a RAGFlow global config key: (key, name, type, value).
type RagflowSeedRow<'a> = (&'a str, &'a str, &'a str, Value);

/// Builds the default seed rows for every RAGFlow global config key, mirroring
/// `RagflowConfig::default()`. Values are wrapped as `{ "value": ... }` to match
/// the shape written by the web-admin create API.
fn ragflow_seed_rows() -> Vec<RagflowSeedRow<'static>> {
    let rows = [
        (
            GLOBAL_RAGFLOW_BASE_URL,
            "string",
            json!(DEFAULT_RAGFLOW_BASE_URL),
        ),
        (
            GLOBAL_RAGFLOW_API_KEY,
            "string",
            json!(DEFAULT_RAGFLOW_API_KEY),
        ),
        (
            GLOBAL_RAGFLOW_DATASET_IDS,
            "json",
            json!(split_csv(DEFAULT_RAGFLOW_DATASET_IDS)),
        ),
        (
            GLOBAL_RAGFLOW_DOCUMENT_IDS,
            "json",
            json!(split_csv(DEFAULT_RAGFLOW_DOCUMENT_IDS)),
        ),
        (GLOBAL_RAGFLOW_PAGE, "number", json!(DEFAULT_RAGFLOW_PAGE)),
        (
            GLOBAL_RAGFLOW_PAGE_SIZE,
            "number",
            json!(DEFAULT_RAGFLOW_PAGE_SIZE),
        ),
        (
            GLOBAL_RAGFLOW_SIMILARITY_THRESHOLD,
            "number",
            json!(DEFAULT_RAGFLOW_SIMILARITY_THRESHOLD),
        ),
        (
            GLOBAL_RAGFLOW_VECTOR_SIMILARITY_WEIGHT,
            "number",
            json!(DEFAULT_RAGFLOW_VECTOR_SIMILARITY_WEIGHT),
        ),
        (GLOBAL_RAGFLOW_TOP_K, "number", json!(DEFAULT_RAGFLOW_TOP_K)),
        (
            GLOBAL_RAGFLOW_RERANK_ID,
            "string",
            json!(DEFAULT_RAGFLOW_RERANK_ID),
        ),
        (
            GLOBAL_RAGFLOW_KEYWORD,
            "boolean",
            json!(DEFAULT_RAGFLOW_KEYWORD),
        ),
        (
            GLOBAL_RAGFLOW_HIGHLIGHT,
            "boolean",
            json!(DEFAULT_RAGFLOW_HIGHLIGHT),
        ),
        (
            GLOBAL_RAGFLOW_TIMEOUT_SECS,
            "number",
            json!(DEFAULT_RAGFLOW_TIMEOUT_SECS),
        ),
    ];
    rows.into_iter()
        .map(|(key, type_, value)| {
            let name = GLOBAL_RAGFLOW_NAMES
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, name)| *name)
                .unwrap_or(key);
            (key, name, type_, json!({ "value": value }))
        })
        .collect()
}

/// Fully resolved RAGFlow settings used to build a retrieval request.
#[derive(Debug, Clone, PartialEq)]
pub struct RagflowConfig {
    pub base_url: String,
    pub api_key: String,
    pub dataset_ids: Vec<String>,
    pub document_ids: Vec<String>,
    pub page: u32,
    pub page_size: u32,
    pub similarity_threshold: f32,
    pub vector_similarity_weight: f32,
    pub top_k: u32,
    pub rerank_id: Option<String>,
    pub keyword: bool,
    pub highlight: bool,
    pub timeout_secs: u64,
}

#[derive(Debug)]
struct RagflowConfigState {
    global_values: HashMap<String, Value>,
    resolved: RagflowConfig,
}

impl RagflowConfigState {
    fn from_sources(
        global_values: HashMap<String, Value>,
        env_lookup: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let resolved = resolve_ragflow_config(&global_values, env_lookup);
        Self {
            global_values,
            resolved,
        }
    }

    fn upsert(&mut self, key: &str, data: Value, env_lookup: impl Fn(&str) -> Option<String>) {
        let Some(key) = normalize_global_key(key) else {
            return;
        };
        self.global_values.insert(key, data);
        self.resolved = resolve_ragflow_config(&self.global_values, env_lookup);
    }

    fn remove(&mut self, key: &str, env_lookup: impl Fn(&str) -> Option<String>) {
        let Some(key) = normalize_global_key(key) else {
            return;
        };
        self.global_values.remove(&key);
        self.resolved = resolve_ragflow_config(&self.global_values, env_lookup);
    }
}

static RAGFLOW_CONFIG_STATE: LazyLock<RwLock<RagflowConfigState>> = LazyLock::new(|| {
    RwLock::new(RagflowConfigState::from_sources(HashMap::new(), |key| {
        env::var(key).ok()
    }))
});

impl Default for RagflowConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_RAGFLOW_BASE_URL.to_string(),
            api_key: DEFAULT_RAGFLOW_API_KEY.to_string(),
            dataset_ids: split_csv(DEFAULT_RAGFLOW_DATASET_IDS),
            document_ids: split_csv(DEFAULT_RAGFLOW_DOCUMENT_IDS),
            page: DEFAULT_RAGFLOW_PAGE,
            page_size: DEFAULT_RAGFLOW_PAGE_SIZE,
            similarity_threshold: DEFAULT_RAGFLOW_SIMILARITY_THRESHOLD,
            vector_similarity_weight: DEFAULT_RAGFLOW_VECTOR_SIMILARITY_WEIGHT,
            top_k: DEFAULT_RAGFLOW_TOP_K,
            rerank_id: DEFAULT_RAGFLOW_RERANK_ID.map(str::to_string),
            keyword: DEFAULT_RAGFLOW_KEYWORD,
            highlight: DEFAULT_RAGFLOW_HIGHLIGHT,
            timeout_secs: DEFAULT_RAGFLOW_TIMEOUT_SECS,
        }
    }
}

impl RagflowConfig {
    /// Returns a snapshot of the current process-global RAGFlow configuration.
    pub fn current() -> Self {
        read_state().resolved.clone()
    }
}

/// Ensures every RAGFlow global config key exists in the database, inserting the
/// centralized defaults (`RagflowConfig::default`) for any key that is missing.
/// Existing rows are never overwritten, so web-admin edits are preserved.
async fn ensure_global_keys(pool: &MySqlPool) {
    let existing = match crate::services::global_config::fetch_values_by_keys(pool, GLOBAL_RAGFLOW_KEYS).await {
        Ok(values) => values,
        Err(error) => {
            tracing::warn!(
                error = %error,
                "check RAGFlow global config keys failed; skipping seed"
            );
            return;
        }
    };

    let missing: Vec<_> = ragflow_seed_rows()
        .into_iter()
        .filter(|(key, _, _, _)| !existing.contains_key(*key))
        .collect();

    if missing.is_empty() {
        return;
    }

    for (key, name, type_, data) in missing {
        if let Err(error) = crate::services::global_config::ensure_key(pool, name, key, type_, &data).await {
            tracing::warn!(
                error = %error,
                config_key = key,
                "seed RAGFlow global config key failed"
            );
        }
    }
    tracing::info!(seeded = GLOBAL_RAGFLOW_KEYS.len() - existing.len(), "RAGFlow global config keys seeded");
}

/// Loads or reloads global RAGFlow overrides from the database into memory.
pub async fn initialize(pool: &MySqlPool) {
    ensure_global_keys(pool).await;
    match load_global_values(pool).await {
        Ok(global_values) => {
            let override_count = global_values.len();
            *write_state() =
                RagflowConfigState::from_sources(global_values, |key| env::var(key).ok());
            tracing::info!(override_count, "RAGFlow global config loaded");
        }
        Err(error) => {
            tracing::warn!(
                error = %error,
                "load RAGFlow global config failed; retaining current process state"
            );
        }
    }
}

/// Synchronizes a web-admin create/update locally and notifies other instances.
pub async fn sync_global_value(redis: &RedisClient, key: &str, data: &Value) {
    if normalize_global_key(key).is_none() {
        return;
    }
    write_state().upsert(key, data.clone(), |env_key| env::var(env_key).ok());
    tracing::info!(config_key = key, "RAGFlow global config synchronized");
    publish_change(redis).await;
}

/// Removes a web-admin override locally and notifies other instances.
pub async fn remove_global_value(redis: &RedisClient, key: &str) {
    if normalize_global_key(key).is_none() {
        return;
    }
    write_state().remove(key, |env_key| env::var(env_key).ok());
    tracing::info!(config_key = key, "RAGFlow global config override removed");
    publish_change(redis).await;
}

/// Starts the reconnecting cross-instance RAGFlow configuration subscriber.
pub fn spawn_subscriber(redis: RedisClient, pool: MySqlPool) {
    tokio::spawn(async move {
        loop {
            if let Err(error) = subscribe_once(&redis, &pool).await {
                tracing::warn!(
                    error = %error,
                    retry_after_ms = PUBSUB_RECONNECT_DELAY.as_millis(),
                    "RAGFlow config Pub/Sub subscriber disconnected"
                );
            }
            sleep(PUBSUB_RECONNECT_DELAY).await;
        }
    });
}

async fn publish_change(redis: &RedisClient) {
    let result = async {
        let mut connection = redis.get_multiplexed_async_connection().await?;
        redis::cmd("PUBLISH")
            .arg(RAGFLOW_CONFIG_CHANNEL)
            .arg(INSTANCE_ID.as_str())
            .query_async::<_, i64>(&mut connection)
            .await
    }
    .await;

    match result {
        Ok(subscriber_count) => tracing::debug!(
            subscriber_count,
            "RAGFlow config change notification published"
        ),
        Err(error) => tracing::error!(
            error = %error,
            "publish RAGFlow config change notification failed"
        ),
    }
}

async fn subscribe_once(redis: &RedisClient, pool: &MySqlPool) -> RedisResult<()> {
    let mut pubsub = redis.get_async_pubsub().await?;
    pubsub.subscribe(RAGFLOW_CONFIG_CHANNEL).await?;

    // Subscription is active before the reload, closing the load/subscribe race.
    initialize(pool).await;
    tracing::info!(
        channel = RAGFLOW_CONFIG_CHANNEL,
        "RAGFlow config subscriber ready"
    );

    let mut messages = pubsub.on_message();
    while let Some(message) = messages.next().await {
        let source_instance = message.get_payload::<String>().unwrap_or_default();
        if should_reload_from_notification(&source_instance, INSTANCE_ID.as_str()) {
            initialize(pool).await;
        }
    }
    Ok(())
}

fn should_reload_from_notification(source_instance: &str, local_instance: &str) -> bool {
    source_instance != local_instance
}

fn read_state() -> RwLockReadGuard<'static, RagflowConfigState> {
    RAGFLOW_CONFIG_STATE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_state() -> RwLockWriteGuard<'static, RagflowConfigState> {
    RAGFLOW_CONFIG_STATE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn normalize_global_key(key: &str) -> Option<String> {
    let normalized = key.to_ascii_lowercase();
    GLOBAL_RAGFLOW_KEYS
        .contains(&normalized.as_str())
        .then_some(normalized)
}

async fn load_global_values(pool: &MySqlPool) -> Result<HashMap<String, Value>, AppError> {
    let values =
        crate::services::global_config::fetch_values_by_keys(pool, GLOBAL_RAGFLOW_KEYS).await?;
    Ok(values
        .into_iter()
        .map(|(key, data)| (key.to_ascii_lowercase(), data))
        .collect())
}

fn resolve_ragflow_config(
    global: &HashMap<String, Value>,
    env_lookup: impl Fn(&str) -> Option<String>,
) -> RagflowConfig {
    RagflowConfig {
        base_url: resolve_string(
            global,
            GLOBAL_RAGFLOW_BASE_URL,
            ENV_RAGFLOW_BASE_URL,
            &env_lookup,
            DEFAULT_RAGFLOW_BASE_URL,
        ),
        api_key: resolve_string(
            global,
            GLOBAL_RAGFLOW_API_KEY,
            ENV_RAGFLOW_API_KEY,
            &env_lookup,
            DEFAULT_RAGFLOW_API_KEY,
        ),
        dataset_ids: resolve_list(
            global,
            GLOBAL_RAGFLOW_DATASET_IDS,
            ENV_RAGFLOW_DATASET_IDS,
            &env_lookup,
            DEFAULT_RAGFLOW_DATASET_IDS,
        ),
        document_ids: resolve_list(
            global,
            GLOBAL_RAGFLOW_DOCUMENT_IDS,
            ENV_RAGFLOW_DOCUMENT_IDS,
            &env_lookup,
            DEFAULT_RAGFLOW_DOCUMENT_IDS,
        ),
        page: resolve_positive_u32(
            global,
            GLOBAL_RAGFLOW_PAGE,
            ENV_RAGFLOW_PAGE,
            &env_lookup,
            DEFAULT_RAGFLOW_PAGE,
        ),
        page_size: resolve_positive_u32(
            global,
            GLOBAL_RAGFLOW_PAGE_SIZE,
            ENV_RAGFLOW_PAGE_SIZE,
            &env_lookup,
            DEFAULT_RAGFLOW_PAGE_SIZE,
        ),
        similarity_threshold: resolve_unit_f32(
            global,
            GLOBAL_RAGFLOW_SIMILARITY_THRESHOLD,
            ENV_RAGFLOW_SIMILARITY_THRESHOLD,
            &env_lookup,
            DEFAULT_RAGFLOW_SIMILARITY_THRESHOLD,
        ),
        vector_similarity_weight: resolve_unit_f32(
            global,
            GLOBAL_RAGFLOW_VECTOR_SIMILARITY_WEIGHT,
            ENV_RAGFLOW_VECTOR_SIMILARITY_WEIGHT,
            &env_lookup,
            DEFAULT_RAGFLOW_VECTOR_SIMILARITY_WEIGHT,
        ),
        top_k: resolve_positive_u32(
            global,
            GLOBAL_RAGFLOW_TOP_K,
            ENV_RAGFLOW_TOP_K,
            &env_lookup,
            DEFAULT_RAGFLOW_TOP_K,
        ),
        rerank_id: resolve_optional_string(
            global,
            GLOBAL_RAGFLOW_RERANK_ID,
            ENV_RAGFLOW_RERANK_ID,
            &env_lookup,
        )
        .or_else(|| DEFAULT_RAGFLOW_RERANK_ID.map(str::to_string)),
        keyword: resolve_bool(
            global,
            GLOBAL_RAGFLOW_KEYWORD,
            ENV_RAGFLOW_KEYWORD,
            &env_lookup,
            DEFAULT_RAGFLOW_KEYWORD,
        ),
        highlight: resolve_bool(
            global,
            GLOBAL_RAGFLOW_HIGHLIGHT,
            ENV_RAGFLOW_HIGHLIGHT,
            &env_lookup,
            DEFAULT_RAGFLOW_HIGHLIGHT,
        ),
        timeout_secs: resolve_positive_u64(
            global,
            GLOBAL_RAGFLOW_TIMEOUT_SECS,
            ENV_RAGFLOW_TIMEOUT_SECS,
            &env_lookup,
            DEFAULT_RAGFLOW_TIMEOUT_SECS,
        ),
    }
}

fn resolve_string(
    global: &HashMap<String, Value>,
    global_key: &str,
    env_key: &str,
    env_lookup: &impl Fn(&str) -> Option<String>,
    default: &str,
) -> String {
    global_value(global, global_key)
        .and_then(value_string)
        .or_else(|| env_lookup(env_key).and_then(non_empty_string))
        .unwrap_or_else(|| default.to_string())
}

fn resolve_optional_string(
    global: &HashMap<String, Value>,
    global_key: &str,
    env_key: &str,
    env_lookup: &impl Fn(&str) -> Option<String>,
) -> Option<String> {
    global_value(global, global_key)
        .and_then(value_string)
        .or_else(|| env_lookup(env_key).and_then(non_empty_string))
}

fn resolve_list(
    global: &HashMap<String, Value>,
    global_key: &str,
    env_key: &str,
    env_lookup: &impl Fn(&str) -> Option<String>,
    default: &str,
) -> Vec<String> {
    global_value(global, global_key)
        .and_then(value_list)
        .or_else(|| {
            env_lookup(env_key).and_then(|raw| {
                let values = split_csv(&raw);
                (!values.is_empty()).then_some(values)
            })
        })
        .unwrap_or_else(|| split_csv(default))
}

fn resolve_positive_u32(
    global: &HashMap<String, Value>,
    global_key: &str,
    env_key: &str,
    env_lookup: &impl Fn(&str) -> Option<String>,
    default: u32,
) -> u32 {
    global_value(global, global_key)
        .and_then(value_u32)
        .filter(|value| *value > 0)
        .or_else(|| {
            env_lookup(env_key)
                .and_then(|raw| raw.trim().parse().ok())
                .filter(|value| *value > 0)
        })
        .unwrap_or(default)
}

fn resolve_positive_u64(
    global: &HashMap<String, Value>,
    global_key: &str,
    env_key: &str,
    env_lookup: &impl Fn(&str) -> Option<String>,
    default: u64,
) -> u64 {
    global_value(global, global_key)
        .and_then(value_u64)
        .filter(|value| *value > 0)
        .or_else(|| {
            env_lookup(env_key)
                .and_then(|raw| raw.trim().parse().ok())
                .filter(|value| *value > 0)
        })
        .unwrap_or(default)
}

fn resolve_unit_f32(
    global: &HashMap<String, Value>,
    global_key: &str,
    env_key: &str,
    env_lookup: &impl Fn(&str) -> Option<String>,
    default: f32,
) -> f32 {
    global_value(global, global_key)
        .and_then(value_f32)
        .filter(|value| (0.0..=1.0).contains(value))
        .or_else(|| {
            env_lookup(env_key)
                .and_then(|raw| raw.trim().parse().ok())
                .filter(|value| (0.0..=1.0).contains(value))
        })
        .unwrap_or(default)
}

fn resolve_bool(
    global: &HashMap<String, Value>,
    global_key: &str,
    env_key: &str,
    env_lookup: &impl Fn(&str) -> Option<String>,
    default: bool,
) -> bool {
    global_value(global, global_key)
        .and_then(value_bool)
        .or_else(|| env_lookup(env_key).and_then(|raw| parse_bool(&raw)))
        .unwrap_or(default)
}

fn global_value<'a>(global: &'a HashMap<String, Value>, key: &str) -> Option<&'a Value> {
    let data = global.get(key)?;
    Some(data.get("value").unwrap_or(data))
}

fn non_empty_string(raw: String) -> Option<String> {
    let value = raw.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn value_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .and_then(|raw| non_empty_string(raw.to_string()))
}

fn value_list(value: &Value) -> Option<Vec<String>> {
    let values = if let Some(items) = value.as_array() {
        items
            .iter()
            .filter_map(Value::as_str)
            .filter_map(|item| non_empty_string(item.to_string()))
            .collect()
    } else {
        value.as_str().map(split_csv).unwrap_or_default()
    };
    (!values.is_empty()).then_some(values)
}

fn value_u32(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .or_else(|| value.as_str().and_then(|raw| raw.trim().parse().ok()))
}

fn value_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|raw| raw.trim().parse().ok()))
}

fn value_f32(value: &Value) -> Option<f32> {
    value
        .as_f64()
        .map(|number| number as f32)
        .or_else(|| value.as_str().and_then(|raw| raw.trim().parse().ok()))
        .filter(|number| number.is_finite())
}

fn value_bool(value: &Value) -> Option<bool> {
    value
        .as_bool()
        .or_else(|| value.as_str().and_then(parse_bool))
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

fn split_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .filter_map(|item| non_empty_string(item.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::{
        GLOBAL_RAGFLOW_API_KEY, GLOBAL_RAGFLOW_BASE_URL, GLOBAL_RAGFLOW_DATASET_IDS,
        GLOBAL_RAGFLOW_DOCUMENT_IDS, GLOBAL_RAGFLOW_HIGHLIGHT, GLOBAL_RAGFLOW_KEYWORD,
        GLOBAL_RAGFLOW_PAGE, GLOBAL_RAGFLOW_PAGE_SIZE, GLOBAL_RAGFLOW_RERANK_ID,
        GLOBAL_RAGFLOW_SIMILARITY_THRESHOLD, GLOBAL_RAGFLOW_TIMEOUT_SECS, GLOBAL_RAGFLOW_TOP_K,
        GLOBAL_RAGFLOW_VECTOR_SIMILARITY_WEIGHT, RagflowConfig, RagflowConfigState,
        resolve_ragflow_config, should_reload_from_notification,
    };

    #[test]
    fn global_config_overrides_environment_values_for_every_ragflow_setting() {
        let global = HashMap::from([
            (
                GLOBAL_RAGFLOW_BASE_URL.to_string(),
                json!({ "value": "https://global.example" }),
            ),
            (
                GLOBAL_RAGFLOW_API_KEY.to_string(),
                json!({ "value": "global-key" }),
            ),
            (
                GLOBAL_RAGFLOW_DATASET_IDS.to_string(),
                json!({ "value": ["global-dataset"] }),
            ),
            (
                GLOBAL_RAGFLOW_DOCUMENT_IDS.to_string(),
                json!({ "value": "global-doc-1,global-doc-2" }),
            ),
            (GLOBAL_RAGFLOW_PAGE.to_string(), json!({ "value": 2 })),
            (GLOBAL_RAGFLOW_PAGE_SIZE.to_string(), json!({ "value": 7 })),
            (
                GLOBAL_RAGFLOW_SIMILARITY_THRESHOLD.to_string(),
                json!({ "value": 0.4 }),
            ),
            (
                GLOBAL_RAGFLOW_VECTOR_SIMILARITY_WEIGHT.to_string(),
                json!({ "value": 0.6 }),
            ),
            (GLOBAL_RAGFLOW_TOP_K.to_string(), json!({ "value": 12 })),
            (
                GLOBAL_RAGFLOW_RERANK_ID.to_string(),
                json!({ "value": "global-reranker" }),
            ),
            (
                GLOBAL_RAGFLOW_KEYWORD.to_string(),
                json!({ "value": false }),
            ),
            (
                GLOBAL_RAGFLOW_HIGHLIGHT.to_string(),
                json!({ "value": true }),
            ),
            (
                GLOBAL_RAGFLOW_TIMEOUT_SECS.to_string(),
                json!({ "value": 45 }),
            ),
        ]);
        let env_values = environment_values("env");

        let resolved = resolve_ragflow_config(&global, |key| env_values.get(key).cloned());

        assert_eq!(resolved.base_url, "https://global.example");
        assert_eq!(resolved.api_key, "global-key");
        assert_eq!(resolved.dataset_ids, vec!["global-dataset"]);
        assert_eq!(resolved.document_ids, vec!["global-doc-1", "global-doc-2"]);
        assert_eq!(resolved.page, 2);
        assert_eq!(resolved.page_size, 7);
        assert_eq!(resolved.similarity_threshold, 0.4);
        assert_eq!(resolved.vector_similarity_weight, 0.6);
        assert_eq!(resolved.top_k, 12);
        assert_eq!(resolved.rerank_id.as_deref(), Some("global-reranker"));
        assert!(!resolved.keyword);
        assert!(resolved.highlight);
        assert_eq!(resolved.timeout_secs, 45);
    }

    #[test]
    fn missing_global_config_falls_back_to_environment_then_central_defaults() {
        let env_values = environment_values("env");
        let from_env = resolve_ragflow_config(&HashMap::new(), |key| env_values.get(key).cloned());

        assert_eq!(from_env.base_url, "https://env.example");
        assert_eq!(from_env.dataset_ids, vec!["env-dataset"]);
        assert_eq!(from_env.page_size, 8);
        assert_eq!(from_env.similarity_threshold, 0.5);
        assert!(!from_env.keyword);

        let from_defaults = resolve_ragflow_config(&HashMap::new(), |_| None);
        assert_eq!(from_defaults, RagflowConfig::default());
    }

    #[test]
    fn cached_state_tracks_web_admin_update_and_delete() {
        let env_values = environment_values("env");
        let mut state =
            RagflowConfigState::from_sources(HashMap::new(), |key| env_values.get(key).cloned());

        assert_eq!(state.resolved.top_k, 15);

        state.upsert(GLOBAL_RAGFLOW_TOP_K, json!({ "value": 21 }), |key| {
            env_values.get(key).cloned()
        });
        assert_eq!(state.resolved.top_k, 21);

        state.remove(GLOBAL_RAGFLOW_TOP_K, |key| env_values.get(key).cloned());
        assert_eq!(state.resolved.top_k, 15);
    }

    #[test]
    fn pubsub_notification_ignores_this_instance_and_refreshes_other_instances() {
        assert!(!should_reload_from_notification("instance-a", "instance-a"));
        assert!(should_reload_from_notification("instance-b", "instance-a"));
        assert!(should_reload_from_notification("", "instance-a"));
    }

    fn environment_values(prefix: &str) -> HashMap<&'static str, String> {
        HashMap::from([
            ("RAGFLOW_BASE_URL", format!("https://{prefix}.example")),
            ("RAGFLOW_API_KEY", format!("{prefix}-key")),
            ("RAGFLOW_DATASET_IDS", format!("{prefix}-dataset")),
            ("RAGFLOW_DOCUMENT_IDS", format!("{prefix}-doc")),
            ("RAGFLOW_PAGE", "3".to_string()),
            ("RAGFLOW_PAGE_SIZE", "8".to_string()),
            ("RAGFLOW_SIMILARITY_THRESHOLD", "0.5".to_string()),
            ("RAGFLOW_VECTOR_SIMILARITY_WEIGHT", "0.7".to_string()),
            ("RAGFLOW_TOP_K", "15".to_string()),
            ("RAGFLOW_RERANK_ID", format!("{prefix}-reranker")),
            ("RAGFLOW_KEYWORD", "false".to_string()),
            ("RAGFLOW_HIGHLIGHT", "true".to_string()),
            ("RAGFLOW_TIMEOUT_SECS", "50".to_string()),
        ])
    }
}
