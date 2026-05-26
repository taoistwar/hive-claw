//! Contract tests for /api/plugins/* (T037 / T040 / T070-T073 — US1).
//!
//! Red-phase tests written before Plugin handler exists; expected to fail
//! with 404 until US1 backend wires the routes.

mod common;

#[tokio::test]
async fn t037_plugins_list_requires_auth() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let (status, _) = common::get(&app, "/api/plugins?offset=0&limit=20", None).await?;
    assert_eq!(
        status, 401,
        "unauthenticated /api/plugins must return 401, got {status}"
    );
    Ok(())
}

#[tokio::test]
async fn t037_plugins_list_returns_envelope() -> anyhow::Result<()> {
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 2 /* System */, 1, "test-pass-123").await?;

    let (status, body) =
        common::get(&app, "/api/plugins?offset=0&limit=20", Some(&admin.token()?)).await?;

    assert_eq!(status, 200, "GET /api/plugins must return 200; got {status}: {body}");
    // contracts/api.md §0 envelope: { code: 0, message, data: { items, total, offset, limit } }
    assert_eq!(body["code"], 0, "envelope code must be 0");
    let data = &body["data"];
    assert!(data.is_object(), "data must be object");
    assert!(data["items"].is_array(), "data.items must be array");
    assert!(data["total"].is_number(), "data.total must be number");
    assert_eq!(data["offset"], 0);
    assert_eq!(data["limit"], 20);
    Ok(())
}

#[tokio::test]
async fn t040_plugin_delete_blocks_when_referenced() -> anyhow::Result<()> {
    // Spec FR-007 / SC-009: deleting a plugin still referenced by a function
    // must return 4093 ResourceInUse (HTTP 409).
    let app = common::test_app().await?;
    let pool = common::test_pool().await?;
    let admin = common::seed_admin(&pool, 2 /* System */, 1, "test-pass-123").await?;

    // Seed a plugin row + a function row referencing it. Use raw SQL so we
    // don't depend on the upload pipeline being wired yet.
    let res = sqlx::query(
        r#"INSERT INTO plugins (identifier, name, version, runtime, s3_key, sha256, size_bytes)
           VALUES (?, ?, ?, 'extism', ?, ?, ?)"#,
    )
    .bind(format!("plg-test-{}", admin.id))
    .bind("test plugin")
    .bind("1.0.0")
    .bind("plugins/test/1.0.0.wasm")
    .bind("0".repeat(64))
    .bind(1024_i64)
    .execute(&pool)
    .await?;
    let plugin_id = res.last_insert_id() as i64;

    sqlx::query(
        r#"INSERT INTO functions (identifier, name, kind, input_schema, output_schema, plugin_id, plugin_export)
           VALUES (?, 'fn-test', 2, '{}', '{}', ?, 'lookup')"#,
    )
    .bind(format!("fn-test-{plugin_id}"))
    .bind(plugin_id)
    .execute(&pool)
    .await?;

    let (status, body) = common::delete_auth(
        &app,
        &format!("/api/plugins/{plugin_id}"),
        &admin.token()?,
    )
    .await?;

    // contracts §Errors: 4093 ResourceInUse → HTTP 409
    assert_eq!(
        status, 409,
        "deleting referenced plugin must return 409; got {status}: {body}"
    );
    assert_eq!(body["code"], 4093, "business code must be 4093 ResourceInUse");

    // Cleanup
    sqlx::query("DELETE FROM functions WHERE plugin_id = ?")
        .bind(plugin_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM plugins WHERE id = ?")
        .bind(plugin_id)
        .execute(&pool)
        .await?;

    Ok(())
}
