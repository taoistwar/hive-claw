//! Unit test for FR-007b + data-model invariant A1 (count + total budget)
//! on `Conversation::send_user_message`, plus retry-with-attachments
//! coverage so retried turns preserve their attachment list (analysis
//! finding F10 → moved into T065 per the analyse-fix pass).

use hivegui::model::conversation::{
    Attachment, AttachmentId, AttachmentPayload, Conversation, SendError, TurnContent, TurnError,
    TurnErrorKind, MAX_ATTACHMENTS_PER_TURN, TOTAL_ATTACHMENTS_MAX_BYTES,
};
use uuid::Uuid;

fn att(filename: &str, mime: &str, size_bytes: u64) -> Attachment {
    Attachment {
        id: AttachmentId(Uuid::new_v4()),
        filename: filename.into(),
        mime: mime.into(),
        size_bytes,
        payload: AttachmentPayload::Inline {
            base64_data_uri: format!("data:{};base64,YWJj", mime),
        },
    }
}

#[test]
fn empty_text_and_no_attachments_rejected() {
    let mut conv = Conversation::new();
    let err = conv.send_user_message(String::new(), vec![]).unwrap_err();
    assert!(matches!(err, SendError::Empty));
    assert!(!conv.is_busy());
}

#[test]
fn empty_text_with_one_attachment_is_permitted() {
    let mut conv = Conversation::new();
    let pending = conv
        .send_user_message(String::new(), vec![att("a.sql", "text/plain", 100)])
        .unwrap();
    assert!(conv.is_busy());
    assert_eq!(conv.pending(), Some(pending));
    assert_eq!(conv.turns().len(), 1);
    match &conv.turns()[0].content {
        TurnContent::UserMessage { text, attachments } => {
            assert!(text.is_empty());
            assert_eq!(attachments.len(), 1);
            assert_eq!(attachments[0].filename, "a.sql");
        }
        _ => panic!("expected UserMessage"),
    }
}

#[test]
fn too_many_attachments_rejected() {
    let mut conv = Conversation::new();
    let mut atts = Vec::new();
    for i in 0..(MAX_ATTACHMENTS_PER_TURN + 1) {
        atts.push(att(&format!("f{i}.txt"), "text/plain", 10));
    }
    let err = conv.send_user_message("hi".into(), atts).unwrap_err();
    assert!(matches!(err, SendError::TooManyAttachments));
}

#[test]
fn total_too_large_rejected() {
    let mut conv = Conversation::new();
    // 5 × 900 KiB = 4.39 MiB > 4 MiB cap.
    let mut atts = Vec::new();
    for i in 0..5 {
        atts.push(att(
            &format!("f{i}.bin"),
            "application/octet-stream",
            900 * 1024,
        ));
    }
    let err = conv.send_user_message("hi".into(), atts).unwrap_err();
    assert!(matches!(err, SendError::TotalTooLarge));
    // Sanity check the cap matches the public constant.
    assert_eq!(TOTAL_ATTACHMENTS_MAX_BYTES, 4 * 1024 * 1024);
}

#[test]
fn second_send_with_attachments_blocked_on_busy() {
    let mut conv = Conversation::new();
    let _ = conv
        .send_user_message("a".into(), vec![att("x.txt", "text/plain", 10)])
        .unwrap();
    let err = conv.send_user_message("b".into(), vec![]).unwrap_err();
    assert!(matches!(err, SendError::Busy));
}

#[test]
fn retry_preserves_text_and_attachments() {
    let mut conv = Conversation::new();
    let pending = conv
        .send_user_message(
            "解释这段 SQL".into(),
            vec![att("query.hql", "text/plain", 1234)],
        )
        .unwrap();
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
    match &conv.turns()[1].content {
        TurnContent::UserMessage { text, attachments } => {
            assert_eq!(text, "解释这段 SQL");
            assert_eq!(attachments.len(), 1);
            assert_eq!(attachments[0].filename, "query.hql");
            assert_eq!(attachments[0].size_bytes, 1234);
            // New attachment id is allowed but the content should be equivalent.
            match &attachments[0].payload {
                AttachmentPayload::Inline { base64_data_uri } => {
                    assert!(base64_data_uri.starts_with("data:text/plain;base64,"));
                }
            }
        }
        _ => panic!("expected UserMessage on retried turn"),
    }
}
