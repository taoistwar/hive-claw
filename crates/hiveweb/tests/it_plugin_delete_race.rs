//! Plugin delete race condition integration test (T165 / SC-009)
//!
//! Verifies the two row-locking paths that close the race window:
//!   1. Soft delete uses `SELECT ... FOR UPDATE` before checking Function references
//!   2. Function creation uses `SELECT ... FOR SHARE` and inserts in the same transaction
//!
//! Scenario: Thread A tries to soft-delete an active, unreferenced Plugin while
//! Thread B concurrently creates a new Function referencing that Plugin. Exactly
//! one operation may succeed, with no Function referencing a soft-deleted Plugin.

mod common;

use std::{sync::Arc, time::Duration};
use tokio::sync::Barrier;

use axum::{Router, http::StatusCode};
use common::{delete_auth, get, post_json_auth, seed_admin};
use serde_json::{Value, json};

const T165_RACE_SCENARIOS: usize = 100;

/// Helper: upload a minimal valid WASM plugin and return plugin id.
async fn upload_plugin(app: &Router, token: &str, identifier: &str) -> anyhow::Result<i64> {
    // Create a minimal WASM binary (valid magic bytes + empty module)
    let wasm_bytes: Vec<u8> = vec![
        0x00, 0x61, 0x73, 0x6d, // magic \0asm
        0x01, 0x00, 0x00, 0x00, // version 1
    ];

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let boundary = "----WebKitFormBoundaryTest123";
    let mut body = Vec::new();

    // meta field first
    body.extend_from_slice(b"------WebKitFormBoundaryTest123\r\n");
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"meta\"\r\n\r\n");
    let meta = json!({
        "identifier": identifier,
        "version": "1.0.0",
        "name": format!("Test Plugin {identifier}"),
        "description": "Test plugin for race condition",
        "author": "test"
    });
    body.extend_from_slice(meta.to_string().as_bytes());
    body.extend_from_slice(b"\r\n");

    // file field
    body.extend_from_slice(b"------WebKitFormBoundaryTest123\r\n");
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"test.wasm\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: application/wasm\r\n\r\n");
    body.extend_from_slice(&wasm_bytes);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(b"------WebKitFormBoundaryTest123--\r\n");

    let req = Request::builder()
        .method("POST")
        .uri("/api/plugins")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={}", boundary),
        )
        .header("authorization", format!("Bearer {}", token))
        .body(Body::from(body))?;

    let resp = app.clone().oneshot(req).await?;
    let status = resp.status();
    let bytes = resp.into_body().collect().await?.to_bytes();
    let resp_body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(
        status,
        StatusCode::OK,
        "upload plugin failed: {status} {resp_body}"
    );
    Ok(resp_body["data"]["id"].as_i64().expect("missing plugin id"))
}

/// Seed only the Plugin row needed by the 100-round service race. Repeating the
/// upload API would test the unrelated per-IP rate limiter instead of SC-009.
async fn insert_plugin_fixture(pool: &sqlx::MySqlPool, identifier: &str) -> anyhow::Result<i64> {
    let s3_key = format!("plugins/{identifier}/1.0.0.wasm");
    let sha256 = "0".repeat(64);
    let result = sqlx::query(
        r#"INSERT INTO plugins
           (identifier, name, version, s3_key, sha256, size_bytes)
           VALUES (?, ?, '1.0.0', ?, ?, 8)"#,
    )
    .bind(identifier)
    .bind(identifier)
    .bind(s3_key)
    .bind(sha256)
    .execute(pool)
    .await?;
    Ok(result.last_insert_id() as i64)
}

/// Helper: create a function referencing a plugin without assuming which
/// concurrent operation wins.
async fn create_function_response(
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
            "name": format!("Test Function {identifier}"),
            "description": "Test function",
            "plugin_id": plugin_id,
            "plugin_export": "test_export",
            "input_schema": {"type": "object", "properties": {}},
            "output_schema": {"type": "object", "properties": {}}
        }),
    )
    .await
}

/// Helper: create a function when success is the only valid outcome.
async fn create_function(
    app: &Router,
    token: &str,
    plugin_id: i64,
    identifier: &str,
) -> anyhow::Result<i64> {
    let (status, body) = create_function_response(app, token, plugin_id, identifier).await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "create function failed: {status} {body}"
    );
    Ok(body["data"]["id"].as_i64().expect("missing function id"))
}

/// Helper: soft-delete a plugin
async fn delete_plugin(
    app: &Router,
    token: &str,
    plugin_id: i64,
) -> anyhow::Result<(axum::http::StatusCode, Value)> {
    delete_auth(app, &format!("/api/plugins/{plugin_id}"), token).await
}

async fn deleted_plugin_reference_count(
    pool: &sqlx::MySqlPool,
    plugin_id: i64,
) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        r#"SELECT COUNT(*)
           FROM functions f
           JOIN plugins p ON p.id = f.plugin_id
           WHERE f.plugin_id = ?
             AND p.deleted_at IS NOT NULL"#,
    )
    .bind(plugin_id)
    .fetch_one(pool)
    .await?)
}

#[tokio::test]
async fn t165_delete_blocked_when_function_exists() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = seed_admin(&pool, 3, 1, "test123").await?;
    let token = admin.token()?;

    // Upload a plugin
    let plugin_id = upload_plugin(&app, &token, "race-test-plugin").await?;

    // Create a function referencing it
    let _func_id = create_function(&app, &token, plugin_id, "race-test-func").await?;

    // Try to delete the plugin → must fail with 409 (reference blocked)
    let (status, body) = delete_plugin(&app, &token, plugin_id).await?;
    let code = body["code"].as_i64();
    assert_eq!(
        status, 409,
        "delete must be blocked when function references plugin; got {status} {body}"
    );
    assert_eq!(code, Some(4093), "expected 4093 ResourceInUse; got {body}");

    // Verify plugin is still not soft-deleted
    let (status, body) = get(&app, &format!("/api/plugins/{plugin_id}"), Some(&token)).await?;
    assert_eq!(status, 200);
    assert!(
        body["data"]["deleted_at"].is_null(),
        "plugin must not be soft-deleted after blocked attempt; got {body}"
    );

    Ok(())
}

#[tokio::test]
async fn t165_concurrent_delete_and_create_no_race() -> anyhow::Result<()> {
    let pool = common::test_pool().await?;
    let run_id = uuid::Uuid::new_v4().simple().to_string();

    for scenario in 1..=T165_RACE_SCENARIOS {
        let plugin_identifier = format!("concurrent-race-plugin-{run_id}-{scenario:03}");
        let function_identifier = format!("concurrent-race-function-{run_id}-{scenario:03}");
        let function_name = format!("Test Function {function_identifier}");
        let plugin_id = insert_plugin_fixture(&pool, &plugin_identifier).await?;

        let barrier = Arc::new(Barrier::new(2));

        let mut delete_handle = tokio::spawn({
            let barrier = barrier.clone();
            let pool = pool.clone();
            async move {
                barrier.wait().await;
                hiveweb::services::plugin::soft_delete(&pool, plugin_id).await
            }
        });

        let mut create_handle = tokio::spawn({
            let barrier = barrier.clone();
            let pool = pool.clone();
            async move {
                barrier.wait().await;
                hiveweb::services::function::create_custom(
                    &pool,
                    hiveweb::services::function::CreateMeta {
                        identifier: function_identifier,
                        name: function_name,
                        description: Some("T165 free-running race".to_string()),
                        plugin_id,
                        plugin_export: "test_export".to_string(),
                        input_schema: json!({"type": "object", "properties": {}}),
                        output_schema: json!({"type": "object", "properties": {}}),
                        category_id: None,
                        required_capabilities: None,
                        tag_ids: Vec::new(),
                    },
                )
                .await
            }
        });

        let (delete_result, create_result) =
            match tokio::time::timeout(Duration::from_secs(30), async {
                tokio::join!(&mut delete_handle, &mut create_handle)
            })
            .await
            {
                Ok(results) => results,
                Err(_) => {
                    delete_handle.abort();
                    create_handle.abort();
                    anyhow::bail!("scenario {scenario}: delete/create race timed out after 30s");
                }
            };
        let delete_result = delete_result?;
        let create_result = create_result?;

        match (&delete_result, &create_result) {
            (Ok(()), Err(error)) => {
                assert_eq!(
                    error.code(),
                    4093,
                    "scenario {scenario}: delete winner must make create fail with \
                     ResourceInUse; got {error}"
                );
            }
            (Err(error), Ok(_)) => {
                assert_eq!(
                    error.code(),
                    4093,
                    "scenario {scenario}: create winner must make delete fail with \
                     ResourceInUse; got {error}"
                );
            }
            _ => {
                panic!(
                    "scenario {scenario}: exactly one operation must succeed: \
                     delete={delete_result:?}, create={create_result:?}"
                );
            }
        }

        assert_eq!(
            deleted_plugin_reference_count(&pool, plugin_id).await?,
            0,
            "scenario {scenario}: a Function must never reference a soft-deleted Plugin"
        );

        sqlx::query("DELETE FROM functions WHERE plugin_id = ?")
            .bind(plugin_id)
            .execute(&pool)
            .await?;
        sqlx::query("DELETE FROM plugins WHERE id = ?")
            .bind(plugin_id)
            .execute(&pool)
            .await?;
    }

    Ok(())
}

#[tokio::test]
async fn t165_create_waits_for_delete_and_rejects_deleted_plugin() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = seed_admin(&pool, 3, 1, "test123").await?;
    let token = admin.token()?;
    let plugin_id = upload_plugin(&app, &token, "controlled-delete-wins-plugin").await?;

    // Reproduce the critical section of Plugin soft-delete while retaining the
    // row lock. Function creation must wait for this transaction to settle.
    let mut delete_tx = pool.begin().await?;
    let locked_plugin_id: i64 =
        sqlx::query_scalar("SELECT id FROM plugins WHERE id = ? FOR UPDATE")
            .bind(plugin_id)
            .fetch_one(&mut *delete_tx)
            .await?;
    assert_eq!(locked_plugin_id, plugin_id);

    let existing_references: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM functions WHERE plugin_id = ?")
            .bind(plugin_id)
            .fetch_one(&mut *delete_tx)
            .await?;
    assert_eq!(existing_references, 0);

    sqlx::query("UPDATE plugins SET deleted_at = NOW() WHERE id = ?")
        .bind(plugin_id)
        .execute(&mut *delete_tx)
        .await?;

    let mut create_handle = tokio::spawn({
        let app = app.clone();
        let token = token.clone();
        async move {
            create_function_response(&app, &token, plugin_id, "controlled-delete-wins-function")
                .await
        }
    });

    assert!(
        tokio::time::timeout(Duration::from_millis(500), &mut create_handle)
            .await
            .is_err(),
        "Function creation must wait while Plugin deletion owns the row lock"
    );

    delete_tx.commit().await?;

    let (create_status, create_body) = create_handle.await??;
    assert_eq!(
        create_status,
        StatusCode::CONFLICT,
        "create must lose after Plugin deletion commits; got {create_status} {create_body}"
    );
    assert_eq!(
        create_body["code"].as_i64(),
        Some(4093),
        "deleted Plugin must be rejected as ResourceInUse; got {create_body}"
    );
    assert_eq!(
        deleted_plugin_reference_count(&pool, plugin_id).await?,
        0,
        "delete winner must not leave a Function referencing the deleted Plugin"
    );

    Ok(())
}

#[tokio::test]
async fn t165_create_service_reports_resource_in_use_for_deleted_plugin() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = seed_admin(&pool, 3, 1, "test123").await?;
    let token = admin.token()?;
    let plugin_id = upload_plugin(&app, &token, "deleted-plugin-service-check").await?;

    let (delete_status, delete_body) = delete_plugin(&app, &token, plugin_id).await?;
    assert_eq!(
        delete_status,
        StatusCode::OK,
        "fixture delete failed: {delete_status} {delete_body}"
    );

    let error = hiveweb::services::function::create_custom(
        &pool,
        hiveweb::services::function::CreateMeta {
            identifier: "deleted-plugin-service-function".to_string(),
            name: "Deleted Plugin Service Function".to_string(),
            description: Some("SC-009 direct service regression".to_string()),
            plugin_id,
            plugin_export: "test_export".to_string(),
            input_schema: json!({"type": "object", "properties": {}}),
            output_schema: json!({"type": "object", "properties": {}}),
            category_id: None,
            required_capabilities: None,
            tag_ids: Vec::new(),
        },
    )
    .await
    .expect_err("soft-deleted Plugin must reject Function creation");

    assert!(
        matches!(&error, hiveweb::utils::error::AppError::ResourceInUse(_)),
        "expected ResourceInUse, got {error}"
    );
    assert_eq!(error.code(), 4093);

    Ok(())
}

#[tokio::test]
async fn t165_delete_succeeds_when_no_functions() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = seed_admin(&pool, 3, 1, "test123").await?;
    let token = admin.token()?;

    // Upload a plugin without any functions
    let plugin_id = upload_plugin(&app, &token, "no-func-plugin").await?;

    // Delete should succeed (no references)
    let (status, body) = delete_plugin(&app, &token, plugin_id).await?;
    assert_eq!(
        status, 200,
        "delete must succeed when no functions reference plugin; got {status} {body}"
    );

    // Verify soft-delete flag
    let (status, body) = get(&app, &format!("/api/plugins/{plugin_id}"), Some(&token)).await?;
    assert_eq!(status, 200);
    assert!(
        body["data"]["deleted_at"].as_str().is_some(),
        "plugin must be soft-deleted; got {body}"
    );

    Ok(())
}
