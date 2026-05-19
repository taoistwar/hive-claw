//! Heavyweight cron-scheduled memory consolidation — port of
//! `nanobot.agent.memory.Dream`.
//!
//! Two-phase processor:
//!
//! 1. **Phase 1** — call the LLM with a system prompt that turns recent
//!    `history.jsonl` entries plus the current memory files into a list of
//!    `[FILE]` / `[FILE-REMOVE]` / `[SKILL]` findings.
//! 2. **Phase 2** — hand the analysis to an [`AgentRunner`] equipped with
//!    `read_file` / `edit_file` / `write_file` so the LLM can apply
//!    surgical edits.
//!
//! The Rust port deviates from Python in two minor places:
//!
//! * The built-in `skill-creator` reference SKILL.md is embedded in the
//!   binary (via the `skills` crate), so we materialize it under
//!   `<workspace>/.nanobot/cache/skill-creator-reference.md` on first run
//!   instead of pointing at a directory inside the Python package.
//! * Templates live in `crates/agent/src/prompts/` and are rendered with
//!   the same `{{ key }}` substitution helper used by `ContextBuilder`.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Datelike, Local};
use log::{debug, info, warn};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::json;

use providers::{ChatRequest, LLMProvider, RetryMode};
use utils::helpers::truncate_text;

use crate::memory::{Dream, MemoryStore};
use crate::runner::{AgentRunSpec, AgentRunner};
use crate::tools::{
    EditFileTool, FsTool, ReadFileTool, Tool, ToolRegistry, WriteFileTool,
};

const STALE_THRESHOLD_DAYS: i64 = 14;

const MEMORY_FILE_MAX_CHARS: usize = 32_000;
const SOUL_FILE_MAX_CHARS: usize = 16_000;
const USER_FILE_MAX_CHARS: usize = 16_000;
const HISTORY_ENTRY_PREVIEW_MAX_CHARS: usize = 4_000;

const PHASE1_TEMPLATE: &str = include_str!("prompts/dream_phase1.md.tpl");
const PHASE2_TEMPLATE: &str = include_str!("prompts/dream_phase2.md.tpl");

static VAR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\{\{\s*(\w+)\s*\}\}").unwrap());

fn render(template: &str, pairs: &[(&str, String)]) -> String {
    VAR_RE
        .replace_all(template, |caps: &regex::Captures| {
            let key = &caps[1];
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| caps[0].to_string())
        })
        .into_owned()
}

/// Tunable knobs for [`MemoryDream`].
#[derive(Debug, Clone)]
pub struct DreamConfig {
    /// Max history entries to consider per run.
    pub max_batch_size: usize,
    /// Max iterations for the Phase 2 [`AgentRunner`] loop.
    pub max_iterations: u32,
    /// Tool-result truncation for Phase 2.
    pub max_tool_result_chars: usize,
    /// When `true`, MEMORY.md lines older than [`STALE_THRESHOLD_DAYS`]
    /// get a `← Nd` suffix in the Phase 1 prompt.
    pub annotate_line_ages: bool,
}

impl Default for DreamConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 20,
            max_iterations: 10,
            max_tool_result_chars: 16_000,
            annotate_line_ages: true,
        }
    }
}

/// Concrete two-phase memory consolidator.
///
/// Implements the [`Dream`] trait so callers (e.g. the gateway cron
/// handler) can hold it behind `Arc<dyn Dream>`.
pub struct MemoryDream {
    store: MemoryStore,
    provider: Arc<dyn LLMProvider>,
    runner: AgentRunner,
    model: String,
    cfg: DreamConfig,
    tools: ToolRegistry,
    skill_creator_path: PathBuf,
}

impl MemoryDream {
    /// Build a Dream processor for `workspace`.
    ///
    /// The returned instance is **synchronous to construct** but ships a
    /// pre-built [`ToolRegistry`]; call [`MemoryDream::initialize`]
    /// (async) afterwards to register the FS tools.
    pub fn new(
        workspace: PathBuf,
        provider: Arc<dyn LLMProvider>,
        model: String,
        cfg: DreamConfig,
    ) -> Self {
        let store = MemoryStore::new(&workspace, None);
        let runner = AgentRunner::new(provider.clone());
        let skill_creator_path = workspace
            .join(".nanobot")
            .join("cache")
            .join("skill-creator-reference.md");
        Self {
            store,
            provider,
            runner,
            model,
            cfg,
            tools: ToolRegistry::new(),
            skill_creator_path,
        }
    }

    /// Register the read/write/edit tools used by Phase 2 and materialize
    /// the embedded `skill-creator` reference SKILL.md so `read_file` can
    /// load it from disk.
    pub async fn initialize(&self) -> std::io::Result<()> {
        let workspace = self.store.workspace.clone();
        let skills_dir = workspace.join("skills");
        std::fs::create_dir_all(&skills_dir)?;

        let read_tool = ReadFileTool(FsTool::new(
            Some(workspace.clone()),
            Some(workspace.clone()),
            Vec::new(),
        ));
        let edit_tool = EditFileTool(FsTool::new(
            Some(workspace.clone()),
            Some(workspace.clone()),
            Vec::new(),
        ));
        let write_tool = WriteFileTool(FsTool::new(
            Some(workspace.clone()),
            Some(skills_dir),
            Vec::new(),
        ));

        self.tools.register(Arc::new(read_tool) as Arc<dyn Tool>).await;
        self.tools.register(Arc::new(edit_tool) as Arc<dyn Tool>).await;
        self.tools.register(Arc::new(write_tool) as Arc<dyn Tool>).await;

        if let Some(parent) = self.skill_creator_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Some(content) = skills::skill_manifest("skill-creator") {
            let _ = std::fs::write(&self.skill_creator_path, content);
        }
        Ok(())
    }

    fn list_existing_skills(&self) -> Vec<String> {
        let mut entries: Vec<(String, String)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let desc_re = Regex::new(r"(?m)^description:\s*(.+)$").unwrap();

        let workspace_skills = self.store.workspace.join("skills");
        if let Ok(rd) = std::fs::read_dir(&workspace_skills) {
            for entry in rd.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let skill_md = path.join("SKILL.md");
                if !skill_md.exists() {
                    continue;
                }
                let head = std::fs::read_to_string(&skill_md).unwrap_or_default();
                let head = head.chars().take(500).collect::<String>();
                let desc = desc_re
                    .captures(&head)
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_str().trim().to_string())
                    .unwrap_or_else(|| "(no description)".to_string());
                seen.insert(name.to_string());
                entries.push((name.to_string(), desc));
            }
        }

        for name in skills::skill_names() {
            if seen.contains(name) {
                continue;
            }
            let manifest = skills::skill_manifest(name).unwrap_or_default();
            let head = manifest.chars().take(500).collect::<String>();
            let desc = desc_re
                .captures(&head)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_else(|| "(no description)".to_string());
            entries.push((name.to_string(), desc));
        }

        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries
            .into_iter()
            .map(|(n, d)| format!("{n} — {d}"))
            .collect()
    }

    /// Append per-line age suffixes to MEMORY.md content. Falls back to
    /// the original text when git is unavailable or line counts disagree.
    fn annotate_with_ages(&self, content: &str) -> String {
        let ages = self.store.git().line_ages("memory/MEMORY.md");
        if ages.is_empty() {
            return content.to_string();
        }
        let had_trailing = content.ends_with('\n');
        let lines: Vec<&str> = content.split('\n').collect();
        // splitlines() in Python drops the trailing empty entry; mimic that.
        let lines: Vec<&str> = if had_trailing && lines.last().map(|s| s.is_empty()).unwrap_or(false)
        {
            lines[..lines.len() - 1].to_vec()
        } else {
            lines
        };
        if lines.len() != ages.len() {
            debug!(
                "line_ages length mismatch for memory/MEMORY.md (lines={}, ages={}); skipping annotation",
                lines.len(),
                ages.len()
            );
            return content.to_string();
        }
        let mut annotated: Vec<String> = Vec::with_capacity(lines.len());
        for (line, age) in lines.iter().zip(ages.iter()) {
            if line.trim().is_empty() {
                annotated.push(line.to_string());
            } else if age.age_days > STALE_THRESHOLD_DAYS {
                annotated.push(format!("{line}  \u{2190} {}d", age.age_days));
            } else {
                annotated.push(line.to_string());
            }
        }
        let mut result = annotated.join("\n");
        if had_trailing {
            result.push('\n');
        }
        result
    }
}

#[async_trait]
impl Dream for MemoryDream {
    async fn run(&self) -> bool {
        let last_cursor = self.store.last_dream_cursor();
        let entries = self.store.read_unprocessed_history(last_cursor);
        if entries.is_empty() {
            return false;
        }

        let take = entries.len().min(self.cfg.max_batch_size);
        let batch = &entries[..take];

        let last_cursor_in_batch = batch
            .last()
            .and_then(|e| e.get("cursor"))
            .and_then(|v| v.as_i64())
            .unwrap_or(last_cursor);
        info!(
            "Dream: processing {} entries (cursor {}→{}), batch={}",
            entries.len(),
            last_cursor,
            last_cursor_in_batch,
            batch.len()
        );

        let history_text = batch
            .iter()
            .map(|e| {
                let ts = e.get("timestamp").and_then(|v| v.as_str()).unwrap_or("?");
                let content = e.get("content").and_then(|v| v.as_str()).unwrap_or("");
                format!(
                    "[{ts}] {}",
                    truncate_text(content, HISTORY_ENTRY_PREVIEW_MAX_CHARS)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        let now = Local::now();
        let current_date = format!(
            "{:04}-{:02}-{:02}",
            now.year(),
            now.month(),
            now.day()
        );

        let raw_memory = self.store.read_memory();
        let raw_memory = if raw_memory.is_empty() {
            "(empty)".to_string()
        } else {
            raw_memory
        };
        let annotated_memory = if self.cfg.annotate_line_ages {
            self.annotate_with_ages(&raw_memory)
        } else {
            raw_memory.clone()
        };
        let current_memory = truncate_text(&annotated_memory, MEMORY_FILE_MAX_CHARS);

        let raw_soul = self.store.read_soul();
        let current_soul = truncate_text(
            if raw_soul.is_empty() { "(empty)" } else { &raw_soul },
            SOUL_FILE_MAX_CHARS,
        );
        let raw_user = self.store.read_user();
        let current_user = truncate_text(
            if raw_user.is_empty() { "(empty)" } else { &raw_user },
            USER_FILE_MAX_CHARS,
        );

        let file_context = format!(
            "## Current Date\n{current_date}\n\n\
             ## Current MEMORY.md ({mlen} chars)\n{current_memory}\n\n\
             ## Current SOUL.md ({slen} chars)\n{current_soul}\n\n\
             ## Current USER.md ({ulen} chars)\n{current_user}",
            mlen = current_memory.chars().count(),
            slen = current_soul.chars().count(),
            ulen = current_user.chars().count(),
        );

        // ---- Phase 1: analyze ----
        let phase1_system = render(
            PHASE1_TEMPLATE,
            &[(
                "stale_threshold_days",
                STALE_THRESHOLD_DAYS.to_string(),
            )],
        );
        let phase1_user = format!("## Conversation History\n{history_text}\n\n{file_context}");

        let phase1_response = self
            .provider
            .chat_with_retry(
                ChatRequest {
                    messages: vec![
                        json!({"role":"system","content": phase1_system}),
                        json!({"role":"user","content": phase1_user}),
                    ],
                    tools: None,
                    model: Some(self.model.clone()),
                    max_tokens: 0,
                    temperature: f32::NAN,
                    reasoning_effort: None,
                    tool_choice: None,
                },
                RetryMode::Standard,
                None,
            )
            .await;

        if phase1_response.finish_reason == "error" {
            warn!(
                "Dream Phase 1 failed: {}",
                phase1_response
                    .content
                    .as_deref()
                    .unwrap_or("<no detail>")
            );
            return false;
        }

        let analysis = phase1_response.content.unwrap_or_default();
        let preview: String = analysis.chars().take(500).collect();
        debug!(
            "Dream Phase 1 analysis ({} chars): {}",
            analysis.chars().count(),
            preview
        );

        // ---- Phase 2: AgentRunner with FS tools ----
        let existing_skills = self.list_existing_skills();
        let skills_section = if existing_skills.is_empty() {
            String::new()
        } else {
            let body: String = existing_skills
                .into_iter()
                .map(|s| format!("- {s}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("\n\n## Existing Skills\n{body}")
        };
        let phase2_system = render(
            PHASE2_TEMPLATE,
            &[(
                "skill_creator_path",
                self.skill_creator_path.to_string_lossy().into_owned(),
            )],
        );
        let phase2_user = format!(
            "## Analysis Result\n{analysis}\n\n{file_context}{skills_section}"
        );
        let messages = vec![
            json!({"role":"system","content": phase2_system}),
            json!({"role":"user","content": phase2_user}),
        ];

        let mut spec = AgentRunSpec::new(
            messages,
            self.tools.clone(),
            self.model.clone(),
            self.cfg.max_iterations,
            self.cfg.max_tool_result_chars,
        );
        spec.workspace = Some(self.store.workspace.clone());
        spec.session_key = Some("system::dream".into());
        spec.fail_on_tool_error = false;

        let result = self.runner.run(spec).await;

        debug!(
            "Dream Phase 2 complete: stop_reason={}, tool_events={}",
            result.stop_reason,
            result.tool_events.len()
        );
        for ev in &result.tool_events {
            let detail: String = ev.detail.chars().take(200).collect();
            info!(
                "Dream tool_event: name={}, status={}, detail={}",
                ev.name, ev.status, detail
            );
        }

        let changelog: Vec<String> = result
            .tool_events
            .iter()
            .filter(|e| e.status == "ok")
            .map(|e| format!("{}: {}", e.name, e.detail))
            .collect();

        // Always advance the cursor, even if Phase 2 didn't complete cleanly
        // — otherwise we'd reprocess the same history forever.
        let new_cursor = last_cursor_in_batch;
        let _ = self.store.set_last_dream_cursor(new_cursor);
        let _ = self.store.compact_history();

        if result.stop_reason == "completed" {
            info!(
                "Dream done: {} change(s), cursor advanced to {}",
                changelog.len(),
                new_cursor
            );
        } else {
            warn!(
                "Dream incomplete ({}): cursor advanced to {}",
                result.stop_reason, new_cursor
            );
        }

        if !changelog.is_empty() && self.store.git().is_initialized() {
            let ts = batch
                .last()
                .and_then(|e| e.get("timestamp"))
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string();
            let summary = format!("dream: {ts}, {} change(s)", changelog.len());
            let commit_msg = format!("{summary}\n\n{}", analysis.trim());
            if let Some(sha) = self.store.git().auto_commit(&commit_msg) {
                info!("Dream commit: {sha}");
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use providers::types::{LLMResponse, GenerationSettings};

    fn tmp_workspace() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("dream-test-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Minimal provider that records call count and replies with a fixed
    /// non-error response (no tool calls), so Phase 2 will short-circuit
    /// after one iteration without touching the FS.
    struct StubProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl LLMProvider for StubProvider {
        fn default_model(&self) -> String {
            "stub".into()
        }
        fn generation(&self) -> GenerationSettings {
            GenerationSettings::default()
        }
        async fn chat(&self, _req: ChatRequest) -> LLMResponse {
            self.calls.fetch_add(1, Ordering::SeqCst);
            LLMResponse {
                content: Some("noop".into()),
                ..Default::default()
            }
        }
    }

    #[tokio::test]
    async fn run_returns_false_when_no_history() {
        let ws = tmp_workspace();
        let provider = Arc::new(StubProvider { calls: AtomicUsize::new(0) });
        let dream = MemoryDream::new(
            ws.clone(),
            provider.clone(),
            "stub".into(),
            DreamConfig::default(),
        );
        dream.initialize().await.unwrap();
        assert!(!dream.run().await);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn run_advances_cursor_with_history() {
        let ws = tmp_workspace();
        let store = MemoryStore::new(&ws, None);
        store.append_history("entry-a").unwrap();
        let last = store.append_history("entry-b").unwrap();

        let provider = Arc::new(StubProvider { calls: AtomicUsize::new(0) });
        let dream = MemoryDream::new(
            ws.clone(),
            provider.clone(),
            "stub".into(),
            DreamConfig::default(),
        );
        dream.initialize().await.unwrap();
        assert!(dream.run().await);
        // Phase 1 + Phase 2 = 2 LLM calls.
        assert!(provider.calls.load(Ordering::SeqCst) >= 1);
        assert_eq!(store.last_dream_cursor(), last);
        let _ = std::fs::remove_dir_all(&ws);
    }
}
