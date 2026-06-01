use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

const ADMIN_SECRET_ENV: &str = "JWT_ADMIN_SECRET";
const USER_SECRET_ENV: &str = "JWT_USER_SECRET";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub admin_id: Option<i64>,
    pub user_id: Option<i64>,
    pub role: i8,
    pub token_type: String,
    pub exp: usize,
    pub iat: usize,
}

impl Claims {
    pub fn is_user(&self) -> bool {
        self.token_type == "user"
    }

    pub fn is_admin(&self) -> bool {
        self.token_type == "admin"
    }
}

fn get_admin_secret() -> String {
    std::env::var(ADMIN_SECRET_ENV)
        .unwrap_or_else(|_| "admin-secret-change-me-in-production".to_string())
}

fn get_user_secret() -> String {
    std::env::var(USER_SECRET_ENV)
        .unwrap_or_else(|_| "user-secret-change-me-in-production".to_string())
}

pub fn create_admin_token(admin_id: i64, role: i8) -> anyhow::Result<String> {
    let secret = get_admin_secret();
    let now = Utc::now();
    let expiry = now + Duration::hours(24);

    let claims = Claims {
        admin_id: Some(admin_id),
        user_id: None,
        role,
        token_type: "admin".to_string(),
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
    let secret = get_user_secret();
    let now = Utc::now();
    let expiry = now + Duration::hours(24);

    let claims = Claims {
        admin_id: None,
        user_id: Some(user_id),
        role: 0,
        token_type: "user".to_string(),
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

pub fn verify_admin_token(token: &str) -> anyhow::Result<Claims> {
    let secret = get_admin_secret();
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )?;

    if !token_data.claims.is_admin() {
        anyhow::bail!("Not an admin token");
    }

    Ok(token_data.claims)
}

pub fn verify_user_token(token: &str) -> anyhow::Result<Claims> {
    let secret = get_user_secret();
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )?;

    if !token_data.claims.is_user() {
        anyhow::bail!("Not a user token");
    }

    Ok(token_data.claims)
}
