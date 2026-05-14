//! Addresses analysis finding C3: SC-003 ("100% of attempted sends with
//! HiveClaw unreachable surface a human-readable error") must have
//! automated coverage. Point the client at a closed port; assert that
//! `Conversation::record_failure` produces a retryable failed turn that
//! the UI would attach a `重试` affordance to.

use hivegui::client::{self, sync, OpenResponsesRequest};
use hivegui::model::conversation::{Conversation, TurnError, TurnErrorKind, TurnStatus};
use url::Url;
use uuid::Uuid;

#[tokio::test]
async fn unreachable_hiveclaw_yields_retryable_failed_turn() {
    // Bind to an ephemeral port, then drop the listener so the port is
    // refused on connect.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let url = Url::parse(&format!("http://{addr}")).unwrap();

    let http = client::build_client();
    let err = sync::send(
        &http,
        &url,
        OpenResponsesRequest {
            model: "openclaw:test".to_string(),
            input: "hi".to_string(),
            instructions: None,
            stream: false,
        },
        Uuid::new_v4(),
    )
    .await
    .expect_err("send should fail when no server is listening");

    assert!(matches!(err, client::ClientError::Unreachable(_)));

    // The conversation surface translates that into a retryable failure.
    let mut conv = Conversation::new();
    let pending = conv.send_user_message("hi".to_string()).unwrap();
    conv.record_failure(
        pending,
        TurnError {
            kind: TurnErrorKind::Unreachable,
            message_zh: hivegui::ui::strings_zh::ERR_HIVECLAW_UNREACHABLE.to_string(),
        },
    );
    let turn = &conv.turns()[0];
    match turn.status {
        TurnStatus::Failed { retryable } => assert!(retryable),
        _ => panic!("expected retryable failed turn"),
    }
    assert!(turn.error.is_some());
}
