//! Cron tool for scheduling reminders and tasks.
//! Port of `nanobot.agent.tools.cron`.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, FixedOffset, TimeZone};
use chrono_tz::Tz;
use serde_json::{Value, json};

use cron::service::{CronService, RemoveOutcome};
use cron::types::{CronJob, CronPayload, CronSchedule, ScheduleKind};

use super::base::{Tool, ToolExecError};
use super::context::RequestContext;

#[derive(Clone, Default, Debug)]
pub struct CronContext {
    pub channel: String,
    pub chat_id: String,
    pub in_cron_context: bool,
}

pub struct CronTool {
    service: Arc<CronService>,
    default_timezone: String,
    context: Arc<Mutex<CronContext>>,
}

impl CronTool {
    pub fn new(service: Arc<CronService>, default_timezone: impl Into<String>) -> Self {
        Self {
            service,
            default_timezone: default_timezone.into(),
            context: Arc::new(Mutex::new(CronContext::default())),
        }
    }

    pub fn set_context(&self, channel: &str, chat_id: &str) {
        let mut ctx = self.context.lock().unwrap();
        ctx.channel = channel.to_string();
        ctx.chat_id = chat_id.to_string();
    }

    pub fn set_cron_context(&self, active: bool) {
        self.context.lock().unwrap().in_cron_context = active;
    }

    fn validate_timezone(tz: &str) -> Option<String> {
        if tz.parse::<Tz>().is_err() {
            return Some(format!("Error: unknown timezone '{tz}'"));
        }
        None
    }

    fn format_timing(&self, schedule: &CronSchedule) -> String {
        match schedule.kind {
            ScheduleKind::Cron => {
                let expr = schedule.expr.clone().unwrap_or_default();
                let tz = schedule
                    .tz
                    .as_ref()
                    .map(|t| format!(" ({t})"))
                    .unwrap_or_default();
                format!("cron: {expr}{tz}")
            }
            ScheduleKind::Every => match schedule.every_ms {
                Some(ms) if ms % 3_600_000 == 0 => format!("every {}h", ms / 3_600_000),
                Some(ms) if ms % 60_000 == 0 => format!("every {}m", ms / 60_000),
                Some(ms) if ms % 1000 == 0 => format!("every {}s", ms / 1000),
                Some(ms) => format!("every {ms}ms"),
                None => "every ?".into(),
            },
            ScheduleKind::At => match schedule.at_ms {
                Some(ms) => format!(
                    "at {}",
                    Self::format_timestamp(ms, self.display_tz(schedule))
                ),
                None => "at ?".into(),
            },
        }
    }

    fn display_tz<'a>(&'a self, schedule: &'a CronSchedule) -> &'a str {
        schedule.tz.as_deref().unwrap_or(&self.default_timezone)
    }

    fn format_timestamp(ms: i64, tz_name: &str) -> String {
        let tz: Tz = tz_name.parse().unwrap_or(chrono_tz::UTC);
        let dt = tz
            .timestamp_millis_opt(ms)
            .single()
            .unwrap_or_else(|| tz.timestamp_opt(0, 0).unwrap());
        format!("{} ({tz_name})", dt.to_rfc3339())
    }

    fn system_job_purpose(job: &CronJob) -> &'static str {
        if job.name == "dream" {
            "Dream memory consolidation for long-term memory."
        } else {
            "System-managed internal job."
        }
    }
}

#[async_trait]
impl Tool for CronTool {
    fn name(&self) -> &str {
        "cron"
    }
    fn description(&self) -> String {
        "Schedule reminders and recurring tasks. Actions: add, list, remove.".into()
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "action":{"type":"string","enum":["add","list","remove"],"description":"Action to perform"},
                "name":{"type":"string","description":"Optional short label"},
                "message":{"type":"string","description":"REQUIRED when action='add'. Instruction to execute on trigger."},
                "every_seconds":{"type":"integer","description":"Interval in seconds (recurring)","minimum":0},
                "cron_expr":{"type":"string","description":"Cron expression like '0 9 * * *'"},
                "tz":{"type":"string","description":"IANA timezone for cron_expr"},
                "at":{"type":"string","description":"ISO datetime for one-time execution"},
                "deliver":{"type":"boolean","default":true,"description":"Deliver result to user channel"},
                "job_id":{"type":"string","description":"REQUIRED when action='remove'"},
            },
        })
    }
    fn set_tool_context(&self, ctx: &RequestContext) {
        self.set_context(&ctx.channel, &ctx.chat_id);
    }
    async fn execute(&self, params: Value) -> Result<Value, ToolExecError> {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        match action {
            "add" => Ok(Value::String(self.add_job(&params).await)),
            "list" => Ok(Value::String(self.list_jobs().await)),
            "remove" => Ok(Value::String(self.remove_job(&params).await)),
            other => Ok(Value::String(format!("Unknown action: {other}"))),
        }
    }
}

impl CronTool {
    async fn add_job(&self, params: &Value) -> String {
        if self.context.lock().unwrap().in_cron_context {
            return "Error: cannot schedule new jobs from within a cron job execution".into();
        }
        let message = params
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if message.is_empty() {
            return "Error: cron action='add' requires a non-empty 'message' parameter".into();
        }
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| message.chars().take(30).collect());
        let deliver = params
            .get("deliver")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let every_seconds = params.get("every_seconds").and_then(|v| v.as_i64());
        let cron_expr = params
            .get("cron_expr")
            .and_then(|v| v.as_str())
            .map(String::from);
        let tz = params.get("tz").and_then(|v| v.as_str()).map(String::from);
        let at = params.get("at").and_then(|v| v.as_str()).map(String::from);

        let (channel, chat_id) = {
            let ctx = self.context.lock().unwrap();
            (ctx.channel.clone(), ctx.chat_id.clone())
        };
        if channel.is_empty() || chat_id.is_empty() {
            return "Error: no session context (channel/chat_id)".into();
        }
        if tz.is_some() && cron_expr.is_none() {
            return "Error: tz can only be used with cron_expr".into();
        }
        if let Some(tz_name) = &tz {
            if let Some(err) = Self::validate_timezone(tz_name) {
                return err;
            }
        }

        let (schedule, delete_after) = if let Some(sec) = every_seconds.filter(|v| *v > 0) {
            (
                CronSchedule {
                    kind: ScheduleKind::Every,
                    every_ms: Some(sec * 1000),
                    ..Default::default()
                },
                false,
            )
        } else if let Some(expr) = cron_expr {
            let effective_tz = tz.unwrap_or_else(|| self.default_timezone.clone());
            if let Some(err) = Self::validate_timezone(&effective_tz) {
                return err;
            }
            (
                CronSchedule {
                    kind: ScheduleKind::Cron,
                    expr: Some(expr),
                    tz: Some(effective_tz),
                    ..Default::default()
                },
                false,
            )
        } else if let Some(at_str) = at {
            let parsed: Option<DateTime<FixedOffset>> = DateTime::parse_from_rfc3339(&at_str)
                .or_else(|_| DateTime::parse_from_str(&at_str, "%Y-%m-%dT%H:%M:%S%:z"))
                .ok()
                .or_else(|| {
                    chrono::NaiveDateTime::parse_from_str(&at_str, "%Y-%m-%dT%H:%M:%S")
                        .ok()
                        .and_then(|ndt| {
                            let tz: Tz = self.default_timezone.parse().unwrap_or(chrono_tz::UTC);
                            tz.from_local_datetime(&ndt)
                                .single()
                                .map(|dt| dt.with_timezone(&FixedOffset::east_opt(0).unwrap()))
                        })
                });
            let Some(dt) = parsed else {
                return format!(
                    "Error: invalid ISO datetime format '{at_str}'. Expected YYYY-MM-DDTHH:MM:SS"
                );
            };
            let at_ms = dt.timestamp_millis();
            (
                CronSchedule {
                    kind: ScheduleKind::At,
                    at_ms: Some(at_ms),
                    ..Default::default()
                },
                true,
            )
        } else {
            return "Error: either every_seconds, cron_expr, or at is required".into();
        };

        match self
            .service
            .add_job(
                &name,
                schedule,
                message,
                deliver,
                Some(channel),
                Some(chat_id),
                delete_after,
                None,
                None,
            )
            .await
        {
            Ok(job) => format!("Created job '{}' (id: {})", job.name, job.id),
            Err(e) => format!("Error: {e}"),
        }
    }

    async fn list_jobs(&self) -> String {
        let jobs = self.service.list_jobs(false).await;
        if jobs.is_empty() {
            return "No scheduled jobs.".into();
        }
        let mut lines = Vec::new();
        for j in &jobs {
            let timing = self.format_timing(&j.schedule);
            let mut parts = vec![format!("- {} (id: {}, {})", j.name, j.id, timing)];
            if matches!(j.payload.kind, cron::types::PayloadKind::SystemEvent) {
                parts.push(format!("  Purpose: {}", Self::system_job_purpose(j)));
                parts.push("  Protected: visible for inspection, but cannot be removed.".into());
            }
            parts.extend(self.format_state(&j.state, &j.schedule));
            lines.push(parts.join("\n"));
        }
        format!("Scheduled jobs:\n{}", lines.join("\n"))
    }

    fn format_state(
        &self,
        state: &cron::types::CronJobState,
        schedule: &CronSchedule,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        let display_tz = self.display_tz(schedule);
        if let Some(ms) = state.last_run_at_ms {
            let status = state
                .last_status
                .map(|s| format!("{s:?}").to_lowercase())
                .unwrap_or_else(|| "unknown".into());
            let mut info = format!(
                "  Last run: {} — {}",
                Self::format_timestamp(ms, display_tz),
                status
            );
            if let Some(err) = &state.last_error {
                info.push_str(&format!(" ({err})"));
            }
            lines.push(info);
        }
        if let Some(ms) = state.next_run_at_ms {
            lines.push(format!(
                "  Next run: {}",
                Self::format_timestamp(ms, display_tz)
            ));
        }
        lines
    }

    async fn remove_job(&self, params: &Value) -> String {
        let Some(job_id) = params.get("job_id").and_then(|v| v.as_str()) else {
            return "Error: job_id is required for remove".into();
        };
        match self.service.remove_job(job_id).await {
            RemoveOutcome::Removed => format!("Removed job {job_id}"),
            RemoveOutcome::Protected => format!(
                "Cannot remove job `{job_id}`. This is a protected system-managed cron job."
            ),
            RemoveOutcome::NotFound => format!("Job {job_id} not found"),
        }
    }
}

// Keep field suppress unused-warning.
#[allow(dead_code)]
fn _unused(payload: CronPayload) -> CronPayload {
    payload
}
