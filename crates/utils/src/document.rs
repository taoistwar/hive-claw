//! Document text-extraction helpers (port of `nanobot.utils.document`).
//!
//! The Python original depends on heavy native / binding crates for
//! PDF / DOCX / XLSX / PPTX parsing (pypdf, python-docx, openpyxl,
//! python-pptx). Rather than hard-wire equivalents here, we expose a
//! minimal trait so that the concrete extractors can live in their own
//! feature crate.
//!
//! Plain-text and image classification are implemented directly — those
//! are the only behaviours needed to split media into *documents to
//! transcribe* vs. *images to forward*.

use std::fs;
use std::path::Path;

use log::warn;
use mime_guess::from_path;

use crate::helpers::detect_image_mime;

const MAX_EXTRACT_FILE_SIZE: u64 = 50 * 1024 * 1024;
const MAX_TEXT_LENGTH: usize = 200_000;

/// Supported file extensions for text extraction.
pub fn supported_extensions() -> &'static [&'static str] {
    &[
        // Document formats
        ".pdf", ".docx", ".xlsx", ".pptx", // Text formats
        ".txt", ".md", ".csv", ".json", ".xml", ".html", ".htm", ".log", ".yaml", ".yml", ".toml",
        ".ini", ".cfg", // Image formats
        ".png", ".jpg", ".jpeg", ".gif", ".webp",
    ]
}

/// Pluggable extractor for formats that need native dependencies
/// (PDF / DOCX / XLSX / PPTX). Concrete implementations live in their own
/// crate to keep the `utils` dependency graph small.
pub trait BinaryDocumentExtractor: Send + Sync {
    fn extract(&self, path: &Path) -> Option<String>;
}

/// Best-effort text extraction from *path*. Plain-text / image handling is
/// built in; binary document formats need an optional
/// [`BinaryDocumentExtractor`].
pub fn extract_text(
    path: &Path,
    binary_extractor: Option<&dyn BinaryDocumentExtractor>,
) -> Option<String> {
    if !path.exists() {
        return Some(format!("[error: file not found: {}]", path.display()));
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| format!(".{}", s.to_lowercase()))
        .unwrap_or_default();

    match ext.as_str() {
        ".pdf" | ".docx" | ".xlsx" | ".pptx" => match binary_extractor {
            Some(ex) => ex.extract(path),
            None => Some(format!("[error: {} extractor not installed]", &ext[1..])),
        },
        e if is_text_extension(e) => extract_text_file(path),
        ".png" | ".jpg" | ".jpeg" | ".gif" | ".webp" => path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| format!("[image: {n}]")),
        _ => None,
    }
}

fn extract_text_file(path: &Path) -> Option<String> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            warn!("Failed to read text file {}: {e}", path.display());
            return Some(format!("[error: failed to read file: {e}]"));
        }
    };
    let text = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => {
            // Fall back to Latin-1 interpretation: every byte maps to a
            // Unicode scalar value 0–255.
            let bytes = e.into_bytes();
            bytes.iter().map(|b| *b as char).collect()
        }
    };
    Some(truncate(&text, MAX_TEXT_LENGTH))
}

fn truncate(text: &str, max_length: usize) -> String {
    let n = text.chars().count();
    if n <= max_length {
        return text.to_string();
    }
    let head: String = text.chars().take(max_length).collect();
    format!("{head}... (truncated, {n} chars total)")
}

fn is_text_extension(ext: &str) -> bool {
    matches!(
        ext,
        ".txt"
            | ".md"
            | ".csv"
            | ".json"
            | ".xml"
            | ".html"
            | ".htm"
            | ".log"
            | ".yaml"
            | ".yml"
            | ".toml"
            | ".ini"
            | ".cfg"
    )
}

/// Separate images from documents in *media_paths*.
///
/// Documents have their text extracted (when an extractor is provided) and
/// appended to *text*. Only image paths are kept in the returned list so
/// downstream layers only need to handle vision blocks.
pub fn extract_documents(
    mut text: String,
    media_paths: &[String],
    binary_extractor: Option<&dyn BinaryDocumentExtractor>,
    max_file_size: Option<u64>,
) -> (String, Vec<String>) {
    let max_size = max_file_size.unwrap_or(MAX_EXTRACT_FILE_SIZE);
    let mut image_paths: Vec<String> = Vec::new();
    let mut doc_texts: Vec<String> = Vec::new();

    for path_str in media_paths {
        let p = Path::new(path_str);
        if !p.is_file() {
            continue;
        }
        let size = match p.metadata().map(|m| m.len()) {
            Ok(sz) => sz,
            Err(_) => continue,
        };
        if size > max_size {
            warn!(
                "Skipping oversized file for extraction: {} ({:.1} MB > {} MB limit)",
                p.file_name().and_then(|s| s.to_str()).unwrap_or(path_str),
                size as f64 / (1024.0 * 1024.0),
                max_size / (1024 * 1024),
            );
            continue;
        }

        // Classify via magic bytes first, fall back to mime guess.
        let header = match fs::File::open(p).and_then(|mut f| {
            use std::io::Read;
            let mut buf = [0u8; 16];
            let n = f.read(&mut buf)?;
            Ok((buf, n))
        }) {
            Ok((buf, n)) => buf[..n].to_vec(),
            Err(_) => continue,
        };
        let mime = detect_image_mime(&header)
            .map(str::to_string)
            .or_else(|| from_path(p).first_raw().map(str::to_string));
        let is_image = mime
            .as_deref()
            .map(|m| m.starts_with("image/"))
            .unwrap_or(false);
        if is_image {
            image_paths.push(path_str.clone());
        } else if let Some(extracted) = extract_text(p, binary_extractor) {
            if !extracted.starts_with("[error:") {
                let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("file");
                doc_texts.push(format!("[File: {name}]\n{extracted}"));
            }
        }
    }

    if !doc_texts.is_empty() {
        text.push_str("\n\n");
        text.push_str(&doc_texts.join("\n\n"));
    }
    (text, image_paths)
}
