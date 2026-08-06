//! Minimal replacement for `oauth_cli_kit` — just enough for the Codex
//! and GitHub Copilot providers.
//!
//! The Python project persists tokens under `~/.<app>/<filename>`, and
//! for Codex it additionally falls back to `~/.codex/auth.json` (written
//! by the official Codex CLI).  We mirror both behaviours so existing
//! user installs keep working.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// OAuth token persisted on disk.
///
/// Timestamps are epoch milliseconds (to match the existing Python
/// layout).  `refresh` / `account_id` are optional depending on the
/// upstream provider.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OAuthToken {
    pub access: String,
    #[serde(default)]
    pub refresh: String,
    /// Absolute expiry in epoch milliseconds.  `0` means "unknown /
    /// long-lived" — don't treat it as expired.
    #[serde(default)]
    pub expires: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

impl OAuthToken {
    pub fn is_expired(&self) -> bool {
        if self.expires <= 0 {
            return false;
        }
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        now_ms >= self.expires
    }
}

/// File-backed token store. Mirrors the Python `FileTokenStorage`.
#[derive(Debug, Clone)]
pub struct FileTokenStorage {
    pub token_filename: String,
    pub app_name: String,
    pub import_codex_cli: bool,
}

impl FileTokenStorage {
    pub fn new(
        token_filename: impl Into<String>,
        app_name: impl Into<String>,
        import_codex_cli: bool,
    ) -> Self {
        Self {
            token_filename: token_filename.into(),
            app_name: app_name.into(),
            import_codex_cli,
        }
    }

    /// Primary storage path: `~/.<app>/<filename>`.
    pub fn path(&self) -> PathBuf {
        let home = home_dir();
        home.join(format!(".{}", self.app_name))
            .join(&self.token_filename)
    }

    /// Codex CLI fallback path: `~/.codex/auth.json`.
    pub fn codex_cli_path() -> PathBuf {
        home_dir().join(".codex").join("auth.json")
    }

    pub fn load(&self) -> Option<OAuthToken> {
        if let Some(tok) = read_token(&self.path()) {
            return Some(tok);
        }
        if self.import_codex_cli {
            return read_codex_cli_token(&Self::codex_cli_path());
        }
        None
    }

    pub fn save(&self, token: &OAuthToken) -> std::io::Result<()> {
        let path = self.path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(token)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(&path, json)?;
        Ok(())
    }

    pub fn clear(&self) -> std::io::Result<()> {
        let path = self.path();
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }
}

fn read_token(path: &Path) -> Option<OAuthToken> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice::<OAuthToken>(&bytes).ok()
}

/// Parse the Codex CLI's `~/.codex/auth.json` which has a different shape:
/// `{ "OPENAI_API_KEY": null, "tokens": { "access_token": "...", "refresh_token": "...", "id_token": "..." } }`
/// plus an optional `account_id` nested inside the id_token claim payload.
fn read_codex_cli_token(path: &Path) -> Option<OAuthToken> {
    let bytes = fs::read(path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;

    // Preferred shape: the CLI nests tokens under "tokens".
    let tokens = value.get("tokens").unwrap_or(&value);
    let access = tokens
        .get("access_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if access.is_empty() {
        return None;
    }
    let refresh = tokens
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let account_id = tokens
        .get("account_id")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| {
            decode_account_id_from_id_token(tokens.get("id_token").and_then(|v| v.as_str()))
        });

    Some(OAuthToken {
        access,
        refresh,
        expires: 0,
        account_id,
    })
}

/// Best-effort decode of the `chatgpt_account_id` claim from a JWT id_token.
/// We only split on `.` and base64-decode the middle segment; failure
/// falls through to `None` rather than erroring.
fn decode_account_id_from_id_token(id_token: Option<&str>) -> Option<String> {
    let token = id_token?;
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    // Standard unpadded base64url.
    let decoded = base64_url_decode(parts[1])?;
    let json = String::from_utf8(decoded).ok()?;
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    // The claim name varies: chatgpt_account_id (new) or account_id (older).
    if let Some(id) = v
        .get("https://api.openai.com/auth")
        .and_then(|a| a.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
    {
        return Some(id.into());
    }
    v.get("chatgpt_account_id")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| {
            v.get("account_id")
                .and_then(|v| v.as_str())
                .map(String::from)
        })
}

fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    // Convert base64url → base64 with padding.
    let padded: String = input.replace('-', "+").replace('_', "/");
    let pad = (4 - padded.len() % 4) % 4;
    let padded = format!("{}{}", padded, "=".repeat(pad));
    simple_b64_decode(&padded)
}

/// Minimal RFC4648 base64 decode (A-Z,a-z,0-9,+,/) with `=` padding.
fn simple_b64_decode(input: &str) -> Option<Vec<u8>> {
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    let mut out: Vec<u8> = Vec::with_capacity(input.len() * 3 / 4);
    for ch in input.chars() {
        if ch == '=' {
            break;
        }
        let val: u32 = match ch {
            'A'..='Z' => ch as u32 - 'A' as u32,
            'a'..='z' => ch as u32 - 'a' as u32 + 26,
            '0'..='9' => ch as u32 - '0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            _ => return None,
        };
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

fn home_dir() -> PathBuf {
    if let Ok(h) = std::env::var("HOME") {
        return PathBuf::from(h);
    }
    if let Ok(h) = std::env::var("USERPROFILE") {
        return PathBuf::from(h);
    }
    PathBuf::from(".")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_home(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("rustbot_oauth_{tag}_{nanos}"));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn expired_detection_obeys_epoch() {
        let mut t = OAuthToken {
            expires: 1, // 1970
            ..OAuthToken::default()
        };
        assert!(t.is_expired());
        t.expires = 0;
        assert!(!t.is_expired());
    }

    #[test]
    fn round_trip_save_load() {
        let home = unique_home("rtrip");
        // SAFETY: test-only; single-threaded with respect to HOME since no
        // other test in this module reads HOME during its assertions.
        unsafe { std::env::set_var("HOME", &home) };
        let storage = FileTokenStorage::new("test.json", "rustbot_test", false);
        assert!(storage.load().is_none());
        let tok = OAuthToken {
            access: "access-1".into(),
            refresh: "refresh-1".into(),
            expires: 12345,
            account_id: Some("acc".into()),
        };
        storage.save(&tok).unwrap();
        let loaded = storage.load().unwrap();
        assert_eq!(loaded.access, "access-1");
        assert_eq!(loaded.account_id.as_deref(), Some("acc"));
        storage.clear().unwrap();
        assert!(storage.load().is_none());
    }

    #[test]
    fn base64_url_decode_known_vector() {
        // "hello" -> aGVsbG8 (no padding).
        let out = base64_url_decode("aGVsbG8").unwrap();
        assert_eq!(out, b"hello");
    }

    #[test]
    fn decode_account_id_from_jwt_claim() {
        // Header.Payload.Signature where Payload = {"chatgpt_account_id":"abc"}.
        let payload = r#"{"chatgpt_account_id":"abc"}"#;
        // base64 encode manually.
        let b64: String = {
            let alpha = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let bytes = payload.as_bytes();
            let mut out = String::new();
            let mut i = 0;
            while i < bytes.len() {
                let b0 = bytes[i];
                let b1 = if i + 1 < bytes.len() { bytes[i + 1] } else { 0 };
                let b2 = if i + 2 < bytes.len() { bytes[i + 2] } else { 0 };
                out.push(alpha[(b0 >> 2) as usize] as char);
                out.push(alpha[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
                if i + 1 < bytes.len() {
                    out.push(alpha[(((b1 & 0x0F) << 2) | (b2 >> 6)) as usize] as char);
                } else {
                    out.push('=');
                }
                if i + 2 < bytes.len() {
                    out.push(alpha[(b2 & 0x3F) as usize] as char);
                } else {
                    out.push('=');
                }
                i += 3;
            }
            out
        };
        let token = format!("hdr.{b64}.sig");
        let got = decode_account_id_from_id_token(Some(&token));
        assert_eq!(got.as_deref(), Some("abc"));
    }
}
