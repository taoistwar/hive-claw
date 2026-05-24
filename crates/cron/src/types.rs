//! Cron type definitions (port of `nanobot.cron.types`).

use serde::{Deserialize, Serialize};

/// Discriminant of a [`CronSchedule`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleKind {
    At,
    Every,
    Cron,
}

impl Default for ScheduleKind {
    fn default() -> Self {
        Self::Every
    }
}

/// Schedule definition for a cron job.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronSchedule {
    pub kind: ScheduleKind,
    /// For `at`: timestamp in ms.
    #[serde(default, rename = "atMs")]
    pub at_ms: Option<i64>,
    /// For `every`: interval in ms.
    #[serde(default, rename = "everyMs")]
    pub every_ms: Option<i64>,
    /// For `cron`: cron expression (e.g. `0 9 * * *`).
    #[serde(default)]
    pub expr: Option<String>,
    /// Timezone for cron expressions.
    #[serde(default)]
    pub tz: Option<String>,
}

/// Payload kind — what to do when the job runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayloadKind {
    SystemEvent,
    AgentTurn,
}

impl Default for PayloadKind {
    fn default() -> Self {
        Self::AgentTurn
    }
}

/// What to do when the job runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronPayload {
    #[serde(default)]
    pub kind: PayloadKind,
    #[serde(default)]
    pub message: String,
    /// Deliver response to channel.
    #[serde(default)]
    pub deliver: bool,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    /// Channel-specific metadata (e.g. Slack thread_ts).
    #[serde(default, alias = "channelMeta", rename = "channelMeta")]
    pub channel_meta: Option<serde_json::Value>,
    /// Session key for session-scoped delivery.
    #[serde(default, alias = "sessionKey", rename = "sessionKey")]
    pub session_key: Option<String>,
}

/// Single execution record for a cron job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CronRunRecord {
    #[serde(rename = "runAtMs")]
    pub run_at_ms: i64,
    pub status: RunStatus,
    #[serde(default, rename = "durationMs")]
    pub duration_ms: i64,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Ok,
    Error,
    Skipped,
}

/// Runtime state of a job.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronJobState {
    #[serde(default, rename = "nextRunAtMs")]
    pub next_run_at_ms: Option<i64>,
    #[serde(default, rename = "lastRunAtMs")]
    pub last_run_at_ms: Option<i64>,
    #[serde(default, rename = "lastStatus")]
    pub last_status: Option<RunStatus>,
    #[serde(default, rename = "lastError")]
    pub last_error: Option<String>,
    #[serde(default, rename = "runHistory")]
    pub run_history: Vec<CronRunRecord>,
}

/// A scheduled job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub schedule: CronSchedule,
    #[serde(default)]
    pub payload: CronPayload,
    #[serde(default)]
    pub state: CronJobState,
    #[serde(default, rename = "createdAtMs")]
    pub created_at_ms: i64,
    #[serde(default, rename = "updatedAtMs")]
    pub updated_at_ms: i64,
    #[serde(default, rename = "deleteAfterRun")]
    pub delete_after_run: bool,
}

impl CronJob {
    /// Python `CronJob.from_dict(kwargs)` equivalent — tolerant of
    /// partially-populated JSON.
    pub fn from_value(v: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(v)
    }
}

fn default_true() -> bool {
    true
}

/// Persistent store for cron jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronStore {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub jobs: Vec<CronJob>,
}

impl Default for CronStore {
    fn default() -> Self {
        Self {
            version: 1,
            jobs: Vec::new(),
        }
    }
}

fn default_version() -> u32 {
    1
}
