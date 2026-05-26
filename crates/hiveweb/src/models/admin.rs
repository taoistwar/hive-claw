use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::models::Role;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Admin {
    pub id: i64,
    pub phone: String,
    pub nickname: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub role: i8,
    pub status: i8,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
}

impl Admin {
    pub fn get_role(&self) -> Role {
        Role::try_from(self.role).unwrap_or(Role::Normal)
    }
}
