use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct RecommendedGameStrategy {
    pub id: i64,
    pub recommended_game_id: i64,
    pub channel: serde_json::Value,
    pub client_type: serde_json::Value,
    pub strategy: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
