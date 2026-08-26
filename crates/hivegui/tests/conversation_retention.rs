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

use std::{collections::BTreeSet, path::Path, sync::Arc};

use chrono::{Datelike, Utc};
use hivegui::datasource::conversation_store::{
    ConversationStore, ConversationStoreError, DEFAULT_RETENTION_DAYS, ExecutionState,
};
use hivegui::datasource::crypto::Crypto;
use hivegui::datasource::{
    entity_store::{AgentInput, AgentStore},
    query_count::{QueryCountObserver, evaluate_query_count, production_query_count_catalog},
    query_plan::{QueryDialect, production_query_catalog},
    store::{Store, StoreOpenOptions},
};
use support::sensitive_canary::{
    SensitiveField, place_canary_for_test, scan_all_mediums_for_test, unique_canary_payload,
};
use support::{
    TestWorkspace,
    performance::{
        CONVERSATION_BUNDLE_ID, CONVERSATION_CLEANUP_ID, CONVERSATION_FIXTURE_ROWS,
        CONVERSATION_LIST_ID, CONVERSATION_RECOVERY_ID, ComparisonOutcome, EnvironmentFingerprint,
        MEASURED_SAMPLES, baseline_path, evaluate_benchmark_gate, regression_exception_path,
        run_target, source_revision, target_specs,
    },
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

async fn open_canonical_conversation_store(
    workspace: &TestWorkspace,
    observer: Option<QueryCountObserver>,
) -> (Store, ConversationStore, i64) {
    let mut options = StoreOpenOptions::new(workspace.database_path(), workspace.plugin_root());
    if let Some(observer) = observer {
        options = options.with_query_count_observer(observer);
    }
    let database = Store::open_local(options)
        .await
        .expect("open canonical v4 Store");
    let agent = AgentStore::new(database.pool().clone())
        .expect("Agent Store")
        .create(
            AgentInput::new_root("conversation-root", "Conversation Root", "system prompt")
                .expect("valid Agent"),
        )
        .await
        .expect("create root Agent");
    let conversations =
        ConversationStore::from_store(&database, None).expect("canonical Conversation Store");
    (database, conversations, agent.id())
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
async fn canonical_session_uses_calendar_expiry_and_delete_cascades_every_child() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (database, store, agent_id) = open_canonical_conversation_store(&workspace, None).await;
    let session = store
        .create_session_for_agent(agent_id, "Calendar retention")
        .await
        .expect("create canonical session");
    let created =
        chrono::DateTime::parse_from_rfc3339(session.created_at()).expect("created_at RFC3339");
    let expires =
        chrono::DateTime::parse_from_rfc3339(session.expires_at()).expect("expires_at RFC3339");
    let expected = created
        .with_year(created.year() + 100)
        .expect("calendar-year retention");
    assert_eq!(expires, expected, "default retention is 100 calendar years");
    assert_eq!(session.entry_agent_id(), agent_id);
    assert_eq!(session.current_agent_id(), Some(agent_id));

    store
        .append_message(session.id(), "cascade message", None)
        .await
        .expect("append persisted message");
    store
        .record_execution(
            session.id(),
            hivegui::datasource::conversation_store::ExecutionState::Completed,
        )
        .await
        .expect("append persisted execution");
    store
        .delete_session(session.id())
        .await
        .expect("delete session cascade");
    let footprint: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM chat_sessions), \
                (SELECT COUNT(*) FROM chat_messages), \
                (SELECT COUNT(*) FROM agent_executions)",
    )
    .fetch_one(database.pool())
    .await
    .expect("cascade footprint");
    assert_eq!(footprint, (0, 0, 0));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM agents WHERE id = ?")
            .bind(agent_id)
            .fetch_one(database.pool())
            .await
            .expect("Agent remains"),
        1,
        "history deletion must never delete Agent configuration"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn retention_preview_is_confirmation_bound_and_stale_preview_changes_nothing() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (database, store, agent_id) = open_canonical_conversation_store(&workspace, None).await;
    let first = store
        .create_session_for_agent(agent_id, "Expired one")
        .await
        .expect("first expired session");
    sqlx::query("UPDATE chat_sessions SET expires_at = '2020-01-01T00:00:00Z' WHERE id = ?")
        .bind(first.id())
        .execute(database.pool())
        .await
        .expect("expire first session");
    let preview = store
        .preview_retention("expired")
        .await
        .expect("preview one expired session");
    assert_eq!(preview.affected_count(), 1);

    let second = store
        .create_session_for_agent(agent_id, "Expired after preview")
        .await
        .expect("second expired session");
    sqlx::query("UPDATE chat_sessions SET expires_at = '2020-01-01T00:00:00Z' WHERE id = ?")
        .bind(second.id())
        .execute(database.pool())
        .await
        .expect("expire second session");
    let stale = store
        .apply_retention(&preview)
        .await
        .expect_err("preview/confirmation race must fail closed");
    assert_eq!(stale.field(), "retention_filter");
    assert_eq!(stale.reason(), "stale_preview");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM chat_sessions")
            .fetch_one(database.pool())
            .await
            .expect("sessions after stale preview"),
        2,
        "stale confirmation must delete nothing"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn legacy_running_executions_recover_once_to_failed_interrupted() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (database, store, agent_id) = open_canonical_conversation_store(&workspace, None).await;
    let session = store
        .create_session_for_agent(agent_id, "Interrupted execution")
        .await
        .expect("create session");
    let execution = store
        .record_execution(
            session.id(),
            hivegui::datasource::conversation_store::ExecutionState::Running,
        )
        .await
        .expect("create running execution");
    assert_eq!(
        store.recover_interrupted_running().await.expect("recover"),
        1
    );
    assert_eq!(
        store
            .recover_interrupted_running()
            .await
            .expect("idempotent"),
        0
    );
    let persisted: (String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT status, finished_at, error_kind FROM agent_executions WHERE execution_id = ?",
    )
    .bind(execution.id())
    .fetch_one(database.pool())
    .await
    .expect("recovered execution row");
    assert_eq!(persisted.0, "failed");
    assert!(persisted.1.is_some());
    assert_eq!(persisted.2.as_deref(), Some("interrupted"));
}

#[tokio::test(flavor = "current_thread")]
async fn session_bundle_load_is_two_queries_at_one_or_twenty_five_sessions() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let observer = QueryCountObserver::new();
    let (_database, store, agent_id) =
        open_canonical_conversation_store(&workspace, Some(observer.clone())).await;
    let mut ids = Vec::new();
    for index in 0..25 {
        let session = store
            .create_session_for_agent(agent_id, &format!("Bundle {index:03}"))
            .await
            .expect("seed session");
        store
            .append_message(session.id(), &format!("message {index:03}"), None)
            .await
            .expect("seed message");
        store
            .record_execution(
                session.id(),
                hivegui::datasource::conversation_store::ExecutionState::Completed,
            )
            .await
            .expect("seed execution");
        ids.push(session.id().to_string());
    }
    let contract = production_query_count_catalog()
        .iter()
        .find(|contract| contract.id == "conversation.session_bundle")
        .expect("conversation query-count contract");
    assert_eq!(contract.owner_phase, "US13");
    assert_eq!(contract.activation_task, "T117");
    assert!(contract.active);
    assert_eq!(contract.maximum_queries, 2);

    let small_scope = observer.start_scope(contract.id, 1);
    let small = store
        .load_session_bundles(&ids[..1])
        .await
        .expect("load one bundle");
    let small_sample = small_scope.finish();
    let large_scope = observer.start_scope(contract.id, ids.len());
    let large = store
        .load_session_bundles(&ids)
        .await
        .expect("load many bundles");
    let large_sample = large_scope.finish();
    assert_eq!(small.len(), 1);
    assert_eq!(large.len(), 25);
    assert!(
        large
            .iter()
            .all(|bundle| { bundle.messages().len() == 1 && bundle.executions().len() == 1 })
    );
    let verdict = evaluate_query_count(contract.maximum_queries, &small_sample, &large_sample);
    assert!(
        verdict.is_accepted(),
        "session bundle must remain two queries: {:?}",
        verdict.failures()
    );
}

#[test]
fn conversation_query_catalog_owns_all_list_bundle_retention_and_recovery_routes() {
    let activated = production_query_catalog()
        .iter()
        .filter(|query| {
            query.active
                && query.owner_phase == "US13"
                && query.activation_task == "T117"
                && query.dialect == QueryDialect::Sqlite
        })
        .collect::<Vec<_>>();
    let observed = activated
        .iter()
        .flat_map(|query| {
            query
                .requirements
                .iter()
                .map(|requirement| requirement.table)
        })
        .collect::<BTreeSet<_>>();
    let required = BTreeSet::from(["chat_sessions", "chat_messages", "agent_executions"]);
    assert!(
        !activated.is_empty()
            && activated
                .iter()
                .all(|query| { !query.sql.trim().is_empty() && !query.requirements.is_empty() })
            && required.is_subset(&observed),
        "active US13/T117 catalog is incomplete: required={required:?}, observed={observed:?}"
    );
}

#[test]
fn conversation_performance_uses_four_canonical_t005_targets() {
    let owned = target_specs()
        .into_iter()
        .filter(|target| target.owner_task == "T117")
        .collect::<Vec<_>>();
    let ids = owned
        .iter()
        .map(|target| target.id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        ids,
        BTreeSet::from([
            CONVERSATION_LIST_ID,
            CONVERSATION_BUNDLE_ID,
            CONVERSATION_CLEANUP_ID,
            CONVERSATION_RECOVERY_ID,
        ])
    );
    assert_eq!(owned.len(), 4);
    const { assert!(CONVERSATION_FIXTURE_ROWS >= 100) };
    assert!(owned.iter().all(|target| {
        target.warmup_iterations == 10
            && target.measured_samples == MEASURED_SAMPLES
            && target.p95_budget_ns <= 1_000_000_000
    }));
}

#[cfg_attr(
    debug_assertions,
    ignore = "release-only T005 benchmark gate; debug has a distinct environment fingerprint"
)]
#[tokio::test(flavor = "current_thread")]
async fn conversation_performance_runner_meets_budgets_and_approved_baselines() {
    let current_source_revision = source_revision(Path::new(env!("CARGO_MANIFEST_DIR")))
        .expect("capture repository-anchored benchmark source revision");
    let as_of = Utc::now().date_naive();
    let environment = EnvironmentFingerprint::capture();
    for target in target_specs()
        .into_iter()
        .filter(|target| target.owner_task == "T117")
    {
        let report = run_target(&target, &environment)
            .await
            .unwrap_or_else(|error| panic!("run canonical {} benchmark: {error}", target.id))
            .unwrap_or_else(|| panic!("T005 runner must own {}", target.id));
        assert_eq!(report.sample_count, MEASURED_SAMPLES);
        let baseline = baseline_path(&target, &environment);
        let exception = regression_exception_path(&target, &environment);
        let evaluation = evaluate_benchmark_gate(
            &report,
            &current_source_revision,
            &baseline,
            &exception,
            as_of,
        )
        .unwrap_or_else(|error| {
            panic!(
                "{} gate error: baseline={}, exception={}, error={error}",
                target.id,
                baseline.display(),
                exception.display()
            )
        });
        assert!(
            matches!(
                evaluation.outcome,
                ComparisonOutcome::Passed | ComparisonOutcome::ApprovedException
            ),
            "{} requires approved T005 evidence: baseline={}, exception={}, evaluation={evaluation:?}",
            target.id,
            baseline.display(),
            exception.display()
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn conversation_t117_canary_leaves_zero_residue() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let (database, store, agent_id) = open_canonical_conversation_store(&workspace, None).await;
    let payload = unique_canary_payload("T117-chat-message-canary");
    let canary =
        place_canary_for_test(SensitiveField::ChatMessageContent, &payload).expect("canary placed");
    let token = canary.plaintext_token();
    let session = store
        .create_session_for_agent(agent_id, "canary session")
        .await
        .expect("create");
    store
        .append_message(session.id(), &token, None)
        .await
        .expect("append");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM chat_messages WHERE session_id = ?")
            .bind(session.id())
            .fetch_one(database.pool())
            .await
            .expect("persisted canary message"),
        1,
        "a zero-hit scan is evidence only after the encrypted row is persisted"
    );
    let scan = scan_all_mediums_for_test(workspace.root(), &canary).expect("scan");
    assert!(
        scan.hits.is_empty(),
        "ChatMessageContent canary must not appear in any medium; hits: {:?}",
        scan.hits
    );
}

#[tokio::test(flavor = "current_thread")]
async fn every_conversation_field_canary_survives_error_and_interrupted_recovery_without_plaintext()
{
    let workspace = TestWorkspace::new().expect("test workspace");
    let (database, store, agent_id) = open_canonical_conversation_store(&workspace, None).await;
    let title = place_canary_for_test(
        SensitiveField::ChatSessionTitle,
        &unique_canary_payload("T117-session-title"),
    )
    .expect("title canary");
    let content = place_canary_for_test(
        SensitiveField::ChatMessageContent,
        &unique_canary_payload("T117-message-content"),
    )
    .expect("content canary");
    let tool_calls = place_canary_for_test(
        SensitiveField::ChatMessageToolCalls,
        &unique_canary_payload("T117-tool-calls"),
    )
    .expect("tool-call canary");
    let execution = place_canary_for_test(
        SensitiveField::AgentExecutionState,
        &unique_canary_payload("T117-execution-state"),
    )
    .expect("execution canary");

    let session = store
        .create_session_for_agent(agent_id, &title.plaintext_token())
        .await
        .expect("persist encrypted title");
    let tool_payload = serde_json::json!({
        "name": "format_template",
        "arguments": {"canary": tool_calls.plaintext_token()},
        "result": {"canary": tool_calls.plaintext_token()},
    });
    store
        .append_message(
            session.id(),
            &content.plaintext_token(),
            Some(&tool_payload),
        )
        .await
        .expect("persist encrypted message and Tool payload");

    let invalid = store
        .append_message(
            "not-a-session-uuid",
            &content.plaintext_token(),
            Some(&tool_payload),
        )
        .await
        .expect_err("error path must reject before persistence");
    let safe_error = invalid.to_string();
    assert!(!safe_error.contains(&content.plaintext_token()));
    assert!(!safe_error.contains(&tool_calls.plaintext_token()));

    let state = serde_json::json!({
        "schema_version": 1,
        "route": [agent_id],
        "completed": [],
        "interrupted": [execution.plaintext_token()],
        "not_started": [],
        "checkpoint": null,
        "side_effect_notice": "completed external side effects are not rolled back",
    });
    let record = store
        .record_execution_state(session.id(), agent_id, ExecutionState::Running, &state)
        .await
        .expect("persist encrypted running state");
    assert_eq!(
        store
            .recover_interrupted_running()
            .await
            .expect("recover interrupted execution"),
        1
    );
    let recovered: (String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT status, finished_at, error_kind FROM agent_executions WHERE execution_id = ?",
    )
    .bind(record.id())
    .fetch_one(database.pool())
    .await
    .expect("recovered encrypted execution");
    assert_eq!(recovered.0, "failed");
    assert!(recovered.1.is_some());
    assert_eq!(recovered.2.as_deref(), Some("interrupted"));

    for canary in [&title, &content, &tool_calls, &execution] {
        let scan = scan_all_mediums_for_test(workspace.root(), canary).expect("scan every medium");
        assert!(
            scan.hits.is_empty(),
            "{} canary leaked through normal/error/recovery paths: {:?}",
            canary.field.canonical_slot(),
            scan.hits
        );
    }
}
