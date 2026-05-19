/// Minimal snake-case conversion: insert `_` before uppercase letters and
/// lowercase the whole thing. Enough for typical config keys like
/// `"OpenRouter"` -> `"open_router"`. We explicitly do **not** collapse
/// runs because that matches the Python behaviour for things like
/// `"GitHubCopilot"` -> `"git_hub_copilot"` (handled separately below).
pub fn to_snake(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 4);
    let mut prev_lower = false;
    for ch in input.chars() {
        if ch.is_ascii_uppercase() {
            if prev_lower {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            prev_lower = false;
        } else {
            out.push(ch);
            prev_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        }
    }
    out
}
