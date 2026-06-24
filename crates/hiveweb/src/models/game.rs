use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const MAX_NAME_LENGTH: usize = 50;
pub const MAX_ALIAS_LENGTH: usize = 50;
pub const MAX_ALIASES_COUNT: usize = 20;
pub const DEFAULT_PAGE_SIZE: u64 = 10;
pub const MAX_PAGE_SIZE: u64 = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Game {
    pub id: i64,
    pub name: String,
    pub aliases: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateGameRequest {
    pub name: String,
    pub aliases: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateGameRequest {
    pub name: Option<String>,
    pub aliases: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct GameResponse {
    pub id: i64,
    pub name: String,
    pub aliases: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<Game> for GameResponse {
    fn from(game: Game) -> Self {
        GameResponse {
            id: game.id,
            name: game.name,
            aliases: game.aliases,
            created_at: game.created_at,
            updated_at: game.updated_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct GameListResponse {
    pub total: u64,
    pub games: Vec<GameResponse>,
}
