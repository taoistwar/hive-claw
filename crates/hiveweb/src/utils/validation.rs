//! Boundary input validation for admin-facing fields.
//! Phone format per data-model.md §Admin: `^1[3-9]\d{9}$` (CN mainland mobile).

use crate::utils::error::AppError;

/// Validate a CN mainland mobile number.
/// Accept: 11 digits, starts with `1`, second digit in `3..=9`.
pub fn validate_phone(phone: &str) -> Result<(), AppError> {
    if phone.len() != 11 {
        return Err(AppError::BadRequest(format!(
            "Phone must be exactly 11 digits, got {}",
            phone.len()
        )));
    }
    let mut chars = phone.chars();
    let first = chars.next();
    let second = chars.next();
    let all_digits = phone.chars().all(|c| c.is_ascii_digit());
    if !all_digits || first != Some('1') || !matches!(second, Some('3'..='9')) {
        return Err(AppError::BadRequest(format!(
            "Phone format invalid: {} (expected ^1[3-9]\\d{{9}}$)",
            phone
        )));
    }
    Ok(())
}

/// Validate a nickname per data-model.md §Admin (2–20 chars).
pub fn validate_nickname(nickname: &str) -> Result<(), AppError> {
    let len = nickname.chars().count();
    if !(2..=20).contains(&len) {
        return Err(AppError::BadRequest(format!(
            "Nickname must be 2-20 characters, got {}",
            len
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_phone() {
        assert!(validate_phone("13800138000").is_ok());
        assert!(validate_phone("18810154696").is_ok());
    }

    #[test]
    fn reject_invalid_phone() {
        assert!(validate_phone("not-a-phone").is_err());
        assert!(validate_phone("12800138000").is_err()); // 2nd digit < 3
        assert!(validate_phone("23800138000").is_err()); // 1st digit != 1
        assert!(validate_phone("138001380001").is_err()); // 12 digits
        assert!(validate_phone("1380013800").is_err()); // 10 digits
    }

    #[test]
    fn nickname_bounds() {
        assert!(validate_nickname("ab").is_ok());
        assert!(validate_nickname(&"a".repeat(20)).is_ok());
        assert!(validate_nickname("a").is_err());
        assert!(validate_nickname(&"a".repeat(21)).is_err());
    }
}
