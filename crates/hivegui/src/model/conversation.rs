//! Conversation state per `specs/001-hiveclaw-hivegui/data-model.md`.
//! Enforces the FR-008a single-pending-turn invariant.

use chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub struct ConversationId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub struct TurnId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub struct PendingTurnId(pub Uuid);

impl From<PendingTurnId> for TurnId {
    fn from(p: PendingTurnId) -> Self {
        TurnId(p.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Author {
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub enum TurnContent {
    UserText { text: String },
    AssistantText { buffer: String },
}

#[derive(Debug, Clone)]
pub struct AssistantReply {
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct TurnError {
    pub kind: TurnErrorKind,
    pub message_zh: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnErrorKind {
    Unreachable,
    TransportFailure,
    ServerError,
}

#[derive(Debug, Clone)]
pub enum TurnStatus {
    Pending,
    Delivered,
    Failed { retryable: bool },
}

#[derive(Debug, Clone)]
pub struct ConversationTurn {
    pub id: TurnId,
    pub author: Author,
    pub content: TurnContent,
    pub status: TurnStatus,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub error: Option<TurnError>,
}

#[derive(Debug, Error)]
pub enum BusyError {
    #[error("conversation has a pending turn; wait for it to resolve before sending another")]
    Pending,
}

#[derive(Debug, Error)]
pub enum RetryError {
    #[error("turn id not found")]
    NotFound,
    #[error("turn is not in a retryable failed state")]
    NotRetryable,
    #[error("another turn is already pending")]
    Busy,
}

#[derive(Debug)]
pub struct Conversation {
    id: ConversationId,
    turns: Vec<ConversationTurn>,
    pending: Option<PendingTurnId>,
    started_at: DateTime<Utc>,
}

impl Conversation {
    pub fn new() -> Self {
        Conversation {
            id: ConversationId(Uuid::new_v4()),
            turns: Vec::new(),
            pending: None,
            started_at: Utc::now(),
        }
    }

    pub fn id(&self) -> ConversationId {
        self.id
    }

    pub fn started_at(&self) -> DateTime<Utc> {
        self.started_at
    }

    pub fn turns(&self) -> &[ConversationTurn] {
        &self.turns
    }

    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    pub fn pending(&self) -> Option<PendingTurnId> {
        self.pending
    }

    /// FR-008a: returns `BusyError` when another turn is already pending.
    pub fn send_user_message(&mut self, text: String) -> Result<PendingTurnId, BusyError> {
        if self.pending.is_some() {
            return Err(BusyError::Pending);
        }
        let turn_id = TurnId(Uuid::new_v4());
        let pending = PendingTurnId(turn_id.0);
        self.turns.push(ConversationTurn {
            id: turn_id,
            author: Author::User,
            content: TurnContent::UserText { text },
            status: TurnStatus::Pending,
            created_at: Utc::now(),
            completed_at: None,
            error: None,
        });
        self.pending = Some(pending);
        Ok(pending)
    }

    /// Append an arriving streaming chunk to the user-turn-correlated
    /// assistant reply buffer. Idempotently creates an assistant turn the
    /// first time it is called for the active pending user turn.
    pub fn append_assistant_chunk(&mut self, pending: PendingTurnId, chunk: &str) {
        if self.pending != Some(pending) {
            return;
        }
        // Find or create the assistant turn that pairs with this pending.
        let already_has_assistant = self.turns.iter().any(|t| {
            matches!(t.author, Author::Assistant)
                && matches!(&t.content, TurnContent::AssistantText { .. })
                && t.created_at >= self.user_turn_started(pending)
        });
        if !already_has_assistant {
            self.turns.push(ConversationTurn {
                id: TurnId(Uuid::new_v4()),
                author: Author::Assistant,
                content: TurnContent::AssistantText {
                    buffer: chunk.to_string(),
                },
                status: TurnStatus::Delivered,
                created_at: Utc::now(),
                completed_at: Some(Utc::now()),
                error: None,
            });
        } else if let Some(t) = self.turns.iter_mut().rev().find(|t| {
            matches!(t.author, Author::Assistant)
                && matches!(&t.content, TurnContent::AssistantText { .. })
        }) {
            if let TurnContent::AssistantText { buffer } = &mut t.content {
                buffer.push_str(chunk);
            }
        }
    }

    fn user_turn_started(&self, pending: PendingTurnId) -> DateTime<Utc> {
        self.turns
            .iter()
            .find(|t| t.id.0 == pending.0)
            .map(|t| t.created_at)
            .unwrap_or_else(Utc::now)
    }

    /// Resolve the pending user turn as Delivered and append the assistant
    /// reply. If the assistant turn was already created via streaming
    /// chunks, this finalises its buffer to the canonical text and clears
    /// `pending`.
    pub fn record_assistant_reply(&mut self, pending: PendingTurnId, reply: AssistantReply) {
        if self.pending != Some(pending) {
            return;
        }
        // Finalise (or create) the assistant turn.
        let assistant_exists = self.turns.iter().any(|t| {
            matches!(t.author, Author::Assistant) && t.created_at >= self.user_turn_started(pending)
        });
        if assistant_exists {
            if let Some(t) = self
                .turns
                .iter_mut()
                .rev()
                .find(|t| matches!(t.author, Author::Assistant))
            {
                if let TurnContent::AssistantText { buffer } = &mut t.content {
                    *buffer = reply.text.clone();
                }
                t.completed_at = Some(Utc::now());
                t.status = TurnStatus::Delivered;
            }
        } else {
            self.turns.push(ConversationTurn {
                id: TurnId(Uuid::new_v4()),
                author: Author::Assistant,
                content: TurnContent::AssistantText { buffer: reply.text },
                status: TurnStatus::Delivered,
                created_at: Utc::now(),
                completed_at: Some(Utc::now()),
                error: None,
            });
        }

        // Mark user turn delivered.
        for t in self.turns.iter_mut() {
            if t.id.0 == pending.0 {
                t.status = TurnStatus::Delivered;
                t.completed_at = Some(Utc::now());
            }
        }
        self.pending = None;
    }

    pub fn record_failure(&mut self, pending: PendingTurnId, error: TurnError) {
        if self.pending != Some(pending) {
            return;
        }
        for t in self.turns.iter_mut() {
            if t.id.0 == pending.0 {
                t.status = TurnStatus::Failed { retryable: true };
                t.completed_at = Some(Utc::now());
                t.error = Some(error.clone());
            }
        }
        self.pending = None;
    }

    /// Manual retry: turn a failed user turn back into a Pending turn with
    /// a fresh id. The new pending id is returned so the caller can
    /// re-dispatch the network call.
    pub fn retry(&mut self, failed: TurnId) -> Result<PendingTurnId, RetryError> {
        if self.pending.is_some() {
            return Err(RetryError::Busy);
        }
        let pos = self
            .turns
            .iter()
            .position(|t| t.id == failed)
            .ok_or(RetryError::NotFound)?;
        let is_retryable = matches!(
            self.turns[pos].status,
            TurnStatus::Failed { retryable: true }
        );
        if !is_retryable {
            return Err(RetryError::NotRetryable);
        }
        let text = match &self.turns[pos].content {
            TurnContent::UserText { text } => text.clone(),
            _ => return Err(RetryError::NotRetryable),
        };
        let new_id = TurnId(Uuid::new_v4());
        let pending = PendingTurnId(new_id.0);
        self.turns.push(ConversationTurn {
            id: new_id,
            author: Author::User,
            content: TurnContent::UserText { text },
            status: TurnStatus::Pending,
            created_at: Utc::now(),
            completed_at: None,
            error: None,
        });
        self.pending = Some(pending);
        Ok(pending)
    }

    pub fn dismiss_failure(&mut self, failed: TurnId) {
        if let Some(t) = self.turns.iter_mut().find(|t| t.id == failed) {
            if matches!(t.status, TurnStatus::Failed { .. }) {
                // Leave the turn visible in history but unattached from any
                // retry affordance. We model "dismissed" as cleared error +
                // non-retryable failed status so the UI knows not to render
                // the 重试 button.
                t.status = TurnStatus::Failed { retryable: false };
            }
        }
    }
}

impl Default for Conversation {
    fn default() -> Self {
        Conversation::new()
    }
}
