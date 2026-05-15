use super::{AttachmentMeta, ContentItem, OpenResponse, OutputItem, ResponseStatus, Usage};

/// The canonical zh-CN placeholder reply text. Kept deterministic so
/// contract tests can assert on it.
pub const PLACEHOLDER_TEXT: &str = "HiveClaw 占位回复：已收到你的请求。";

/// Render the final placeholder reply text. For text-only requests this
/// equals [`PLACEHOLDER_TEXT`]; when attachments are present, the
/// `附件：…` suffix is appended per contracts/openresponses-v1.md
/// §Stub-reply enrichment.
pub fn build_text(attachments: &[AttachmentMeta]) -> String {
    if attachments.is_empty() {
        return PLACEHOLDER_TEXT.to_string();
    }
    let mut out = String::from(PLACEHOLDER_TEXT);
    out.push_str("附件：");
    for (i, a) in attachments.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&format!(
            "{} ({}, {})",
            a.filename,
            format_size(a.size_bytes),
            a.mime
        ));
    }
    out
}

/// Produce a complete synchronous response body for a placeholder run.
pub fn build_response(
    id: &str,
    model: &str,
    created: i64,
    input_chars: usize,
    attachments: &[AttachmentMeta],
) -> OpenResponse {
    let text = build_text(attachments);
    let output_tokens = approx_tokens(&text);
    let input_tokens = approx_tokens_from_len(input_chars);

    OpenResponse {
        id: id.to_string(),
        object: "response",
        created,
        model: model.to_string(),
        status: ResponseStatus::Completed,
        output: vec![OutputItem {
            kind: "message",
            role: "assistant",
            content: vec![ContentItem {
                kind: "output_text",
                text,
            }],
        }],
        usage: Usage {
            input_tokens,
            output_tokens,
            total_tokens: input_tokens + output_tokens,
        },
    }
}

/// Stream chunks for the placeholder reply. Returns ≥2 deltas whose
/// concatenation equals the result of [`build_text`].
pub fn stream_chunks(attachments: &[AttachmentMeta]) -> Vec<String> {
    let mut chunks: Vec<String> = vec![
        "HiveClaw 占位回复：".to_string(),
        "已收到".to_string(),
        "你的请求。".to_string(),
    ];
    if !attachments.is_empty() {
        // Append the attachment metadata as a single trailing chunk, so the
        // concatenation invariant (sum(deltas) == final text) stays trivial
        // to assert from contract tests.
        let mut tail = String::from("附件：");
        for (i, a) in attachments.iter().enumerate() {
            if i > 0 {
                tail.push_str(", ");
            }
            tail.push_str(&format!(
                "{} ({}, {})",
                a.filename,
                format_size(a.size_bytes),
                a.mime
            ));
        }
        chunks.push(tail);
    }
    chunks
}

/// Format a byte count as a human-readable zh-CN-friendly string.
/// Uses one-decimal precision and binary prefixes: `B`, `KiB`, `MiB`.
pub fn format_size(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let n = bytes as f64;
    if n < KIB {
        format!("{} B", bytes)
    } else if n < MIB {
        format!("{:.1} KiB", n / KIB)
    } else {
        format!("{:.1} MiB", n / MIB)
    }
}

fn approx_tokens(text: &str) -> u32 {
    let n = text.chars().count();
    n as u32
}

fn approx_tokens_from_len(chars: usize) -> u32 {
    chars as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_concatenate_to_text_no_attachments() {
        let joined: String = stream_chunks(&[]).into_iter().collect();
        assert_eq!(joined, PLACEHOLDER_TEXT);
        assert_eq!(joined, build_text(&[]));
    }

    #[test]
    fn chunks_concatenate_to_text_with_attachments() {
        let attachments = vec![
            AttachmentMeta {
                filename: "a.sql".into(),
                mime: "text/plain".into(),
                size_bytes: 1234,
            },
            AttachmentMeta {
                filename: "b.json".into(),
                mime: "application/json".into(),
                size_bytes: 4096,
            },
        ];
        let joined: String = stream_chunks(&attachments).into_iter().collect();
        assert_eq!(joined, build_text(&attachments));
        assert!(joined.contains("a.sql (1.2 KiB, text/plain)"));
        assert!(joined.contains("b.json (4.0 KiB, application/json)"));
    }

    #[test]
    fn format_size_boundaries() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 KiB");
        assert_eq!(format_size(1024 * 1024), "1.0 MiB");
    }
}
