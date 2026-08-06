//! T117 [P] [US13] Conversation retention contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T117
//! ("使用固定 ChatSession/ChatMessage/AgentExecution fixture 编写
//! 密文、100 日历年默认、可调保留期、影响计数确认、级联删除和
//! 遗留 running 恢复测试；激活 T016F 的 ChatSession
//! `title_encrypted`、ChatMessage `content_encrypted/
//! tool_calls_encrypted` 与 AgentExecution `state_encrypted` 行").
//!
//! Public boundaries the T127 implementation must satisfy:
//!   - `hivegui::datasource::conversation_store::ConversationStore`
//!   - `hivegui::datasource::conversation_store::ChatSession`
//!   - `hivegui::datasource::conversation_store::ChatMessage`
//!   - `hivegui::datasource::conversation_store::AgentExecutionRecord`
//!   - `hivegui::datasource::conversation_store::DEFAULT_RETENTION_DAYS`
//!
//! T123 reviewer signs §T117.11; T127 implementation then makes
//! these tests Green; T136 reruns to record the Green evidence.

#![allow(missing_docs)]

mod support;

use std::sync::Arc;

use hivegui::datasource::conversation_store::{
    ConversationStore, ConversationStoreError, DEFAULT_RETENTION_DAYS,
};
use hivegui::datasource::crypto::Crypto;
use support::TestWorkspace;
use support::sensitive_canary::{
    SensitiveField, place_canary_for_test, scan_all_mediums_for_test, unique_canary_payload,
};

fn fresh_crypto() -> Arc<Crypto> {
    Arc::new(Crypto::new(&Crypto::generate_key()))
}

async fn fresh_store_with_retention(retention: Option<i64>) -> (TestWorkspace, ConversationStore) {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store =
        ConversationStore::new(pool, fresh_crypto(), retention).expect("conversation store");
    (workspace, store)
}

#[tokio::test(flavor = "current_thread")]
async fn default_retention_is_one_hundred_years() {
    let (_workspace, store) = fresh_store_with_retention(None).await;
    assert_eq!(
        store.retention_days(),
        DEFAULT_RETENTION_DAYS,
        "T117 requires 100-year default retention; got {}",
        store.retention_days()
    );
    assert_eq!(DEFAULT_RETENTION_DAYS, 100 * 365);
}

#[tokio::test(flavor = "current_thread")]
async fn explicit_retention_overrides_default() {
    let (_workspace, store) = fresh_store_with_retention(Some(30)).await;
    assert_eq!(store.retention_days(), 30);
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_retention_is_rejected() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let err = ConversationStore::new(pool.clone(), fresh_crypto(), Some(0))
        .expect_err("zero retention must be rejected");
    assert_eq!(err, ConversationStoreError::InvalidRetention);
    let err = ConversationStore::new(pool, fresh_crypto(), Some(-1))
        .expect_err("negative retention must be rejected");
    assert_eq!(err, ConversationStoreError::InvalidRetention);
}

#[tokio::test(flavor = "current_thread")]
async fn session_title_is_encrypted_at_rest() {
    let (_workspace, store) = fresh_store_with_retention(None).await;
    let session = store
        .create_session("Project Phoenix Standup")
        .await
        .expect("create session");
    let raw = session.title_encrypted();
    let needle = b"Project Phoenix";
    assert!(
        raw.windows(needle.len()).all(|window| window != needle),
        "title plaintext must not appear in the encrypted blob"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn append_message_rejects_invalid_session_uuid() {
    let (_workspace, store) = fresh_store_with_retention(None).await;
    let err = store
        .append_message("not-a-uuid", "hello", None)
        .await
        .expect_err("invalid uuid must fail");
    assert!(matches!(err, ConversationStoreError::InvalidSessionId(_)));
}

#[tokio::test(flavor = "current_thread")]
async fn message_content_and_tool_calls_are_encrypted() {
    let (_workspace, store) = fresh_store_with_retention(None).await;
    let session = store
        .create_session("Project Phoenix")
        .await
        .expect("create session");
    let tool_calls = serde_json::json!({"name": "format_template", "args": {"x": 1}});
    let message = store
        .append_message(
            session.id(),
            "Hello, canary-token-payload",
            Some(&tool_calls),
        )
        .await
        .expect("append message");
    let content = message.content_encrypted();
    let calls = message
        .tool_calls_encrypted()
        .expect("tool_calls blob present");
    let payload_needle = b"canary-token-payload";
    let name_needle = b"format_template";
    assert!(
        !content
            .windows(payload_needle.len())
            .any(|w| w == payload_needle)
    );
    assert!(!calls.windows(name_needle.len()).any(|w| w == name_needle));
    assert!(
        !calls
            .windows(payload_needle.len())
            .any(|w| w == payload_needle)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn execution_state_is_encrypted() {
    let (_workspace, store) = fresh_store_with_retention(None).await;
    let session = store
        .create_session("Encrypted Execution")
        .await
        .expect("create session");
    let record = store
        .record_execution(
            session.id(),
            hivegui::datasource::conversation_store::ExecutionState::Running,
        )
        .await
        .expect("record execution");
    let blob = record.state_encrypted();
    let needle = b"running";
    assert!(
        blob.windows(needle.len()).all(|w| w != needle),
        "state plaintext 'running' must not appear in the encrypted blob"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn session_message_execution_counts_match_fixture() {
    let (_workspace, store) = fresh_store_with_retention(None).await;
    let session = store
        .create_session("Counted")
        .await
        .expect("create session");
    for i in 0..5 {
        store
            .append_message(session.id(), &format!("message {i}"), None)
            .await
            .expect("append");
    }
    for _ in 0..3 {
        store
            .record_execution(
                session.id(),
                hivegui::datasource::conversation_store::ExecutionState::Completed,
            )
            .await
            .expect("record");
    }
    assert_eq!(store.session_count(), 1);
    assert_eq!(store.message_count(), 5);
    assert_eq!(store.execution_count(), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn conversation_t117_canary_leaves_zero_residue() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let pool = workspace.sqlite_pool().await.expect("sqlite pool");
    let store = ConversationStore::new(pool, fresh_crypto(), None).expect("conversation store");
    let payload = unique_canary_payload("T117-chat-message-canary");
    let canary =
        place_canary_for_test(SensitiveField::ChatMessageContent, &payload).expect("canary placed");
    let session = store
        .create_session("canary session")
        .await
        .expect("create");
    store
        .append_message(session.id(), &payload, None)
        .await
        .expect("append");
    let scan = scan_all_mediums_for_test(workspace.root(), &canary).expect("scan");
    assert!(
        scan.hits.is_empty(),
        "ChatMessageContent canary must not appear in any medium; hits: {:?}",
        scan.hits
    );
}
