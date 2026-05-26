//! Logging-safe helpers — apply at every boundary before a value reaches
//! `tracing::info!` / `tracing::error!` etc.
//!
//! Spec FR-021: phone numbers MUST be masked in logs.

/// Mask a CN mainland mobile number for logs: keep the first 3 and last 4
/// digits, replace the middle 4 with `****`.
///
/// `13800138000` → `138****8000`
///
/// Inputs that are not exactly 11 chars are returned with all but the first
/// two characters masked, so we never accidentally leak a non-conforming
/// value either.
pub fn mask_phone(phone: &str) -> String {
    let chars: Vec<char> = phone.chars().collect();
    if chars.len() != 11 {
        if chars.len() <= 2 {
            return "*".repeat(chars.len());
        }
        let mut out: String = chars[..2].iter().collect();
        out.push_str(&"*".repeat(chars.len() - 2));
        return out;
    }
    let mut out = String::with_capacity(11);
    out.extend(chars[..3].iter());
    out.push_str("****");
    out.extend(chars[7..].iter());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_11_digit_phone() {
        assert_eq!(mask_phone("13800138000"), "138****8000");
        assert_eq!(mask_phone("18810154696"), "188****4696");
    }

    #[test]
    fn handles_non_conforming_input() {
        assert_eq!(mask_phone(""), "");
        assert_eq!(mask_phone("12"), "**");
        assert_eq!(mask_phone("12345"), "12***");
    }
}
