//! Plugin delete race-condition integration tests (T165 / SC-009).
//!
//! The production race defence is symmetric:
//!   1. Plugin soft delete holds `SELECT ... FOR UPDATE` through its reference
//!      count and `deleted_at` update.
//!   2. Custom Function creation holds `SELECT ... FOR SHARE` through its
//!      Function/tagging inserts.
//!
//! The 100-round test races both HTTP operations. Two controlled-interleaving
//! tests additionally hold the same row locks in explicit transactions so each
//! possible winner is exercised deterministically.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow};
use axum::Router;
use axum::http::StatusCode;
use common::{delete_auth, get, post_json_auth, seed_admin};
use serde_json::{Value, json};
use sqlx::{MySqlPool, Row};
use tokio::sync::Barrier;
use tokio::task::JoinHandle;
use tokio::time::timeout;

static NEXT_IDENTIFIER: AtomicU64 = AtomicU64::new(0);

type ApiTask = JoinHandle<anyhow::Result<(StatusCode, Value)>>;

fn unique_identifier(label: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must be after UNIX_EPOCH")
        .as_nanos();
    let sequence = NEXT_IDENTIFIER.fetch_add(1, Ordering::Relaxed);
    let identifier = format!("sc009-{label}-{nanos:x}-{sequence:x}");
    assert!(
        identifier.len() <= 64,
        "test identifier exceeds the VARCHAR(64) contract: {identifier}"
    );
    identifier
}

/// Seed only the Plugin row needed by this race suite.
///
/// Upload/S3 behaviour is tested elsewhere; avoiding 100 object-store writes
/// keeps this test focused on the Function/Plugin transaction protocol.
async fn seed_plugin(pool: &MySqlPool, identifier: &str) -> anyhow::Result<i64> {
    let result = sqlx::query(
        r#"INSERT INTO plugins
           (identifier, name, version, s3_key, sha256, size_bytes)
           VALUES (?, ?, '1.0.0', ?, ?, 8)"#,
    )
    .bind(identifier)
    .bind(format!("SC-009 Plugin {identifier}"))
    .bind(format!("plugins/{identifier}/1.0.0.wasm"))
    .bind("0".repeat(64))
    .execute(pool)
    .await?;

    Ok(result.last_insert_id() as i64)
}

async fn cleanup_plugin(pool: &MySqlPool, plugin_id: i64) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM functions WHERE plugin_id = ?")
        .bind(plugin_id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM plugins WHERE id = ?")
        .bind(plugin_id)
        .execute(pool)
        .await?;
    Ok(())
}

async fn wait_for_process_sql(
    observer: &MySqlPool,
    required_tokens: &[&str],
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(5), async {
        loop {
            let rows = sqlx::query("SHOW FULL PROCESSLIST")
                .fetch_all(observer)
                .await?;

            let found = rows.iter().any(|row| {
                let command = row.try_get::<String, _>("Command").unwrap_or_default();
                let info = row
                    .try_get::<Option<String>, _>("Info")
                    .ok()
                    .flatten()
                    .unwrap_or_default()
                    .to_ascii_lowercase();

                command == "Query"
                    && required_tokens
                        .iter()
                        .all(|token| info.contains(&token.to_ascii_lowercase()))
            });

            if found {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| anyhow!("target locking SQL never became visible in PROCESSLIST"))??;

    Ok(())
}

async fn abort_and_await(handle: &mut ApiTask) {
    handle.abort();
    let _ = handle.await;
}

async fn await_api_task(
    handle: &mut ApiTask,
    operation: &str,
) -> anyhow::Result<(StatusCode, Value)> {
    match timeout(Duration::from_secs(10), &mut *handle).await {
        Ok(result) => result.context("API task panicked or was cancelled")?,
        Err(_) => {
            abort_and_await(handle).await;
            Err(anyhow!("{operation} timed out after 10 seconds"))
        }
    }
}

async fn await_race_tasks(
    delete_handle: &mut ApiTask,
    create_handle: &mut ApiTask,
    round: usize,
) -> anyhow::Result<((StatusCode, Value), (StatusCode, Value))> {
    match timeout(Duration::from_secs(10), async {
        let (delete_result, create_result) = tokio::join!(&mut *delete_handle, &mut *create_handle);
        let delete_result = delete_result.context("delete task panicked or was cancelled")??;
        let create_result = create_result.context("create task panicked or was cancelled")??;
        Ok::<_, anyhow::Error>((delete_result, create_result))
    })
    .await
    {
        Ok(result) => result,
        Err(_) => {
            abort_and_await(delete_handle).await;
            abort_and_await(create_handle).await;
            Err(anyhow!(
                "round {round}: delete/create race timed out after 10 seconds"
            ))
        }
    }
}

async fn try_create_function(
    app: &Router,
    token: &str,
    plugin_id: i64,
    identifier: &str,
) -> anyhow::Result<(StatusCode, Value)> {
    post_json_auth(
        app,
        "/api/functions",
        token,
        json!({
            "identifier": identifier,
            "name": format!("SC-009 Function {identifier}"),
            "description": "Function/Plugin race test",
            "plugin_id": plugin_id,
            "plugin_export": "test_export",
            "input_schema": {"type": "object", "properties": {}},
            "output_schema": {"type": "object", "properties": {}}
        }),
    )
    .await
}

async fn create_function(
    app: &Router,
    token: &str,
    plugin_id: i64,
    identifier: &str,
) -> anyhow::Result<i64> {
    let (status, body) = try_create_function(app, token, plugin_id, identifier).await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "create function failed: {status} {body}"
    );
    Ok(body["data"]["id"].as_i64().expect("missing function id"))
}

async fn delete_plugin(
    app: &Router,
    token: &str,
    plugin_id: i64,
) -> anyhow::Result<(StatusCode, Value)> {
    delete_auth(app, &format!("/api/plugins/{plugin_id}"), token).await
}

async fn assert_plugin_invariant(
    app: &Router,
    pool: &MySqlPool,
    token: &str,
    plugin_id: i64,
    plugin_deleted: bool,
    expected_function_count: i64,
) -> anyhow::Result<()> {
    let (status, body) = get(app, &format!("/api/plugins/{plugin_id}"), Some(token)).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["data"]["deleted_at"].as_str().is_some(),
        plugin_deleted,
        "plugin state does not match the winning operation: {body}"
    );

    let (function_count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM functions WHERE plugin_id = ?")
            .bind(plugin_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(
        function_count, expected_function_count,
        "unexpected Function reference count for plugin_id={plugin_id}"
    );
    assert!(
        !plugin_deleted || function_count == 0,
        "a soft-deleted Plugin must never retain a Function reference"
    );
    Ok(())
}

#[tokio::test]
async fn t165_delete_blocked_when_function_exists() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = seed_admin(&pool, 3, 1, "test123").await?;
    let token = admin.token()?;
    let plugin_identifier = unique_identifier("referenced-plugin");
    let function_identifier = unique_identifier("referencing-function");
    let plugin_id = seed_plugin(&pool, &plugin_identifier).await?;

    create_function(&app, &token, plugin_id, &function_identifier).await?;

    let (status, body) = delete_plugin(&app, &token, plugin_id).await?;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "delete must be blocked when a Function references the Plugin: {body}"
    );
    assert_eq!(
        body["code"].as_i64(),
        Some(4093),
        "expected 4093 ResourceInUse: {body}"
    );
    assert_plugin_invariant(&app, &pool, &token, plugin_id, false, 1).await?;

    cleanup_plugin(&pool, plugin_id).await
}

#[tokio::test]
async fn t165_concurrent_delete_and_create_no_race_100_rounds() -> anyhow::Result<()> {
    const ROUNDS: usize = 100;

    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = seed_admin(&pool, 3, 1, "test123").await?;
    let token = admin.token()?;
    let mut delete_wins = 0;
    let mut create_wins = 0;

    for round in 0..ROUNDS {
        let plugin_identifier = unique_identifier("race-plugin");
        let function_identifier = unique_identifier("race-function");
        let plugin_id = seed_plugin(&pool, &plugin_identifier).await?;
        let barrier = Arc::new(Barrier::new(2));

        let mut delete_handle = tokio::spawn({
            let app = app.clone();
            let token = token.clone();
            let barrier = Arc::clone(&barrier);
            async move {
                barrier.wait().await;
                delete_plugin(&app, &token, plugin_id).await
            }
        });
        let mut create_handle = tokio::spawn({
            let app = app.clone();
            let token = token.clone();
            let barrier = Arc::clone(&barrier);
            async move {
                barrier.wait().await;
                try_create_function(&app, &token, plugin_id, &function_identifier).await
            }
        });

        let ((delete_status, delete_body), (create_status, create_body)) =
            match await_race_tasks(&mut delete_handle, &mut create_handle, round).await {
                Ok(result) => result,
                Err(error) => {
                    cleanup_plugin(&pool, plugin_id)
                        .await
                        .context("cleanup after timed-out race failed")?;
                    return Err(error);
                }
            };

        let plugin_deleted = delete_status == StatusCode::OK;
        let function_created = create_status == StatusCode::OK;
        assert!(
            plugin_deleted ^ function_created,
            "round {round}: exactly one operation must succeed; \
             delete={delete_status} {delete_body}, create={create_status} {create_body}"
        );

        if plugin_deleted {
            delete_wins += 1;
            assert_eq!(
                create_status,
                StatusCode::CONFLICT,
                "round {round}: create must fail when delete wins: {create_body}"
            );
            assert_eq!(
                create_body["code"].as_i64(),
                Some(4093),
                "round {round}: create conflict must be 4093: {create_body}"
            );
            assert_plugin_invariant(&app, &pool, &token, plugin_id, true, 0).await?;
        } else {
            create_wins += 1;
            assert_eq!(
                delete_status,
                StatusCode::CONFLICT,
                "round {round}: delete must fail when create wins: {delete_body}"
            );
            assert_eq!(
                delete_body["code"].as_i64(),
                Some(4093),
                "round {round}: delete conflict must be 4093: {delete_body}"
            );
            assert_plugin_invariant(&app, &pool, &token, plugin_id, false, 1).await?;
        }

        cleanup_plugin(&pool, plugin_id).await?;
    }

    assert_eq!(delete_wins + create_wins, ROUNDS);
    tracing::info!(delete_wins, create_wins, "T165 completed 100 race rounds");
    Ok(())
}

#[tokio::test]
async fn t165_controlled_create_first_interleaving() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = seed_admin(&pool, 3, 1, "test123").await?;
    let token = admin.token()?;
    let plugin_identifier = unique_identifier("create-first-plugin");
    let function_identifier = unique_identifier("create-first-function");
    let plugin_id = seed_plugin(&pool, &plugin_identifier).await?;

    // Hold the exact shared Plugin row lock and Function INSERT in one
    // uncommitted transaction. The production delete must wait for this lock.
    let mut create_tx = pool.begin().await?;
    sqlx::query("SELECT id FROM plugins WHERE id = ? FOR SHARE")
        .bind(plugin_id)
        .fetch_one(&mut *create_tx)
        .await?;
    sqlx::query(
        r#"INSERT INTO functions
           (identifier, name, kind, input_schema, output_schema, plugin_id, plugin_export)
           VALUES (?, ?, 2, JSON_OBJECT('type', 'object'), JSON_OBJECT('type', 'object'), ?, 'test_export')"#,
    )
    .bind(&function_identifier)
    .bind(format!("SC-009 Function {function_identifier}"))
    .bind(plugin_id)
    .execute(&mut *create_tx)
    .await?;

    let mut delete_handle = tokio::spawn({
        let app = app.clone();
        let token = token.clone();
        async move { delete_plugin(&app, &token, plugin_id).await }
    });

    if let Err(error) =
        wait_for_process_sql(&pool, &["select id, deleted_at from plugins", "for update"]).await
    {
        abort_and_await(&mut delete_handle).await;
        create_tx.rollback().await?;
        cleanup_plugin(&pool, plugin_id).await?;
        return Err(error.context("production delete never reached its locking query"));
    }
    if delete_handle.is_finished() {
        let completed = delete_handle.await;
        create_tx.rollback().await?;
        cleanup_plugin(&pool, plugin_id).await?;
        return Err(anyhow!(
            "delete completed before the create transaction released FOR SHARE: {completed:?}"
        ));
    }

    if let Err(error) = create_tx.commit().await {
        abort_and_await(&mut delete_handle).await;
        cleanup_plugin(&pool, plugin_id).await?;
        return Err(error).context("create-first holder commit failed");
    }
    let (delete_status, delete_body) =
        match await_api_task(&mut delete_handle, "controlled create-first delete").await {
            Ok(result) => result,
            Err(error) => {
                cleanup_plugin(&pool, plugin_id).await?;
                return Err(error);
            }
        };
    assert_eq!(delete_status, StatusCode::CONFLICT, "{delete_body}");
    assert_eq!(delete_body["code"].as_i64(), Some(4093), "{delete_body}");
    assert_plugin_invariant(&app, &pool, &token, plugin_id, false, 1).await?;

    cleanup_plugin(&pool, plugin_id).await
}

#[tokio::test]
async fn t165_controlled_delete_first_interleaving() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = seed_admin(&pool, 3, 1, "test123").await?;
    let token = admin.token()?;
    let plugin_identifier = unique_identifier("delete-first-plugin");
    let function_identifier = unique_identifier("delete-first-function");
    let plugin_id = seed_plugin(&pool, &plugin_identifier).await?;

    // Hold the exact exclusive Plugin row lock and soft-delete update in one
    // uncommitted transaction. Production Function creation must wait for it.
    let mut delete_tx = pool.begin().await?;
    sqlx::query("SELECT id FROM plugins WHERE id = ? FOR UPDATE")
        .bind(plugin_id)
        .fetch_one(&mut *delete_tx)
        .await?;
    sqlx::query("UPDATE plugins SET deleted_at = NOW() WHERE id = ?")
        .bind(plugin_id)
        .execute(&mut *delete_tx)
        .await?;

    let mut create_handle = tokio::spawn({
        let app = app.clone();
        let token = token.clone();
        async move { try_create_function(&app, &token, plugin_id, &function_identifier).await }
    });

    if let Err(error) =
        wait_for_process_sql(&pool, &["select id, deleted_at from plugins", "for share"]).await
    {
        abort_and_await(&mut create_handle).await;
        delete_tx.rollback().await?;
        cleanup_plugin(&pool, plugin_id).await?;
        return Err(error.context("production create never reached its locking query"));
    }
    if create_handle.is_finished() {
        let completed = create_handle.await;
        delete_tx.rollback().await?;
        cleanup_plugin(&pool, plugin_id).await?;
        return Err(anyhow!(
            "create completed before the delete transaction released FOR UPDATE: {completed:?}"
        ));
    }

    if let Err(error) = delete_tx.commit().await {
        abort_and_await(&mut create_handle).await;
        cleanup_plugin(&pool, plugin_id).await?;
        return Err(error).context("delete-first holder commit failed");
    }
    let (create_status, create_body) =
        match await_api_task(&mut create_handle, "controlled delete-first create").await {
            Ok(result) => result,
            Err(error) => {
                cleanup_plugin(&pool, plugin_id).await?;
                return Err(error);
            }
        };
    assert_eq!(create_status, StatusCode::CONFLICT, "{create_body}");
    assert_eq!(create_body["code"].as_i64(), Some(4093), "{create_body}");
    assert_plugin_invariant(&app, &pool, &token, plugin_id, true, 0).await?;

    cleanup_plugin(&pool, plugin_id).await
}

#[tokio::test]
async fn t165_create_blocked_when_plugin_is_deleted() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = seed_admin(&pool, 3, 1, "test123").await?;
    let token = admin.token()?;
    let plugin_identifier = unique_identifier("deleted-plugin");
    let function_identifier = unique_identifier("deleted-plugin-function");
    let plugin_id = seed_plugin(&pool, &plugin_identifier).await?;

    let (delete_status, delete_body) = delete_plugin(&app, &token, plugin_id).await?;
    assert_eq!(delete_status, StatusCode::OK, "{delete_body}");

    let (create_status, create_body) =
        try_create_function(&app, &token, plugin_id, &function_identifier).await?;
    assert_eq!(create_status, StatusCode::CONFLICT, "{create_body}");
    assert_eq!(create_body["code"].as_i64(), Some(4093), "{create_body}");
    assert_plugin_invariant(&app, &pool, &token, plugin_id, true, 0).await?;

    cleanup_plugin(&pool, plugin_id).await
}

#[tokio::test]
async fn t165_delete_succeeds_when_no_functions() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = seed_admin(&pool, 3, 1, "test123").await?;
    let token = admin.token()?;
    let plugin_identifier = unique_identifier("unreferenced-plugin");
    let plugin_id = seed_plugin(&pool, &plugin_identifier).await?;

    let (status, body) = delete_plugin(&app, &token, plugin_id).await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "delete must succeed without Function references: {body}"
    );
    assert_plugin_invariant(&app, &pool, &token, plugin_id, true, 0).await?;

    cleanup_plugin(&pool, plugin_id).await
}
