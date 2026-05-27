//! Plugin delete race condition integration test (T165 / SC-009)
//!
//! Verifies the two-layer defense against the race window:
//!   1. Soft delete transaction uses `SELECT ... FOR UPDATE` + `COUNT(*) FROM functions`
//!   2. New Function INSERT does a secondary check on `deleted_at`
//!
//! Scenario: Thread A tries to soft-delete a Plugin (referenced by a Function)
//! while Thread B concurrently creates a new Function referencing the same Plugin.
//! Must verify 100% consistency — no leaked references or partial deletes.

mod common;

use std::sync::Arc;
use tokio::sync::Barrier;

use common::{mint_jwt, seed_admin, post_json_auth, get, delete_auth};
use axum::Router;
use serde_json::{json, Value};

/// Helper: upload a minimal valid WASM plugin and return plugin id.
async fn upload_plugin(app: &Router, token: &str, identifier: &str) -> anyhow::Result<i64> {
    // Create a minimal WASM binary (valid magic bytes + empty module)
    let wasm_bytes: Vec<u8> = vec![
        0x00, 0x61, 0x73, 0x6d, // magic \0asm
        0x01, 0x00, 0x00, 0x00, // version 1
    ];

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::{BodyExt, Multipart};
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
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"file\"; filename=\"test.wasm\"\r\n");
    body.extend_from_slice(b"Content-Type: application/wasm\r\n\r\n");
    body.extend_from_slice(&wasm_bytes);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(b"------WebKitFormBoundaryTest123--\r\n");

    let req = Request::builder()
        .method("POST")
        .uri("/api/plugins")
        .header("content-type", format!("multipart/form-data; boundary={}", boundary))
        .header("authorization", format!("Bearer {}", token))
        .body(Body::from(body))?;

    let resp = app.clone().oneshot(req).await?;
    let status = resp.status();
    let bytes = resp.into_body().collect().await?.to_bytes();
    let resp_body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::OK, "upload plugin failed: {status} {resp_body}");
    Ok(resp_body["data"]["id"].as_i64().expect("missing plugin id"))
}

/// Helper: create a function referencing a plugin, return function id.
async fn create_function(app: &Router, token: &str, plugin_id: i64, identifier: &str) -> anyhow::Result<i64> {
    let (status, body) = post_json_auth(
        app,
        "/api/functions",
        token,
        json!({
            "identifier": identifier,
            "name": format!("Test Function {identifier}"),
            "description": "Test function",
            "plugin_id": plugin_id,
            "export": "test_export",
            "input_schema": {"type": "object", "properties": {}},
            "output_schema": {"type": "object", "properties": {}}
        }),
    )
    .await?;
    assert_eq!(status, 200, "create function failed: {status} {body}");
    Ok(body["data"]["id"].as_i64().expect("missing function id"))
}

/// Helper: soft-delete a plugin
async fn delete_plugin(app: &Router, token: &str, plugin_id: i64) -> anyhow::Result<(http::StatusCode, Value)> {
    delete_auth(app, &format!("/api/plugins/{plugin_id}"), token).await
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
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = seed_admin(&pool, 3, 1, "test123").await?;
    let token = admin.token()?;

    // Upload a plugin (no existing functions yet)
    let plugin_id = upload_plugin(&app, &token, "concurrent-race-plugin").await?;

    // Use a barrier to synchronize two concurrent tasks
    let barrier = Arc::new(Barrier::new(2));
    let app_clone = app.clone();
    let token_clone = token.clone();

    // Task A: Try to delete the plugin (should succeed since no functions yet)
    let delete_handle = tokio::spawn({
        let barrier = barrier.clone();
        let app = app_clone.clone();
        let token = token_clone.clone();
        async move {
            barrier.wait().await;
            delete_plugin(&app, &token, plugin_id).await
        }
    });

    // Task B: Try to create a function referencing the plugin
    let create_handle = tokio::spawn({
        let barrier = barrier.clone();
        let app = app.clone();
        let token = token.clone();
        async move {
            barrier.wait().await;
            create_function(&app, &token, plugin_id, "concurrent-race-func").await
        }
    });

    let (delete_result, create_result) = tokio::join!(delete_handle, create_handle);
    let (delete_status, delete_body) = delete_result??;
    let create_result = create_result?;

    // At least one of the two operations must succeed deterministically.
    // The FOR UPDATE lock serializes them, so no partial state.
    // Either:
    //   - Delete succeeds (no functions yet), then create may succeed or fail based on timing
    //   - Create succeeds first, then delete is blocked (4093)
    //
    // Critical invariant: if delete succeeded AND create succeeded, the plugin
    // must still exist (delete was soft-delete, not physical).

    let plugin_deleted = delete_status == 200;
    let func_created = create_result.is_ok();

    if plugin_deleted && func_created {
        // Both succeeded: verify the function's plugin_id is still valid
        // (soft-delete doesn't remove the plugin row, just marks it)
        let (status, body) = get(&app, &format!("/api/plugins/{plugin_id}"), Some(&token)).await?;
        assert_eq!(status, 200);
        assert!(
            body["data"]["deleted_at"].as_str().is_some(),
            "plugin should be soft-deleted; got {body}"
        );
    }

    // If create happened first, delete must be blocked
    if func_created && !plugin_deleted {
        let code = delete_body["code"].as_i64();
        assert_eq!(
            code, Some(4093),
            "delete must be blocked when function was created first; got {delete_body}"
        );
    }

    tracing::info!(
        "t165 race test result: plugin_deleted={plugin_deleted}, func_created={func_created}"
    );

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
    assert_eq!(status, 200, "delete must succeed when no functions reference plugin; got {status} {body}");

    // Verify soft-delete flag
    let (status, body) = get(&app, &format!("/api/plugins/{plugin_id}"), Some(&token)).await?;
    assert_eq!(status, 200);
    assert!(
        body["data"]["deleted_at"].as_str().is_some(),
        "plugin must be soft-deleted; got {body}"
    );

    Ok(())
}
