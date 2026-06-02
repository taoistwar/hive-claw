//! Chat service — admin and user tables are completely separate.
//!
//! Admin tables: chat_sessions_admin + chat_messages_admin
//! User tables: chat_sessions_user + chat_messages_user
//!
//! Ownership semantics:
//!   - Admin session: admin_id must match JWT admin_id (or Super)
//!   - User session: user_id must match JWT user_id

use serde::{Deserialize, Serialize};

use crate::models::{ChatMessageAdmin, ChatMessageUser, ChatSessionAdmin, ChatSessionUser};

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
    Admin(ChatSessionAdmin),
    User(ChatSessionUser),
}

impl SessionListItem {
    pub fn id(&self) -> i64 {
        match self {
            SessionListItem::Admin(s) => s.id,
            SessionListItem::User(s) => s.id,
        }
    }
}
