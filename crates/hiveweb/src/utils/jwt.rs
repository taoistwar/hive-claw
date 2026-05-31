use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

const JWT_SECRET_ENV: &str = "JWT_SECRET";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub admin_id: Option<i64>,
    pub user_id: Option<i64>,
    pub role: i8,
    pub exp: usize,
    pub iat: usize,
}

impl Claims {
    /// Return true if this is a user token (not admin)
    pub fn is_user(&self) -> bool {
        self.user_id.is_some() && self.admin_id.is_none()
    }

    /// Return true if this is an admin token
    pub fn is_admin(&self) -> bool {
        self.admin_id.is_some()
    }
}

pub fn create_admin_token(admin_id: i64, role: i8) -> anyhow::Result<String> {
    let secret = std::env::var(JWT_SECRET_ENV).unwrap_or_else(|_| "default-secret-change-me".to_string());
    let now = Utc::now();
    let expiry = now + Duration::hours(24);

    let claims = Claims {
        admin_id: Some(admin_id),
        user_id: None,
        role,
        exp: expiry.timestamp() as usize,
        iat: now.timestamp() as usize,
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;

    Ok(token)
}

pub fn create_user_token(user_id: i64) -> anyhow::Result<String> {
    let secret = std::env::var(JWT_SECRET_ENV).unwrap_or_else(|_| "default-secret-change-me".to_string());
    let now = Utc::now();
    let expiry = now + Duration::hours(24);

    let claims = Claims {
        admin_id: None,
        user_id: Some(user_id),
        role: 0, // users don't have admin roles
        exp: expiry.timestamp() as usize,
        iat: now.timestamp() as usize,
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;

    Ok(token)
}

pub fn verify_token(token: &str) -> anyhow::Result<Claims> {
    let secret = std::env::var(JWT_SECRET_ENV).unwrap_or_else(|_| "default-secret-change-me".to_string());

    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )?;

    Ok(token_data.claims)
}
