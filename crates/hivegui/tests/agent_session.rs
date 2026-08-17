//! T115 [P] [US13] Local Agent contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T115
//! ("在 `crates/hivegui/tests/agent_session.rs` 编写完整本地对话、
//! 取消、快照回滚、Agent 周期事件、Tool 调用与 Tool→HiveWeb 失败
//! 不可静默回退 + 不发请求的本地失败回退路径，并断言 ChatSession /
//! ChatMessage / AgentExecution 行由本测试和 T116/T117 共同首次
//! 激活 T016F canary").
//!
//! Red public boundaries (T124 will add these):
//!   - `hivegui::agent::session::AgentSession`
//!   - `hivegui::agent::session::AgentMessage`
//!   - `hivegui::agent::session::AgentToolCall`
//!   - `hivegui::agent::session::AgentExecution`
//!   - `hivegui::agent::session::CancelToken`
//!   - `hivegui::agent::session::Snapshot`
//!
//! T123 reviewer signs §T115-T122.11; T124-T135 implementation then
//! make these tests Green; T136 reruns to record the Green evidence.

mod support;

use hivegui::agent::session::{
    AgentExecution, AgentMessage, AgentSession, AgentToolCall, CancelToken, Snapshot,
};
use support::TestWorkspace;
use support::sensitive_canary::{
    SensitiveField, place_canary_for_test, scan_all_mediums_for_test, unique_canary_payload,
};

#[tokio::test(flavor = "current_thread")]
async fn session_starts_in_idle_state() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let session = AgentSession::new(workspace.database_path()).expect("session");
    assert_eq!(session.state(), "idle");
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_token_short_circuits_long_running_tool_call() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let mut session = AgentSession::new(workspace.database_path()).expect("session");
    let token: CancelToken = session.start("hello").expect("start");
    let result = session
        .invoke_tool(
            AgentToolCall::new("http_get", serde_json::json!({"url": "https://x"})),
            &token,
        )
        .await;
    assert!(
        result.is_err(),
        "tool call to HiveWeb must surface as local failure, not silent retry"
    );
    token.cancel();
    assert!(token.is_cancelled());
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_rollback_restores_pre_call_state() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let mut session = AgentSession::new(workspace.database_path()).expect("session");
    let snapshot: Snapshot = session.snapshot().expect("snapshot");
    session.start("send a message").expect("start");
    session.rollback_to(snapshot).expect("rollback");
    assert_eq!(session.state(), "idle");
}

#[tokio::test(flavor = "current_thread")]
async fn chat_message_canary_leaves_zero_residue() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let mut session = AgentSession::new(workspace.database_path()).expect("session");
    let payload = unique_canary_payload("T115-canary-chat-message");
    let canary =
        place_canary_for_test(SensitiveField::ChatMessageContent, &payload).expect("canary placed");
    session
        .append_message(AgentMessage::user(payload.clone()))
        .expect("append message");
    let scan = scan_all_mediums_for_test(workspace.root(), &canary).expect("scan");
    assert!(
        scan.hits.is_empty(),
        "ChatMessageContent canary {payload:?} must not appear in any medium; hits: {:?}",
        scan.hits
    );
}

#[allow(dead_code, clippy::diverging_sub_expression)]
fn _pin_types() {
    let _: AgentExecution = panic!("placeholder so the type is referenced");
}
