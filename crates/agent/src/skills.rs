//! Skills loader — port of `nanobot.agent.skills`.
//!
//! Workspace skills live under `<workspace>/skills/<name>/SKILL.md`.
//! Builtin skills are embedded at compile time via the `skills` crate.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

/// Opening `---`, YAML body, closing `---` on its own line. Supports CRLF.
static STRIP_SKILL_FRONTMATTER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)^---\s*\r?\n(.*?)\r?\n---\s*\r?\n?").unwrap());

#[derive(Debug, Clone)]
pub struct SkillEntry {
    pub name: String,
    pub path: String,
    pub source: SkillSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSource {
    Workspace,
    Builtin,
}

impl SkillSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Builtin => "builtin",
        }
    }
}

/// Loader for agent skills.
pub struct SkillsLoader {
    pub workspace: PathBuf,
    pub workspace_skills: PathBuf,
    pub disabled_skills: HashSet<String>,
    /// Optional override of the built-in skills directory (used in tests).
    /// When `None`, the [`skills`] crate's embedded tree is consulted.
    pub builtin_skills_override: Option<PathBuf>,
}

impl SkillsLoader {
    pub fn new(workspace: PathBuf, disabled_skills: Option<HashSet<String>>) -> Self {
        let workspace_skills = workspace.join("skills");
        Self {
            workspace,
            workspace_skills,
            disabled_skills: disabled_skills.unwrap_or_default(),
            builtin_skills_override: None,
        }
    }

    pub fn with_builtin_override(mut self, path: PathBuf) -> Self {
        self.builtin_skills_override = Some(path);
        self
    }

    fn fs_entries(
        &self,
        base: &Path,
        source: SkillSource,
        skip: &HashSet<String>,
    ) -> Vec<SkillEntry> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(base) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()).map(String::from) else {
                continue;
            };
            let skill_md = path.join("SKILL.md");
            if !skill_md.exists() {
                continue;
            }
            if skip.contains(&name) {
                continue;
            }
            out.push(SkillEntry {
                name,
                path: skill_md.to_string_lossy().to_string(),
                source,
            });
        }
        out
    }

    fn builtin_entries(&self, skip: &HashSet<String>) -> Vec<SkillEntry> {
        if let Some(dir) = &self.builtin_skills_override {
            return self.fs_entries(dir, SkillSource::Builtin, skip);
        }
        skills::skill_names()
            .into_iter()
            .filter_map(|name| {
                if skip.contains(name) {
                    return None;
                }
                let path = format!("{name}/SKILL.md");
                skills::get_file(&path).map(|_| SkillEntry {
                    name: name.to_string(),
                    path: format!("<builtin>/{path}"),
                    source: SkillSource::Builtin,
                })
            })
            .collect()
    }

    /// List all available skills. `filter_unavailable` drops skills whose
    /// requirements (bins/env) aren't satisfied.
    pub fn list_skills(&self, filter_unavailable: bool) -> Vec<SkillEntry> {
        let empty = HashSet::new();
        let mut entries = self.fs_entries(&self.workspace_skills, SkillSource::Workspace, &empty);
        let workspace_names: HashSet<String> = entries.iter().map(|e| e.name.clone()).collect();
        entries.extend(self.builtin_entries(&workspace_names));

        if !self.disabled_skills.is_empty() {
            entries.retain(|e| !self.disabled_skills.contains(&e.name));
        }

        if filter_unavailable {
            entries.retain(|e| self.check_requirements(&self.skill_meta(&e.name)));
        }
        entries
    }

    /// Load a skill's SKILL.md content.
    pub fn load_skill(&self, name: &str) -> Option<String> {
        let workspace_path = self.workspace_skills.join(name).join("SKILL.md");
        if workspace_path.exists() {
            return std::fs::read_to_string(&workspace_path).ok();
        }
        if let Some(dir) = &self.builtin_skills_override {
            let path = dir.join(name).join("SKILL.md");
            if path.exists() {
                return std::fs::read_to_string(&path).ok();
            }
            return None;
        }
        skills::skill_manifest(name).map(|s| s.to_string())
    }

    /// Load specific skills formatted for inclusion in agent context.
    pub fn load_skills_for_context(&self, names: &[&str]) -> String {
        let parts: Vec<String> = names
            .iter()
            .filter_map(|n| {
                let markdown = self.load_skill(n)?;
                Some(format!(
                    "### Skill: {n}\n\n{}",
                    strip_frontmatter(&markdown)
                ))
            })
            .collect();
        parts.join("\n\n---\n\n")
    }

    /// Build a summary of all skills (name, description, path, availability).
    pub fn build_skills_summary(&self, exclude: Option<&HashSet<String>>) -> String {
        let all = self.list_skills(false);
        if all.is_empty() {
            return String::new();
        }
        let mut lines: Vec<String> = Vec::new();
        for entry in all {
            if exclude.is_some_and(|ex| ex.contains(&entry.name)) {
                continue;
            }
            let meta = self.skill_meta(&entry.name);
            let available = self.check_requirements(&meta);
            let desc = self
                .skill_description(&entry.name)
                .unwrap_or_else(|| entry.name.clone());
            if available {
                lines.push(format!("- **{}** — {desc}  `{}`", entry.name, entry.path));
            } else {
                let missing = self.missing_requirements(&meta);
                let suffix = if missing.is_empty() {
                    " (unavailable)".to_string()
                } else {
                    format!(" (unavailable: {missing})")
                };
                lines.push(format!(
                    "- **{}** — {desc}{suffix}  `{}`",
                    entry.name, entry.path
                ));
            }
        }
        lines.join("\n")
    }

    /// Get the list of skills marked `always=true` whose requirements are met.
    pub fn get_always_skills(&self) -> Vec<String> {
        self.list_skills(true)
            .into_iter()
            .filter(|e| {
                let meta = self.skill_metadata(&e.name).unwrap_or_default();
                let parsed_meta = parse_nanobot_metadata(meta.get("metadata"));
                parsed_meta.get("always").is_some_and(is_truthy)
                    || meta.get("always").is_some_and(is_truthy)
            })
            .map(|e| e.name)
            .collect()
    }

    /// Return the raw frontmatter metadata map for a skill.
    pub fn skill_metadata(&self, name: &str) -> Option<HashMap<String, Value>> {
        let content = self.load_skill(name)?;
        if !content.starts_with("---") {
            return None;
        }
        let m = STRIP_SKILL_FRONTMATTER.captures(&content)?;
        let body = m.get(1)?.as_str();
        let parsed: serde_yaml::Value = serde_yaml::from_str(body).ok()?;
        let map = parsed.as_mapping()?;
        let mut out = HashMap::new();
        for (k, v) in map {
            let key = k.as_str()?.to_string();
            let val = yaml_to_json(v);
            out.insert(key, val);
        }
        Some(out)
    }

    fn skill_meta(&self, name: &str) -> HashMap<String, Value> {
        let raw = self.skill_metadata(name).unwrap_or_default();
        parse_nanobot_metadata(raw.get("metadata"))
    }

    fn skill_description(&self, name: &str) -> Option<String> {
        let meta = self.skill_metadata(name)?;
        meta.get("description")
            .and_then(|v| v.as_str())
            .map(String::from)
    }

    fn check_requirements(&self, skill_meta: &HashMap<String, Value>) -> bool {
        let requires = match skill_meta.get("requires") {
            Some(Value::Object(m)) => m,
            _ => return true,
        };
        let bins = requires
            .get("bins")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default();
        let envs = requires
            .get("env")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default();

        bins.iter().all(|b| which(b).is_some())
            && envs
                .iter()
                .all(|e| std::env::var(e).map(|v| !v.is_empty()).unwrap_or(false))
    }

    fn missing_requirements(&self, skill_meta: &HashMap<String, Value>) -> String {
        let requires = match skill_meta.get("requires") {
            Some(Value::Object(m)) => m,
            _ => return String::new(),
        };
        let mut parts: Vec<String> = Vec::new();
        if let Some(bins) = requires.get("bins").and_then(|v| v.as_array()) {
            for b in bins.iter().filter_map(|v| v.as_str()) {
                if which(b).is_none() {
                    parts.push(format!("CLI: {b}"));
                }
            }
        }
        if let Some(envs) = requires.get("env").and_then(|v| v.as_array()) {
            for e in envs.iter().filter_map(|v| v.as_str()) {
                if std::env::var(e).map(|v| v.is_empty()).unwrap_or(true) {
                    parts.push(format!("ENV: {e}"));
                }
            }
        }
        parts.join(", ")
    }
}

/// Extract nanobot/openclaw metadata from a frontmatter field.
fn parse_nanobot_metadata(raw: Option<&Value>) -> HashMap<String, Value> {
    let data = match raw {
        Some(Value::Object(map)) => map.clone(),
        Some(Value::String(s)) => match serde_json::from_str::<Value>(s) {
            Ok(Value::Object(m)) => m,
            _ => return HashMap::new(),
        },
        _ => return HashMap::new(),
    };
    let payload = data
        .get("nanobot")
        .or_else(|| data.get("openclaw"))
        .cloned()
        .unwrap_or(Value::Null);
    match payload {
        Value::Object(m) => m.into_iter().collect(),
        _ => HashMap::new(),
    }
}

fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => {
            n.as_i64().is_some_and(|n| n != 0) || n.as_f64().is_some_and(|n| n != 0.0)
        }
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn strip_frontmatter(content: &str) -> String {
    if !content.starts_with("---") {
        return content.to_string();
    }
    if let Some(m) = STRIP_SKILL_FRONTMATTER.find(content) {
        return content[m.end()..].trim().to_string();
    }
    content.to_string()
}

fn yaml_to_json(v: &serde_yaml::Value) -> Value {
    match v {
        serde_yaml::Value::Null => Value::Null,
        serde_yaml::Value::Bool(b) => Value::Bool(*b),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::from(i)
            } else if let Some(u) = n.as_u64() {
                Value::from(u)
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            } else {
                Value::Null
            }
        }
        serde_yaml::Value::String(s) => Value::String(s.clone()),
        serde_yaml::Value::Sequence(items) => {
            Value::Array(items.iter().map(yaml_to_json).collect())
        }
        serde_yaml::Value::Mapping(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let key = match k {
                    serde_yaml::Value::String(s) => s.clone(),
                    other => serde_yaml::to_string(other).unwrap_or_default(),
                };
                out.insert(key.trim().to_string(), yaml_to_json(v));
            }
            Value::Object(out)
        }
        serde_yaml::Value::Tagged(t) => yaml_to_json(&t.value),
    }
}

/// Minimal `which` implementation — checks `PATH` for an executable.
fn which(bin: &str) -> Option<PathBuf> {
    let path_env = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_env) {
        let candidate = dir.join(bin);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let candidate_exe = dir.join(format!("{bin}.exe"));
            if candidate_exe.is_file() {
                return Some(candidate_exe);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_workspace() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("skills-test-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn workspace_skills_shadow_builtins() {
        let ws = tmp_workspace();
        write_file(
            &ws.join("skills/cron/SKILL.md"),
            "---\ndescription: overridden\n---\nbody",
        );
        let loader = SkillsLoader::new(ws.clone(), None);
        let names: Vec<String> = loader
            .list_skills(false)
            .into_iter()
            .filter(|e| e.name == "cron")
            .map(|e| format!("{}:{}", e.name, e.source.as_str()))
            .collect();
        assert_eq!(names, vec!["cron:workspace".to_string()]);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn strip_frontmatter_removes_header() {
        let s = "---\ndescription: x\n---\nhello";
        assert_eq!(strip_frontmatter(s), "hello");
    }
}
