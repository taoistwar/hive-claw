use crate::utils::error::AppError;

pub fn validate_password(password: &str) -> Result<(), AppError> {
    if password.len() < 6 {
        return Err(AppError::BadRequest(
            "Password must be at least 6 characters".to_string(),
        ));
    }
    if password.len() > 20 {
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
    bcrypt::hash(password, bcrypt::DEFAULT_COST).map_err(|e| {
        AppError::Internal(format!("Failed to hash password: {}", e))
    })
}

pub fn verify_password(password: &str, hash: &str) -> Result<bool, AppError> {
    bcrypt::verify(password, hash).map_err(|e| {
        AppError::Internal(format!("Failed to verify password: {}", e))
    })
}
