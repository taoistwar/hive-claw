/// Model information helpers for the onboard wizard.
///
/// Model database / autocomplete is temporarily disabled while litellm is
/// being replaced. All public function signatures are preserved so callers
/// continue to work without changes.

/// Returns an empty list of all models (database temporarily disabled).
pub fn get_all_models() -> Vec<String> {
    vec![]
}

/// Returns `None` for any model lookup (database temporarily disabled).
pub fn find_model_info(_model_name: &str) -> Option<serde_json::Value> {
    None
}

/// Returns `None` for any model context limit query.
pub fn get_model_context_limit(_model: &str, _provider: &str) -> Option<usize> {
    None
}

/// Returns an empty list of model suggestions.
pub fn get_model_suggestions(_partial: &str, _provider: &str, _limit: usize) -> Vec<String> {
    vec![]
}

/// Format token count for display (e.g., 200000 -> "200,000").
pub fn format_token_count(tokens: u64) -> String {
    let s = tokens.to_string();
    let len = s.len();
    let mut result = String::with_capacity(len + len / 3);
    for (i, c) in s.chars().enumerate() {
        let from_end = len - i;
        if from_end < len && from_end % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result
}
