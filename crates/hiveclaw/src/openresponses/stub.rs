use super::{ContentItem, OpenResponse, OutputItem, ResponseStatus, Usage};

/// The canonical zh-CN placeholder reply text. Kept deterministic so
/// contract tests can assert on it.
pub const PLACEHOLDER_TEXT: &str = "HiveClaw 占位回复：已收到你的请求。";

/// Produce a complete synchronous response body for a placeholder run.
pub fn build_response(id: &str, model: &str, created: i64, input_chars: usize) -> OpenResponse {
    let text = PLACEHOLDER_TEXT.to_string();
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

/// Stream chunks for the placeholder reply. Returns 2-4 deltas whose
/// concatenation equals [`PLACEHOLDER_TEXT`]. Used by the streaming branch
/// of the handler.
pub fn stream_chunks() -> Vec<&'static str> {
    // PLACEHOLDER_TEXT = "HiveClaw 占位回复：已收到你的请求。"
    // Three deterministic chunks.
    vec!["HiveClaw 占位回复：", "已收到", "你的请求。"]
}

fn approx_tokens(text: &str) -> u32 {
    // Stub-quality token approximation: 1 token ~ 4 chars for ASCII / 1 char for CJK.
    // We don't need precision; the contract only requires presence and total = input+output.
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
    fn chunks_concatenate_to_placeholder_text() {
        let joined: String = stream_chunks().into_iter().collect();
        assert_eq!(joined, PLACEHOLDER_TEXT);
    }
}
