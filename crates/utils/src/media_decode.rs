//! Helpers for decoding `data:...;base64,...` URLs to disk (port of
//! `nanobot.utils.media_decode`).

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use once_cell::sync::Lazy;
use regex::Regex;

use crate::helpers::safe_filename;

pub const DEFAULT_MAX_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_FILE_SIZE: usize = DEFAULT_MAX_BYTES;

static DATA_URL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)^data:([^;]+);base64,(.+)$").unwrap());

/// Raised when a decoded payload exceeds the caller's size limit.
#[derive(Debug, thiserror::Error)]
pub enum MediaDecodeError {
    #[error("File exceeds {limit_mb}MB limit")]
    FileSizeExceeded { limit_mb: usize },
}

/// Decode a `data:<mime>;base64,<payload>` URL and persist it.
///
/// Returns `Ok(Some(absolute_path))` on success, `Ok(None)` when the URL
/// shape or the base64 payload itself is malformed, and
/// `Err(MediaDecodeError::FileSizeExceeded)` when the decoded payload is
/// larger than `max_bytes` (default 10 MB).
pub fn save_base64_data_url(
    data_url: &str,
    media_dir: &Path,
    max_bytes: Option<usize>,
) -> Result<Option<PathBuf>, MediaDecodeError> {
    let Some(caps) = DATA_URL_RE.captures(data_url) else {
        return Ok(None);
    };
    let mime_type = caps.get(1).unwrap().as_str();
    let payload = caps.get(2).unwrap().as_str();
    let raw = match B64.decode(payload.as_bytes()) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    let limit = max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
    if raw.len() > limit {
        return Err(MediaDecodeError::FileSizeExceeded {
            limit_mb: limit / (1024 * 1024),
        });
    }
    let ext = mime_guess::get_mime_extensions_str(mime_type)
        .and_then(|exts| exts.first().copied())
        .map(|e| format!(".{e}"))
        .unwrap_or_else(|| ".bin".into());

    let stem = uuid::Uuid::new_v4().simple().to_string();
    let short_stem: String = stem.chars().take(12).collect();
    let filename = format!("{short_stem}{ext}");
    let dest = media_dir.join(safe_filename(&filename));
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(&dest, &raw).ok();
    Ok(Some(dest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_shape_returns_none() {
        let d = tempdir();
        assert!(
            save_base64_data_url("not-a-data-url", &d, None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn roundtrip_png() {
        let d = tempdir();
        let raw = b"\x89PNG\r\n\x1a\nHELLO";
        let data_url = format!("data:image/png;base64,{}", B64.encode(raw));
        let path = save_base64_data_url(&data_url, &d, None).unwrap().unwrap();
        assert!(path.exists());
        let got = fs::read(&path).unwrap();
        assert_eq!(got, raw);
    }

    fn tempdir() -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("nanobot-media-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&p).unwrap();
        p
    }
}
