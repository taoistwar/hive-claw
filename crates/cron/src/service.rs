//! Cron service for scheduling agent tasks (port of
//! `nanobot.cron.service`).
//!
//! The Python original runs on asyncio with a single-timer model:
//! a single background task sleeps until the earliest `next_run_at_ms`
//! among enabled jobs, runs all due jobs, then re-arms the timer. This
//! Rust port keeps the same model using a `tokio` task guarded by a
//! `Mutex`, and performs cross-process coordination via a lock file on
//! the action-log sibling (`action.jsonl.lock`).

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use chrono_tz::Tz;
use cron_expr::Schedule as CronExprSchedule;
use fs2::FileExt;
use futures::future::BoxFuture;
use log::{debug, error, info, warn};
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::types::{
    CronJob, CronJobState, CronPayload, CronRunRecord, CronSchedule, CronStore, PayloadKind,
    RunStatus, ScheduleKind,
};

/// Callback invoked when a job is due.
///
/// Boxed future return type keeps the trait object-safe.
#[async_trait]
pub trait JobHandler: Send + Sync {
    async fn on_job(&self, job: &CronJob) -> Result<Option<String>, String>;
}

/// Convenience blanket impl so closures can be used directly.
pub struct FnHandler<F>
where
    F: Fn(&CronJob) -> BoxFuture<'_, Result<Option<String>, String>> + Send + Sync + 'static,
{
    f: F,
}

#[async_trait]
impl<F> JobHandler for FnHandler<F>
where
    F: Fn(&CronJob) -> BoxFuture<'_, Result<Option<String>, String>> + Send + Sync + 'static,
{
    async fn on_job(&self, job: &CronJob) -> Result<Option<String>, String> {
        (self.f)(job).await
    }
}

impl<F> FnHandler<F>
where
    F: Fn(&CronJob) -> BoxFuture<'_, Result<Option<String>, String>> + Send + Sync + 'static,
{
    pub fn new(f: F) -> Self {
        Self { f }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CronError {
    #[error("tz can only be used with cron schedules")]
    TzOnNonCron,
    #[error("unknown timezone '{0}'")]
    UnknownTimezone(String),
    #[error("invalid cron expression '{0}'")]
    BadCronExpr(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Outcome of [`CronService::remove_job`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveOutcome {
    Removed,
    Protected,
    NotFound,
}

/// Outcome of [`CronService::update_job`].
pub enum UpdateOutcome {
    Updated(CronJob),
    Protected,
    NotFound,
}

/// Options for [`CronService::update_job`].
///
/// Use `Option<Option<T>>` for `channel` / `to` so that callers can
/// distinguish "leave unchanged" (`None`) from "set to null"
/// (`Some(None)`).
#[derive(Debug, Default)]
pub struct UpdateJobOpts {
    pub name: Option<String>,
    pub schedule: Option<CronSchedule>,
    pub message: Option<String>,
    pub deliver: Option<bool>,
    pub channel: Option<Option<String>>,
    pub to: Option<Option<String>>,
    pub delete_after_run: Option<bool>,
}

const MAX_RUN_HISTORY: usize = 20;
const DEFAULT_MAX_SLEEP_MS: i64 = 5 * 60 * 1000;

struct Inner {
    store_path: PathBuf,
    action_path: PathBuf,
    lock_path: PathBuf,
    store: Option<CronStore>,
    running: bool,
    timer_active: bool,
    max_sleep_ms: i64,
    timer: Option<JoinHandle<()>>,
    weak_self: Weak<Mutex<Inner>>,
    handler: Option<Arc<dyn JobHandler>>,
}

/// Service for managing and executing scheduled jobs.
#[derive(Clone)]
pub struct CronService {
    inner: Arc<Mutex<Inner>>,
}

impl CronService {
    pub fn new(store_path: PathBuf, handler: Option<Arc<dyn JobHandler>>) -> Self {
        Self::with_max_sleep(store_path, handler, DEFAULT_MAX_SLEEP_MS)
    }

    pub fn with_max_sleep(
        store_path: PathBuf,
        handler: Option<Arc<dyn JobHandler>>,
        max_sleep_ms: i64,
    ) -> Self {
        let action_path = store_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("action.jsonl");
        let lock_path = {
            let mut p = action_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf();
            let fname = format!(
                "{}.lock",
                action_path
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|s| s.to_str())
                    .unwrap_or("action")
            );
            p.push(fname);
            p
        };

        let inner = Arc::new_cyclic(|weak: &Weak<Mutex<Inner>>| {
            Mutex::new(Inner {
                store_path,
                action_path,
                lock_path,
                store: None,
                running: false,
                timer_active: false,
                max_sleep_ms,
                timer: None,
                weak_self: weak.clone(),
                handler,
            })
        });
        Self { inner }
    }

    // -- Public API --------------------------------------------------------

    /// Start the cron service.
    pub async fn start(&self) {
        let mut inner = self.inner.lock().await;
        inner.running = true;
        inner.load_store();
        inner.recompute_next_runs();
        inner.save_store();
        let count = inner.store.as_ref().map(|s| s.jobs.len()).unwrap_or(0);
        inner.arm_timer();
        info!("Cron service started with {count} jobs");
    }

    /// Stop the cron service.
    pub async fn stop(&self) {
        let mut inner = self.inner.lock().await;
        inner.running = false;
        if let Some(handle) = inner.timer.take() {
            handle.abort();
        }
    }

    pub async fn list_jobs(&self, include_disabled: bool) -> Vec<CronJob> {
        let mut inner = self.inner.lock().await;
        inner.load_store();
        let Some(store) = inner.store.as_ref() else {
            return Vec::new();
        };
        let mut jobs: Vec<CronJob> = store
            .jobs
            .iter()
            .filter(|j| include_disabled || j.enabled)
            .cloned()
            .collect();
        jobs.sort_by_key(|j| j.state.next_run_at_ms.unwrap_or(i64::MAX));
        jobs
    }

    pub async fn add_job(
        &self,
        name: &str,
        schedule: CronSchedule,
        message: &str,
        deliver: bool,
        channel: Option<String>,
        to: Option<String>,
        delete_after_run: bool,
    ) -> Result<CronJob, CronError> {
        validate_schedule_for_add(&schedule)?;
        let now = now_ms();
        let id: String = Uuid::new_v4().simple().to_string().chars().take(8).collect();
        let job = CronJob {
            id,
            name: name.to_string(),
            enabled: true,
            schedule: schedule.clone(),
            payload: CronPayload {
                kind: PayloadKind::AgentTurn,
                message: message.to_string(),
                deliver,
                channel,
                to,
            },
            state: CronJobState {
                next_run_at_ms: compute_next_run(&schedule, now),
                ..Default::default()
            },
            created_at_ms: now,
            updated_at_ms: now,
            delete_after_run,
        };

        let mut inner = self.inner.lock().await;
        if inner.running {
            inner.load_store();
            if let Some(store) = inner.store.as_mut() {
                store.jobs.push(job.clone());
            }
            inner.save_store();
            inner.arm_timer();
        } else {
            let v = serde_json::to_value(&job)?;
            inner.append_action("add", v)?;
        }
        info!("Cron: added job '{name}' ({})", job.id);
        Ok(job)
    }

    pub async fn register_system_job(&self, mut job: CronJob) -> CronJob {
        let mut inner = self.inner.lock().await;
        inner.load_store();
        let now = now_ms();
        job.state = CronJobState {
            next_run_at_ms: compute_next_run(&job.schedule, now),
            ..Default::default()
        };
        job.created_at_ms = now;
        job.updated_at_ms = now;
        if let Some(store) = inner.store.as_mut() {
            store.jobs.retain(|j| j.id != job.id);
            store.jobs.push(job.clone());
        }
        inner.save_store();
        inner.arm_timer();
        info!("Cron: registered system job '{}' ({})", job.name, job.id);
        job
    }

    pub async fn remove_job(&self, job_id: &str) -> RemoveOutcome {
        let mut inner = self.inner.lock().await;
        inner.load_store();
        let Some(store) = inner.store.as_mut() else {
            return RemoveOutcome::NotFound;
        };
        let Some(job) = store.jobs.iter().find(|j| j.id == job_id) else {
            return RemoveOutcome::NotFound;
        };
        if job.payload.kind == PayloadKind::SystemEvent {
            info!("Cron: refused to remove protected system job {job_id}");
            return RemoveOutcome::Protected;
        }
        let before = store.jobs.len();
        store.jobs.retain(|j| j.id != job_id);
        let removed = store.jobs.len() < before;
        if !removed {
            return RemoveOutcome::NotFound;
        }
        if inner.running {
            inner.save_store();
            inner.arm_timer();
        } else {
            let v = serde_json::json!({ "job_id": job_id });
            let _ = inner.append_action("del", v);
        }
        info!("Cron: removed job {job_id}");
        RemoveOutcome::Removed
    }

    pub async fn enable_job(&self, job_id: &str, enabled: bool) -> Option<CronJob> {
        let mut inner = self.inner.lock().await;
        inner.load_store();
        let store = inner.store.as_mut()?;
        let idx = store.jobs.iter().position(|j| j.id == job_id)?;
        let now = now_ms();
        {
            let job = &mut store.jobs[idx];
            job.enabled = enabled;
            job.updated_at_ms = now;
            job.state.next_run_at_ms = if enabled {
                compute_next_run(&job.schedule, now)
            } else {
                None
            };
        }
        let job_clone = store.jobs[idx].clone();
        if inner.running {
            inner.save_store();
            inner.arm_timer();
        } else {
            let v = serde_json::to_value(&job_clone).ok()?;
            let _ = inner.append_action("update", v);
        }
        Some(job_clone)
    }

    pub async fn update_job(&self, job_id: &str, opts: UpdateJobOpts) -> UpdateOutcome {
        let mut inner = self.inner.lock().await;
        inner.load_store();
        let Some(store) = inner.store.as_mut() else {
            return UpdateOutcome::NotFound;
        };
        let Some(idx) = store.jobs.iter().position(|j| j.id == job_id) else {
            return UpdateOutcome::NotFound;
        };
        if store.jobs[idx].payload.kind == PayloadKind::SystemEvent {
            return UpdateOutcome::Protected;
        }

        if let Some(ref schedule) = opts.schedule {
            if let Err(e) = validate_schedule_for_add(schedule) {
                warn!("Cron: bad update schedule for {job_id}: {e}");
                return UpdateOutcome::NotFound;
            }
        }
        let now = now_ms();
        {
            let job = &mut store.jobs[idx];
            if let Some(s) = opts.schedule {
                job.schedule = s;
            }
            if let Some(n) = opts.name {
                job.name = n;
            }
            if let Some(m) = opts.message {
                job.payload.message = m;
            }
            if let Some(d) = opts.deliver {
                job.payload.deliver = d;
            }
            if let Some(ch) = opts.channel {
                job.payload.channel = ch;
            }
            if let Some(t) = opts.to {
                job.payload.to = t;
            }
            if let Some(d) = opts.delete_after_run {
                job.delete_after_run = d;
            }
            job.updated_at_ms = now;
            if job.enabled {
                job.state.next_run_at_ms = compute_next_run(&job.schedule, now);
            }
        }
        let job_clone = store.jobs[idx].clone();
        if inner.running {
            inner.save_store();
            inner.arm_timer();
        } else if let Ok(v) = serde_json::to_value(&job_clone) {
            let _ = inner.append_action("update", v);
        }
        info!("Cron: updated job '{}' ({})", job_clone.name, job_clone.id);
        UpdateOutcome::Updated(job_clone)
    }

    pub async fn run_job(&self, job_id: &str, force: bool) -> bool {
        let mut inner = self.inner.lock().await;
        let was_running = inner.running;
        inner.running = true;
        inner.load_store();
        let job = inner
            .store
            .as_ref()
            .and_then(|s| s.jobs.iter().find(|j| j.id == job_id).cloned());
        let Some(mut job) = job else {
            inner.running = was_running;
            return false;
        };
        if !force && !job.enabled {
            inner.running = was_running;
            return false;
        }

        let handler = inner.handler.clone();
        // Drop lock during the handler callback so recursive calls work.
        drop(inner);

        execute_job(&mut job, handler.as_deref()).await;

        let mut inner = self.inner.lock().await;
        if let Some(store) = inner.store.as_mut() {
            if let Some(pos) = store.jobs.iter().position(|j| j.id == job.id) {
                store.jobs[pos] = job;
            }
        }
        inner.save_store();
        inner.running = was_running;
        if was_running {
            inner.arm_timer();
        }
        true
    }

    pub async fn get_job(&self, job_id: &str) -> Option<CronJob> {
        let mut inner = self.inner.lock().await;
        inner.load_store();
        inner
            .store
            .as_ref()
            .and_then(|s| s.jobs.iter().find(|j| j.id == job_id).cloned())
    }

    pub async fn status(&self) -> CronStatus {
        let mut inner = self.inner.lock().await;
        inner.load_store();
        CronStatus {
            enabled: inner.running,
            jobs: inner.store.as_ref().map(|s| s.jobs.len()).unwrap_or(0),
            next_wake_at_ms: inner.next_wake_ms(),
        }
    }
}

/// Summary returned by [`CronService::status`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CronStatus {
    pub enabled: bool,
    pub jobs: usize,
    pub next_wake_at_ms: Option<i64>,
}

impl Inner {
    fn load_store(&mut self) {
        if self.timer_active && self.store.is_some() {
            return;
        }
        let (jobs, version) = match load_jobs_from_disk(&self.store_path) {
            Ok(v) => v,
            Err(e) => {
                warn!("Failed to load cron store: {e}");
                (Vec::new(), 1)
            }
        };
        self.store = Some(CronStore { version, jobs });
        self.merge_action();
    }

    fn merge_action(&mut self) {
        if !self.action_path.exists() {
            return;
        }
        let Some(store) = self.store.as_mut() else {
            return;
        };
        let mut jobs_map: HashMap<String, CronJob> = store
            .jobs
            .iter()
            .map(|j| (j.id.clone(), j.clone()))
            .collect();

        let _lock = acquire_lock(&self.lock_path);
        let Ok(file) = File::open(&self.action_path) else {
            return;
        };
        let reader = BufReader::new(file);
        let mut changed = false;
        for line in reader.lines().flatten() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(action) = serde_json::from_str::<Value>(line) else {
                debug!("load action line error: not json: {line}");
                continue;
            };
            let kind = action.get("action").and_then(Value::as_str).unwrap_or("");
            let params = action.get("params").cloned().unwrap_or(Value::Null);
            match kind {
                "del" => {
                    if let Some(id) = params.get("job_id").and_then(Value::as_str) {
                        jobs_map.remove(id);
                        changed = true;
                    }
                }
                "" => continue,
                _ => {
                    if let Ok(job) = serde_json::from_value::<CronJob>(params) {
                        jobs_map.insert(job.id.clone(), job);
                        changed = true;
                    }
                }
            }
        }
        store.jobs = jobs_map.into_values().collect();
        if self.running && changed {
            let _ = std::fs::write(&self.action_path, "");
            self.save_store();
        }
    }

    fn save_store(&self) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        if let Some(parent) = self.store_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let json = match serde_json::to_string_pretty(store) {
            Ok(s) => s,
            Err(e) => {
                warn!("Cron: failed to serialize store: {e}");
                return;
            }
        };
        if let Err(e) = std::fs::write(&self.store_path, json) {
            warn!("Cron: failed to save store: {e}");
        }
    }

    fn recompute_next_runs(&mut self) {
        if let Some(store) = self.store.as_mut() {
            let now = now_ms();
            for job in store.jobs.iter_mut() {
                if job.enabled {
                    job.state.next_run_at_ms = compute_next_run(&job.schedule, now);
                }
            }
        }
    }

    fn next_wake_ms(&self) -> Option<i64> {
        self.store.as_ref().and_then(|s| {
            s.jobs
                .iter()
                .filter(|j| j.enabled)
                .filter_map(|j| j.state.next_run_at_ms)
                .min()
        })
    }

    fn arm_timer(&mut self) {
        if let Some(handle) = self.timer.take() {
            handle.abort();
        }
        if !self.running {
            return;
        }
        let delay_ms = match self.next_wake_ms() {
            None => self.max_sleep_ms,
            Some(next) => self.max_sleep_ms.min((next - now_ms()).max(0)),
        };
        let weak = self.weak_self.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms as u64)).await;
            let Some(strong) = weak.upgrade() else { return };
            // Reacquire lock and dispatch tick if still running.
            let should_tick = {
                let guard = strong.lock().await;
                guard.running
            };
            if should_tick {
                on_timer(strong).await;
            }
        });
        self.timer = Some(handle);
    }

    fn append_action(&self, kind: &str, params: Value) -> std::io::Result<()> {
        if let Some(parent) = self.store_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _lock = acquire_lock(&self.lock_path);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.action_path)?;
        let v = serde_json::json!({ "action": kind, "params": params });
        writeln!(file, "{}", serde_json::to_string(&v)?)?;
        Ok(())
    }
}

async fn on_timer(inner: Arc<Mutex<Inner>>) {
    let (due_jobs, handler) = {
        let mut guard = inner.lock().await;
        guard.load_store();
        guard.timer_active = true;
        let handler = guard.handler.clone();
        let now = now_ms();
        let due: Vec<CronJob> = guard
            .store
            .as_ref()
            .map(|s| {
                s.jobs
                    .iter()
                    .filter(|j| {
                        j.enabled
                            && j.state
                                .next_run_at_ms
                                .map(|t| now >= t)
                                .unwrap_or(false)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        (due, handler)
    };

    for mut job in due_jobs {
        execute_job(&mut job, handler.as_deref()).await;
        // Write back.
        let mut guard = inner.lock().await;
        if let Some(store) = guard.store.as_mut() {
            if let Some(pos) = store.jobs.iter().position(|j| j.id == job.id) {
                store.jobs[pos] = job.clone();
            }
            // Delete one-shot jobs flagged for removal.
            if job.schedule.kind == ScheduleKind::At && job.delete_after_run {
                store.jobs.retain(|j| j.id != job.id);
            }
        }
    }

    let mut guard = inner.lock().await;
    guard.save_store();
    guard.timer_active = false;
    guard.arm_timer();
}

async fn execute_job(job: &mut CronJob, handler: Option<&dyn JobHandler>) {
    let start_ms = now_ms();
    info!("Cron: executing job '{}' ({})", job.name, job.id);
    let (status, error) = match handler {
        Some(h) => match h.on_job(job).await {
            Ok(_) => {
                info!("Cron: job '{}' completed", job.name);
                (RunStatus::Ok, None)
            }
            Err(e) => {
                error!("Cron: job '{}' failed: {e}", job.name);
                (RunStatus::Error, Some(e))
            }
        },
        None => (RunStatus::Ok, None),
    };
    let end_ms = now_ms();
    job.state.last_status = Some(status);
    job.state.last_error = error.clone();
    job.state.last_run_at_ms = Some(start_ms);
    job.updated_at_ms = end_ms;
    job.state.run_history.push(CronRunRecord {
        run_at_ms: start_ms,
        status,
        duration_ms: end_ms - start_ms,
        error,
    });
    if job.state.run_history.len() > MAX_RUN_HISTORY {
        let drop_n = job.state.run_history.len() - MAX_RUN_HISTORY;
        job.state.run_history.drain(..drop_n);
    }

    if job.schedule.kind == ScheduleKind::At {
        if !job.delete_after_run {
            job.enabled = false;
            job.state.next_run_at_ms = None;
        }
        // delete_after_run is handled by the caller (on_timer / run_job).
    } else {
        job.state.next_run_at_ms = compute_next_run(&job.schedule, now_ms());
    }
}

// -- Helpers -------------------------------------------------------------

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn validate_schedule_for_add(schedule: &CronSchedule) -> Result<(), CronError> {
    if schedule.tz.is_some() && schedule.kind != ScheduleKind::Cron {
        return Err(CronError::TzOnNonCron);
    }
    if schedule.kind == ScheduleKind::Cron {
        if let Some(tz) = &schedule.tz {
            if tz.parse::<Tz>().is_err() {
                return Err(CronError::UnknownTimezone(tz.clone()));
            }
        }
        if let Some(expr) = &schedule.expr {
            if expr.parse::<CronExprSchedule>().is_err() {
                return Err(CronError::BadCronExpr(expr.clone()));
            }
        }
    }
    Ok(())
}

fn compute_next_run(schedule: &CronSchedule, now_ms: i64) -> Option<i64> {
    match schedule.kind {
        ScheduleKind::At => schedule.at_ms.filter(|t| *t > now_ms),
        ScheduleKind::Every => {
            let every = schedule.every_ms?;
            if every <= 0 {
                return None;
            }
            Some(now_ms + every)
        }
        ScheduleKind::Cron => {
            let expr = schedule.expr.as_ref()?;
            // `cron` crate expects 6 or 7 fields (seconds-first) by default.
            // Accept 5-field POSIX-style by prepending a 0 seconds field.
            let normalized = normalize_cron_expr(expr);
            let parsed: CronExprSchedule = normalized.parse().ok()?;
            let secs = now_ms / 1000;
            let nanos = ((now_ms % 1000) * 1_000_000) as u32;
            let base = Utc
                .timestamp_opt(secs, nanos)
                .single()?;
            let next = match schedule.tz.as_ref().and_then(|tz| tz.parse::<Tz>().ok()) {
                Some(tz) => {
                    let base_tz = base.with_timezone(&tz);
                    parsed.after(&base_tz).next()?.with_timezone(&Utc)
                }
                None => parsed.after(&base).next()?,
            };
            Some(next.timestamp_millis())
        }
    }
}

fn normalize_cron_expr(expr: &str) -> String {
    let fields = expr.split_whitespace().count();
    if fields == 5 {
        format!("0 {expr} *")
    } else if fields == 6 {
        // Could be either `s m h d M w` or `m h d M w y` — assume the
        // `cron` crate default of 7 fields with `s m h d M w y`. Append
        // wildcard year.
        format!("{expr} *")
    } else {
        expr.to_string()
    }
}

fn load_jobs_from_disk(path: &Path) -> Result<(Vec<CronJob>, u32), CronError> {
    if !path.exists() {
        return Ok((Vec::new(), 1));
    }
    let text = std::fs::read_to_string(path)?;
    let data: Value = serde_json::from_str(&text)?;
    let version = data.get("version").and_then(Value::as_u64).unwrap_or(1) as u32;
    let jobs_val = data.get("jobs").cloned().unwrap_or(Value::Array(vec![]));
    let jobs: Vec<CronJob> = serde_json::from_value(jobs_val).unwrap_or_default();
    Ok((jobs, version))
}

fn acquire_lock(lock_path: &Path) -> Option<File> {
    // Best effort: failure to acquire a lock should not crash the service
    // (e.g. missing parent directory on first run).
    if let Some(parent) = lock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .read(true)
        .open(lock_path)
        .ok()?;
    match FileExt::lock_exclusive(&file) {
        Ok(_) => Some(file),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_next_run_every() {
        let sch = CronSchedule {
            kind: ScheduleKind::Every,
            every_ms: Some(60_000),
            ..Default::default()
        };
        let got = compute_next_run(&sch, 1_000).unwrap();
        assert_eq!(got, 61_000);
    }

    #[test]
    fn compute_next_run_at_past_returns_none() {
        let sch = CronSchedule {
            kind: ScheduleKind::At,
            at_ms: Some(100),
            ..Default::default()
        };
        assert!(compute_next_run(&sch, 1_000).is_none());
    }

    #[test]
    fn compute_next_run_cron_parse() {
        let sch = CronSchedule {
            kind: ScheduleKind::Cron,
            expr: Some("0 9 * * *".into()),
            ..Default::default()
        };
        assert!(compute_next_run(&sch, now_ms()).is_some());
    }

    #[test]
    fn validate_tz_requires_cron_kind() {
        let sch = CronSchedule {
            kind: ScheduleKind::Every,
            tz: Some("UTC".into()),
            ..Default::default()
        };
        assert!(matches!(
            validate_schedule_for_add(&sch),
            Err(CronError::TzOnNonCron)
        ));
    }

    #[tokio::test]
    async fn add_and_list_round_trip() {
        let dir = tempdir();
        let svc = CronService::new(dir.join("cron.json"), None);
        svc.start().await;
        let sch = CronSchedule {
            kind: ScheduleKind::Every,
            every_ms: Some(60_000),
            ..Default::default()
        };
        let job = svc
            .add_job("job", sch, "ping", false, None, None, false)
            .await
            .unwrap();
        let listed = svc.list_jobs(false).await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, job.id);
        svc.stop().await;
    }

    fn tempdir() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "nanobot-cron-{}",
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
