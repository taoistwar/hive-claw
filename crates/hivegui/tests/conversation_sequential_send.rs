//! Unit test for the FR-008a single-pending-turn invariant and the
//! T1-T4 state-machine rules in `data-model.md`.

use hivegui::model::conversation::{
    AssistantReply, Author, Conversation, RetryError, SendError, TurnContent, TurnError,
    TurnErrorKind, TurnStatus,
};

#[test]
fn idle_conversation_accepts_send() {
    let mut conv = Conversation::new();
    assert!(!conv.is_busy());
    let pending = conv.send_user_message("hello".to_string(), vec![]).unwrap();
    assert!(conv.is_busy());
    assert_eq!(conv.turns().len(), 1);
    assert_eq!(conv.pending(), Some(pending));
    assert!(matches!(conv.turns()[0].author, Author::User));
    assert!(matches!(conv.turns()[0].status, TurnStatus::Pending));
}

#[test]
fn second_send_while_pending_returns_busy() {
    let mut conv = Conversation::new();
    let _ = conv.send_user_message("first".to_string(), vec![]).unwrap();
    let err = conv
        .send_user_message("second".to_string(), vec![])
        .unwrap_err();
    assert!(matches!(err, SendError::Busy));
    // Only the first turn should be present.
    assert_eq!(conv.turns().len(), 1);
}

#[test]
fn record_assistant_reply_clears_pending_and_appends_turn() {
    let mut conv = Conversation::new();
    let pending = conv.send_user_message("hi".to_string(), vec![]).unwrap();
    conv.record_assistant_reply(
        pending,
        AssistantReply {
            text: "hello back".to_string(),
        },
    );
    assert!(!conv.is_busy());
    assert_eq!(conv.pending(), None);
    assert_eq!(conv.turns().len(), 2);
    assert!(matches!(conv.turns()[0].status, TurnStatus::Delivered));
    assert!(matches!(conv.turns()[1].author, Author::Assistant));
    if let TurnContent::AssistantText { buffer } = &conv.turns()[1].content {
        assert_eq!(buffer, "hello back");
    } else {
        panic!("expected assistant text");
    }
}

#[test]
fn record_failure_marks_retryable_and_clears_pending() {
    let mut conv = Conversation::new();
    let pending = conv.send_user_message("hi".to_string(), vec![]).unwrap();
    conv.record_failure(
        pending,
        TurnError {
            kind: TurnErrorKind::Unreachable,
            message_zh: "HiveClaw 不可达".to_string(),
        },
    );
    assert!(!conv.is_busy());
    assert_eq!(conv.turns().len(), 1);
    match conv.turns()[0].status {
        TurnStatus::Failed { retryable } => assert!(retryable),
        _ => panic!("expected Failed"),
    }
}

#[test]
fn retry_produces_new_pending_with_same_content() {
    let mut conv = Conversation::new();
    let pending = conv.send_user_message("hi".to_string(), vec![]).unwrap();
    let failed_id = conv.turns()[0].id;
    conv.record_failure(
        pending,
        TurnError {
            kind: TurnErrorKind::Unreachable,
            message_zh: "x".into(),
        },
    );
    let new_pending = conv.retry(failed_id).unwrap();
    assert!(conv.is_busy());
    assert_eq!(conv.pending(), Some(new_pending));
    assert_eq!(conv.turns().len(), 2);
    if let TurnContent::UserMessage { text, attachments } = &conv.turns()[1].content {
        assert_eq!(text, "hi");
        assert!(attachments.is_empty());
    } else {
        panic!("expected user text on retry");
    }
}

#[test]
fn retry_when_busy_returns_busy_error() {
    let mut conv = Conversation::new();
    let pending = conv.send_user_message("a".to_string(), vec![]).unwrap();
    let failed_id = conv.turns()[0].id;
    conv.record_failure(
        pending,
        TurnError {
            kind: TurnErrorKind::Unreachable,
            message_zh: "x".into(),
        },
    );
    // Start a new fresh turn so the conversation is busy again.
    let _ = conv.send_user_message("b".to_string(), vec![]).unwrap();
    let err = conv.retry(failed_id).unwrap_err();
    assert!(matches!(err, RetryError::Busy));
}

#[test]
fn dismiss_failure_marks_non_retryable() {
    let mut conv = Conversation::new();
    let pending = conv.send_user_message("hi".to_string(), vec![]).unwrap();
    let failed_id = conv.turns()[0].id;
    conv.record_failure(
        pending,
        TurnError {
            kind: TurnErrorKind::Unreachable,
            message_zh: "x".into(),
        },
    );
    conv.dismiss_failure(failed_id);
    match conv.turns()[0].status {
        TurnStatus::Failed { retryable } => assert!(!retryable),
        _ => panic!("expected Failed after dismiss"),
    }
}
