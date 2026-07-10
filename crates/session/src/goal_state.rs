/// Session metadata helpers for sustained goals (e.g. `long_task` / `complete_goal`).
///
/// Tools set `metadata[GOAL_STATE_KEY]`. Reads accept the legacy session key `thread_goal`
/// for older sessions. Callers use `goal_state_runtime_lines`, `goal_state_ws_blob`, and
/// `runner_wall_llm_timeout_s` without importing tool implementations.
use std::collections::HashMap;

use serde_json::Value;

use crate::manager::SessionManager;

pub const GOAL_STATE_KEY: &str = "goal_state";
const LEGACY_GOAL_STATE_SESSION_KEY: &str = "thread_goal";
const MAX_OBJECTIVE_IN_RUNTIME: usize = 4000;
const MAX_OBJECTIVE_WS: usize = 600;

fn session_goal_raw(metadata: Option<&HashMap<String, Value>>) -> Option<&Value> {
    let metadata = metadata?;
    if let Some(val) = metadata.get(GOAL_STATE_KEY) {
        return Some(val);
    }
    metadata.get(LEGACY_GOAL_STATE_SESSION_KEY)
}

/// Remove legacy metadata key after migrating writes to `GOAL_STATE_KEY`.
pub fn discard_legacy_goal_state_key(metadata: &mut HashMap<String, Value>) {
    metadata.remove(LEGACY_GOAL_STATE_SESSION_KEY);
}

/// Return the session goal blob under `GOAL_STATE_KEY` or the legacy key.
pub fn goal_state_raw(metadata: Option<&HashMap<String, Value>>) -> Option<&Value> {
    session_goal_raw(metadata)
}

/// True when this session has an active sustained objective (`long_task` bookkeeping).
pub fn sustained_goal_active(metadata: Option<&HashMap<String, Value>>) -> bool {
    let goal = parse_goal_state(goal_state_raw(metadata));
    matches!(goal, Some(ref g) if g.get("status").and_then(|v| v.as_str()) == Some("active"))
}

/// Parse a goal state blob into a JSON object, if valid.
pub fn parse_goal_state(blob: Option<&Value>) -> Option<serde_json::Map<String, Value>> {
    let blob = blob?;
    if let Some(obj) = blob.as_object() {
        return Some(obj.clone());
    }
    if let Some(s) = blob.as_str() {
        if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(s) {
            return Some(obj);
        }
    }
    None
}

/// Lines appended inside the Runtime Context block when a goal is active.
pub fn goal_state_runtime_lines(metadata: Option<&HashMap<String, Value>>) -> Vec<String> {
    let Some(metadata) = metadata else {
        return vec![];
    };
    let Some(goal) = parse_goal_state(session_goal_raw(Some(metadata))) else {
        return vec![];
    };

    if goal.get("status").and_then(|v| v.as_str()) != Some("active") {
        return vec![];
    }

    let objective = goal
        .get("objective")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if objective.is_empty() {
        return vec!["Goal: active (no objective text stored).".into()];
    }

    let mut objective = if objective.len() > MAX_OBJECTIVE_IN_RUNTIME {
        let truncated: String = objective.chars().take(MAX_OBJECTIVE_IN_RUNTIME).collect();
        let truncated = truncated.trim_end();
        format!("{truncated}\n\u{2026} (truncated)")
    } else {
        objective
    };

    let mut out = vec!["Goal (active):".into(), objective];

    if let Some(hint) = goal.get("ui_summary").and_then(|v| v.as_str()) {
        let hint = hint.trim();
        if !hint.is_empty() {
            out.push(format!("Summary: {hint}"));
        }
    }

    out
}

/// JSON-safe snapshot for WebSocket `goal_state` events (one chat_id per frame).
pub fn goal_state_ws_blob(metadata: Option<&HashMap<String, Value>>) -> serde_json::Value {
    let goal = metadata
        .and_then(|m| session_goal_raw(Some(m)))
        .and_then(|v| parse_goal_state(Some(v)));

    if let Some(goal) = goal {
        if goal.get("status").and_then(|v| v.as_str()) == Some("active") {
            let mut objective = goal
                .get("objective")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();

            if objective.len() > MAX_OBJECTIVE_WS {
                let truncated: String = objective.chars().take(MAX_OBJECTIVE_WS).collect();
                objective = format!("{}\u{2026}", truncated.trim_end());
            }

            let summary = goal
                .get("ui_summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            let summary: String = summary.chars().take(120).collect();

            let mut blob = serde_json::Map::new();
            blob.insert("active".into(), Value::Bool(true));
            if !summary.is_empty() {
                blob.insert("ui_summary".into(), Value::String(summary));
            }
            if !objective.is_empty() {
                blob.insert("objective".into(), Value::String(objective));
            }
            return Value::Object(blob);
        }
    }

    serde_json::json!({"active": false})
}

/// Wall-clock cap for AgentRunner when streaming an LLM.
///
/// Returns `Some(0.0)` to disable `tokio::time::timeout` around the request when a sustained goal is
/// active; `None` means use the default timeout. Pass in-memory `metadata` when the
/// caller already holds `Session.metadata` for this turn.
pub fn runner_wall_llm_timeout_s(
    _sessions: &SessionManager,
    session_key: Option<&str>,
    metadata: Option<&HashMap<String, Value>>,
) -> Option<f64> {
    let meta: Option<&HashMap<String, Value>> = if metadata.is_some() {
        metadata
    } else if session_key.is_some() {
        // TODO: sessions.get_or_create(session_key).metadata reference
        None
    } else {
        None
    };

    if sustained_goal_active(meta) {
        Some(0.0)
    } else {
        None
    }
}
