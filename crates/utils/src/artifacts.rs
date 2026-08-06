/// Artifact persistence helpers for generated media.
use std::collections::HashMap;
use std::path::PathBuf;

use base64::Engine;
use chrono::{DateTime, Local};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::helpers::{detect_image_mime, ensure_dir};

/// Path resolver for media directory - abstracts the `config::paths::get_media_dir` call.
/// Implementors should return the media root path.
pub trait MediaDirResolver: Send + Sync {
    fn get_media_dir(&self) -> PathBuf;
}

static DATA_IMAGE_RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
    regex::Regex::new(r"^data:(image/[A-Za-z0-9.+-]+);base64,(.*)$").unwrap()
});

const MIME_EXTENSIONS: &[(&str, &str)] = &[
    ("image/png", ".png"),
    ("image/jpeg", ".jpg"),
    ("image/webp", ".webp"),
    ("image/gif", ".gif"),
];

#[derive(Error, Debug)]
pub enum ArtifactError {
    #[error("expected a base64 image data URL")]
    InvalidDataUrl,
    #[error("invalid base64 image payload")]
    InvalidBase64,
    #[error("unsupported or unrecognized image data")]
    UnsupportedImage,
    #[error("unsupported image MIME type: {0}")]
    UnsupportedMime(String),
    #[error("save_dir must not be empty")]
    EmptySaveDir,
    #[error("save_dir must be a safe relative path")]
    UnsafeSaveDir,
    #[error("artifact directory escapes media root")]
    EscapesMediaRoot,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Decode a base64 image data URL and return `(bytes, mime)`.
pub fn decode_image_data_url(data_url: &str) -> Result<(Vec<u8>, String), ArtifactError> {
    let captures = DATA_IMAGE_RE
        .captures(data_url.trim())
        .ok_or(ArtifactError::InvalidDataUrl)?;

    let declared_mime = captures.get(1).unwrap().as_str().to_string();
    let encoded = captures.get(2).unwrap().as_str();

    let raw = base64::prelude::BASE64_STANDARD
        .decode(encoded)
        .map_err(|_| ArtifactError::InvalidBase64)?;

    let detected_mime = detect_image_mime(&raw).ok_or(ArtifactError::UnsupportedImage)?;

    let final_mime = if declared_mime != detected_mime {
        detected_mime.to_string()
    } else {
        declared_mime
    };

    Ok((raw, final_mime))
}

fn safe_relative_dir(save_dir: &str) -> Result<PathBuf, ArtifactError> {
    let normalized = save_dir.replace('\\', "/").trim_matches('/').to_string();
    if normalized.is_empty() {
        return Err(ArtifactError::EmptySaveDir);
    }
    if normalized.starts_with('/')
        || normalized
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(ArtifactError::UnsafeSaveDir);
    }
    Ok(PathBuf::from(normalized))
}

fn artifact_root(
    save_dir: &str,
    media_resolver: &dyn MediaDirResolver,
) -> Result<PathBuf, ArtifactError> {
    let media_root = media_resolver.get_media_dir();
    let media_root = media_root.canonicalize().unwrap_or(media_root);
    let root = media_root.join(safe_relative_dir(save_dir)?);
    let root = root.canonicalize().unwrap_or(root);
    root.strip_prefix(&media_root)
        .map_err(|_| ArtifactError::EscapesMediaRoot)?;
    Ok(root)
}

/// Persist a generated image and sidecar metadata under the media root.
#[expect(
    clippy::too_many_arguments,
    reason = "preserve the public API until an artifact request type is introduced"
)]
pub fn store_generated_image_artifact(
    data_url: &str,
    prompt: &str,
    model: &str,
    media_resolver: &dyn MediaDirResolver,
    source_images: Option<&[String]>,
    save_dir: &str,
    provider: &str,
    created_at: Option<DateTime<Local>>,
) -> Result<HashMap<String, Value>, ArtifactError> {
    let (raw, mime) = decode_image_data_url(data_url)?;

    let ext = MIME_EXTENSIONS
        .iter()
        .find(|(m, _)| *m == mime)
        .map(|(_, e)| *e)
        .ok_or_else(|| ArtifactError::UnsupportedMime(mime.clone()))?;

    let now = created_at.unwrap_or_else(Local::now);
    let day_dir = ensure_dir(
        &artifact_root(save_dir, media_resolver)?.join(now.format("%Y-%m-%d").to_string()),
    )?;

    let artifact_id = format!("img_{}", &Uuid::new_v4().simple().to_string()[..12]);
    let image_path = day_dir.join(format!("{artifact_id}{ext}"));
    let metadata_path = day_dir.join(format!("{artifact_id}.json"));

    std::fs::write(&image_path, &raw)?;

    let metadata: HashMap<String, Value> = [
        ("id".to_string(), Value::String(artifact_id.clone())),
        (
            "path".to_string(),
            Value::String(image_path.to_string_lossy().to_string()),
        ),
        ("mime".to_string(), Value::String(mime.clone())),
        ("prompt".to_string(), Value::String(prompt.to_string())),
        ("model".to_string(), Value::String(model.to_string())),
        ("provider".to_string(), Value::String(provider.to_string())),
        (
            "source_images".to_string(),
            Value::Array(
                source_images
                    .unwrap_or(&[])
                    .iter()
                    .map(|s| Value::String(s.clone()))
                    .collect(),
            ),
        ),
        ("created_at".to_string(), Value::String(now.to_rfc3339())),
    ]
    .into_iter()
    .collect();

    std::fs::write(
        &metadata_path,
        serde_json::to_string_pretty(&metadata).unwrap(),
    )?;

    Ok(metadata)
}

/// Return the compact structured result exposed to the LLM.
pub fn generated_image_tool_result(artifacts: &[HashMap<String, Value>]) -> String {
    let result = serde_json::json!({
        "artifacts": artifacts,
        "next_step": "Use these artifact paths as reference_images for follow-up edits. Call the message tool with the artifact paths in the media parameter to deliver the images to the user. Keep raw paths internal unless the user asks for debug details.",
    });
    serde_json::to_string(&result).unwrap_or_default()
}
