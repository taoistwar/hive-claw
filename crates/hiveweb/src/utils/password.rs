use crate::utils::error::AppError;

pub fn validate_password(password: &str) -> Result<(), AppError> {
    let character_count = password.chars().count();
    if character_count < 6 {
        return Err(AppError::BadRequest(
            "Password must be at least 6 characters".to_string(),
        ));
    }
    if character_count > 20 {
        return Err(AppError::BadRequest(
            "Password must be at most 20 characters".to_string(),
        ));
    }

    let has_letter = password.chars().any(|c| c.is_ascii_alphabetic());
    let has_digit = password.chars().any(|c| c.is_ascii_digit());

    if !has_letter || !has_digit {
        return Err(AppError::BadRequest(
            "Password must contain both letters and digits".to_string(),
        ));
    }

    Ok(())
}

pub fn hash_password(password: &str) -> Result<String, AppError> {
    validate_password(password)?;
    bcrypt::hash(password, bcrypt::DEFAULT_COST)
        .map_err(|_| AppError::Internal("Failed to hash password".to_string()))
}

pub fn verify_password(password: &str, hash: &str) -> Result<bool, AppError> {
    bcrypt::verify(password, hash)
        .map_err(|_| AppError::Internal("Failed to verify password".to_string()))
}

#[cfg(test)]
mod tests {
    use super::validate_password;

    #[test]
    fn password_requires_at_least_one_ascii_letter() {
        assert!(validate_password("123456").is_err());
        assert!(validate_password("1234中文").is_err());
    }

    #[test]
    fn password_requires_at_least_one_ascii_digit() {
        assert!(validate_password("letters").is_err());
        assert!(validate_password("字母abcdef").is_err());
    }

    #[test]
    fn password_length_counts_unicode_characters_not_utf8_bytes() {
        assert!(validate_password("a1中文界").is_err()); // 5 characters, 11 bytes
        assert!(validate_password("a1中文世界").is_ok()); // 6 characters, 14 bytes

        let twenty_characters = format!("a1{}", "界".repeat(18));
        assert_eq!(twenty_characters.chars().count(), 20);
        assert!(twenty_characters.len() > 20);
        assert!(validate_password(&twenty_characters).is_ok());

        let twenty_one_characters = format!("a1{}", "界".repeat(19));
        assert!(validate_password(&twenty_one_characters).is_err());
    }
}
