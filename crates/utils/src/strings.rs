//! String manipulation utilities.
//!
//! Provides `to_snake` to match Python's `pydantic.alias_generators.to_snake`.

/// Convert a CamelCase or mixed-case string to snake_case.
///
/// Mirrors Python's `pydantic.alias_generators.to_snake` behavior for the
/// subset of inputs we encounter in provider names.
pub fn to_snake(name: &str) -> String {
    let mut result = String::with_capacity(name.len() * 2);
    let mut prev_is_lower = false;
    let mut prev_is_upper = false;

    for (i, ch) in name.char_indices() {
        if ch.is_uppercase() {
            // Insert underscore before uppercase letter if:
            // - it's not the first character, AND
            // - previous char was lowercase, OR
            // - previous char was uppercase and next char is lowercase
            let next_is_lower = name[i + ch.len_utf8()..]
                .chars()
                .next()
                .map(|c| c.is_lowercase())
                .unwrap_or(false);
            if i > 0 && (prev_is_lower || (prev_is_upper && next_is_lower)) {
                result.push('_');
            }
            result.push(ch.to_lowercase().next().unwrap_or(ch));
            prev_is_lower = false;
            prev_is_upper = true;
        } else {
            result.push(ch);
            prev_is_lower = true;
            prev_is_upper = false;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_camel() {
        assert_eq!(to_snake("OpenAI"), "open_ai");
        assert_eq!(to_snake("GitHub"), "git_hub");
    }

    #[test]
    fn test_already_snake() {
        assert_eq!(to_snake("open_ai"), "open_ai");
    }

    #[test]
    fn test_mixed() {
        assert_eq!(to_snake("AzureOpenAI"), "azure_open_ai");
        assert_eq!(to_snake("Bedrock"), "bedrock");
    }

    #[test]
    fn test_consecutive_uppercase() {
        assert_eq!(to_snake("API"), "api");
        assert_eq!(to_snake("XMLParser"), "xml_parser");
    }
}
