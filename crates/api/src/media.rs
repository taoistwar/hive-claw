//! Media upload helpers.
//!
//! Ports the pieces of `nanobot.utils.media_decode` + `nanobot.utils.helpers`
//! that the API server actually uses: safe filename sanitisation and
//! `data:` URL decoding into the workspace media directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use regex::Regex;
use thiserror::Error;
use uuid::Uuid;

/// Hard cap on any single uploaded file (mirrors Python default).
pub const MAX_FILE_SIZE: usize = 20 * 1024 * 1024;

/// Raised when an upload exceeds [`MAX_FILE_SIZE`].
#[derive(Debug, Error)]
#[error("{0}")]
pub struct FileSizeExceeded(pub String);

static UNSAFE_CHARS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[\\/:*?"<>|\u0000-\u001f]"#).unwrap());

/// Sanitise a user-supplied filename so it's safe to write to disk.
///
/// * Strips path components (only the basename is retained).
/// * Replaces Windows-reserved / control characters with `_`.
/// * Collapses leading dots and whitespace.
/// * Falls back to `"upload.bin"` when the result would be empty.
pub fn safe_filename(input: &str) -> String {
    let input = input.trim();
    if input.is_empty() {
        return "upload.bin".into();
    }
    // Drop directory components (handle both separators).
    let base = input
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or(input);
    let cleaned = UNSAFE_CHARS.replace_all(base, "_").to_string();
    let trimmed = cleaned.trim_start_matches('.').trim();
    if trimmed.is_empty() {
        "upload.bin".into()
    } else {
        trimmed.to_string()
    }
}

static DATA_URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^data:([^;]+);base64,(.+)$").unwrap());

/// Persist a `data:<mime>;base64,<payload>` URL to `media_dir` and
/// return the full saved path. `None` indicates the URL was not a
/// base64 data URL, the payload was empty/oversized, or decoding failed.
///
/// The filename pattern matches Python: `<12-char hex>.<ext>`.
pub fn save_base64_data_url(url: &str, media_dir: &Path) -> Option<PathBuf> {
    let caps = DATA_URL_RE.captures(url)?;
    let mime = caps.get(1)?.as_str();
    let payload = caps.get(2)?.as_str();

    let bytes = STANDARD.decode(payload).ok()?;
    if bytes.is_empty() || bytes.len() > MAX_FILE_SIZE {
        return None;
    }

    let ext = mime_guess::get_mime_extensions_str(mime)
        .and_then(|exts| exts.first().copied())
        .unwrap_or(match mime {
            "image/jpeg" => "jpg",
            "image/png" => "png",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => "bin",
        });

    let stem = Uuid::new_v4().simple().to_string();
    let name = format!("{}.{ext}", &stem[..12]);
    fs::create_dir_all(media_dir).ok()?;
    let path = media_dir.join(name);
    fs::write(&path, &bytes).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("rustbot_api_media_{tag}_{nanos}"));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn safe_filename_strips_path_and_unsafe_chars() {
        assert_eq!(safe_filename("../etc/passwd"), "passwd");
        assert_eq!(safe_filename("C:\\bad<name>.jpg"), "bad_name_.jpg");
        assert_eq!(safe_filename(""), "upload.bin");
        assert_eq!(safe_filename("   "), "upload.bin");
        // Leading dot stripped (no hidden file allowed).
        assert_eq!(safe_filename(".hidden"), "hidden");
    }

    #[test]
    fn save_base64_png_roundtrip() {
        let dir = unique_dir("png");
        // 1x1 transparent PNG.
        let payload = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR4nGNgAAIAAAUAAeImBZsAAAAASUVORK5CYII=";
        let url = format!("data:image/png;base64,{payload}");
        let saved = save_base64_data_url(&url, &dir).unwrap();
        assert!(saved.exists());
        assert!(saved.extension().unwrap().to_str().unwrap().eq_ignore_ascii_case("png"));
        let bytes = fs::read(&saved).unwrap();
        assert!(bytes.len() > 10);
    }

    #[test]
    fn save_rejects_non_data_url() {
        let dir = unique_dir("reject");
        assert!(save_base64_data_url("https://example.com/a.png", &dir).is_none());
    }

    #[test]
    fn save_rejects_bad_base64() {
        let dir = unique_dir("bad");
        assert!(save_base64_data_url("data:image/png;base64,@@@", &dir).is_none());
    }
}
