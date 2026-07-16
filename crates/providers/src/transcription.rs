//! Voice transcription providers (OpenAI Whisper + Groq).
//!
//! Port of `nanobot.providers.transcription`. Both backends speak the
//! same OpenAI-compatible multipart-form endpoint; only the default
//! URL and model name differ, so we share one implementation.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use log::{error, warn};
use reqwest::multipart::{Form, Part};
use serde_json::Value;

/// Common trait every transcription backend implements.
#[async_trait]
pub trait TranscriptionProvider: Send + Sync {
    /// Transcribe an audio file on disk. Returns the plain text output
    /// or an empty string when the request fails / auth is missing.
    async fn transcribe(&self, file_path: &Path) -> String;
}

fn env_or<'a>(explicit: Option<&'a str>, keys: &[&str]) -> Option<String> {
    if let Some(v) = explicit {
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    for k in keys {
        if let Ok(v) = std::env::var(k) {
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// Shared config + request logic for both backends.
#[derive(Debug, Clone)]
pub struct TranscriptionConfig {
    pub api_key: Option<String>,
    pub api_url: String,
    pub model: String,
    pub language: Option<String>,
    pub timeout: Duration,
}

impl TranscriptionConfig {
    fn new(api_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: None,
            api_url: api_url.into(),
            model: model.into(),
            language: None,
            timeout: Duration::from_secs(60),
        }
    }
}

async fn do_transcribe(cfg: &TranscriptionConfig, path: &Path) -> String {
    let Some(api_key) = cfg.api_key.as_ref() else {
        warn!("Transcription API key not configured");
        return String::new();
    };
    if !path.exists() {
        error!("Audio file not found: {}", path.display());
        return String::new();
    }

    let bytes = match tokio::fs::read(path).await {
        Ok(b) => b,
        Err(e) => {
            error!("Failed to read audio file '{}': {e}", path.display());
            return String::new();
        }
    };
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("audio")
        .to_string();

    let file_part = Part::bytes(bytes).file_name(filename);
    let mut form = Form::new()
        .part("file", file_part)
        .text("model", cfg.model.clone());
    if let Some(lang) = cfg.language.as_ref() {
        form = form.text("language", lang.clone());
    }

    let client = match reqwest::Client::builder().timeout(cfg.timeout).build() {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to build HTTP client: {e}");
            return String::new();
        }
    };

    let response = match client
        .post(&cfg.api_url)
        .header("Authorization", format!("Bearer {api_key}"))
        .multipart(form)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            error!("Transcription request failed: {e}");
            return String::new();
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        error!("Transcription HTTP {status}: {text}");
        return String::new();
    }
    let body: Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            error!("Transcription bad JSON: {e}");
            return String::new();
        }
    };
    body.get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// OpenAI Whisper (`whisper-1`) transcription.
pub struct OpenAITranscriptionProvider {
    cfg: TranscriptionConfig,
}

impl OpenAITranscriptionProvider {
    pub fn new(api_key: Option<&str>, api_base: Option<&str>, language: Option<&str>) -> Self {
        let api_url = env_or(api_base, &["OPENAI_TRANSCRIPTION_BASE_URL"])
            .unwrap_or_else(|| "https://api.openai.com/v1/audio/transcriptions".into());
        let api_key = env_or(api_key, &["OPENAI_API_KEY"]);
        let mut cfg = TranscriptionConfig::new(api_url, "whisper-1");
        cfg.api_key = api_key;
        cfg.language = language.map(String::from);
        Self { cfg }
    }

    pub fn path(&self, file_path: impl Into<PathBuf>) -> PathBuf {
        file_path.into()
    }
}

#[async_trait]
impl TranscriptionProvider for OpenAITranscriptionProvider {
    async fn transcribe(&self, file_path: &Path) -> String {
        do_transcribe(&self.cfg, file_path).await
    }
}

/// Groq Whisper (`whisper-large-v3`) transcription.
pub struct GroqTranscriptionProvider {
    cfg: TranscriptionConfig,
}

impl GroqTranscriptionProvider {
    pub fn new(api_key: Option<&str>, api_base: Option<&str>, language: Option<&str>) -> Self {
        let api_url = env_or(api_base, &["GROQ_BASE_URL"])
            .unwrap_or_else(|| "https://api.groq.com/openai/v1/audio/transcriptions".into());
        let api_key = env_or(api_key, &["GROQ_API_KEY"]);
        let mut cfg = TranscriptionConfig::new(api_url, "whisper-large-v3");
        cfg.api_key = api_key;
        cfg.language = language.map(String::from);
        Self { cfg }
    }
}

#[async_trait]
impl TranscriptionProvider for GroqTranscriptionProvider {
    async fn transcribe(&self, file_path: &Path) -> String {
        do_transcribe(&self.cfg, file_path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helpers: these tests avoid std::env::set_var (unsafe in Rust 2024)
    // by constructing TranscriptionConfig values directly whenever we
    // would otherwise rely on environment fallbacks.

    #[tokio::test]
    async fn no_api_key_returns_empty() {
        let cfg = TranscriptionConfig {
            api_key: None,
            api_url: "http://127.0.0.1:1".into(),
            model: "whisper-1".into(),
            language: None,
            timeout: Duration::from_secs(1),
        };
        let out = do_transcribe(&cfg, Path::new("/nonexistent.mp3")).await;
        assert_eq!(out, "");
    }

    #[tokio::test]
    async fn missing_file_returns_empty() {
        let cfg = TranscriptionConfig {
            api_key: Some("key".into()),
            api_url: "http://127.0.0.1:1".into(),
            model: "whisper-1".into(),
            language: None,
            timeout: Duration::from_secs(1),
        };
        let out = do_transcribe(&cfg, Path::new("/nonexistent.mp3")).await;
        assert_eq!(out, "");
    }

    #[test]
    fn groq_explicit_overrides_beat_env() {
        let p = GroqTranscriptionProvider::new(Some("k"), Some("http://example/g"), Some("zh"));
        assert_eq!(p.cfg.model, "whisper-large-v3");
        assert_eq!(p.cfg.api_url, "http://example/g");
        assert_eq!(p.cfg.api_key.as_deref(), Some("k"));
        assert_eq!(p.cfg.language.as_deref(), Some("zh"));
    }

    #[test]
    fn openai_explicit_overrides_beat_env() {
        let p = OpenAITranscriptionProvider::new(Some("abc"), Some("http://x/"), None);
        assert_eq!(p.cfg.api_key.as_deref(), Some("abc"));
        assert_eq!(p.cfg.api_url, "http://x/");
        assert_eq!(p.cfg.model, "whisper-1");
    }
}
