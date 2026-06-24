//! Chat service — user table types.
//!
//! User tables: chat_sessions_user + chat_messages_user
//!
//! Ownership semantics:
//!   - User session: user_id must match JWT user_id

use serde::{Deserialize, Serialize};

use crate::models::ChatSessionUser;

#[derive(Debug, Deserialize)]
pub struct CreateSession {
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PostMessage {
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct SessionList {
    pub items: Vec<SessionListItem>,
    pub total: i64,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum SessionListItem {
    User(ChatSessionUser),
}
