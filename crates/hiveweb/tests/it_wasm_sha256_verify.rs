//! WASM SHA-256 verification integration test (T167 / FR-029).
//!
//! This test exercises the real MySQL → S3 → InstancePool → Invoker path:
//! production upload persists the digest and object, the object is then
//! tampered with, and a cold pool load must reject it before instantiation.
//! The HiveWeb infrastructure CI job supplies disposable MySQL and MinIO.
//!
//! A cold-load SHA mismatch currently returns before `Invoker` records a
//! runtime audit event, so this test deliberately makes no audit assertion.

mod common;

use std::sync::Arc;
use std::time::Duration;

use hiveweb::runtime::RuntimeExecutionContext;
use hiveweb::runtime::capability::{CapabilityRegistry, DispatchCtx};
use hiveweb::runtime::invoker::{Invoker, InvokerError};
use hiveweb::runtime::llm::LlmRegistry;
use hiveweb::runtime::pool::{InstancePool, PoolConfig, PoolError};
use hiveweb::services::plugin::UploadMeta;
use sha2::{Digest, Sha256};

const SHARED_SMOKE_WASM: &[u8] =
    include_bytes!("../../hivegui/tests/fixtures/plugins/shared-smoke/plugin.wasm");

#[tokio::test]
async fn t167_tampered_wasm_is_rejected_by_real_invoker_without_pooling_instance()
-> anyhow::Result<()> {
    let db_pool = common::test_pool().await?;
    let s3 = hiveweb::storage::s3::create_client().await?;
    let identifier = format!("t167-{}", uuid::Uuid::new_v4().simple());
    let s3_key = format!("plugins/{identifier}/1.0.0.wasm");
    let expected_sha256 = format!("{:x}", Sha256::digest(SHARED_SMOKE_WASM));

    let upload = hiveweb::services::plugin::upload(
        &db_pool,
        Some(&s3),
        UploadMeta {
            identifier: identifier.clone(),
            name: identifier.clone(),
            version: "1.0.0".into(),
            description: Some("T167 real SHA-256 verification plugin".into()),
            manifest: None,
            runtime: "extism".into(),
            author: Some("integration-test".into()),
            repository_url: None,
            category_id: None,
            tag_ids: Vec::new(),
        },
        SHARED_SMOKE_WASM.to_vec(),
    )
    .await;
    let plugin = match upload {
        Ok(plugin) => plugin,
        Err(error) => {
            if let Err(cleanup_error) = hiveweb::storage::s3::delete_wasm(&s3, &s3_key).await {
                tracing::warn!(
                    error = %cleanup_error,
                    "failed to clean up T167 object after upload failure"
                );
            }
            if let Err(cleanup_error) =
                sqlx::query("DELETE FROM plugins WHERE identifier = ? AND version = '1.0.0'")
                    .bind(&identifier)
                    .execute(&db_pool)
                    .await
            {
                tracing::warn!(
                    error = %cleanup_error,
                    "failed to clean up T167 database row after upload failure"
                );
            }
            return Err(anyhow::anyhow!("production plugin upload failed: {error}"));
        }
    };

    let pool = InstancePool::new(PoolConfig {
        max_per_plugin: 1,
        max_total: 1,
        idle_timeout: Duration::from_secs(60),
        acquire_timeout: Duration::from_secs(5),
        call_timeout_ms: 5_000,
        call_memory_mb: 128,
        call_fuel: 10_000_000_000,
    });
    let invoker = Invoker::new(Arc::clone(&pool));

    let observations: anyhow::Result<_> = async {
        let stored_sha256: String = sqlx::query_scalar("SELECT sha256 FROM plugins WHERE id = ?")
            .bind(plugin.id)
            .fetch_one(&db_pool)
            .await?;
        anyhow::ensure!(
            stored_sha256 == expected_sha256,
            "production upload stored an unexpected digest: stored={stored_sha256} expected={expected_sha256}"
        );

        let mut tampered = hiveweb::storage::s3::get_wasm(&s3, &plugin.s3_key).await?;
        let last = tampered
            .last_mut()
            .ok_or_else(|| anyhow::anyhow!("uploaded WASM object must not be empty"))?;
        *last ^= 0x01;
        anyhow::ensure!(
            format!("{:x}", Sha256::digest(&tampered)) != stored_sha256,
            "tampering must change the object digest"
        );
        hiveweb::storage::s3::put_wasm(&s3, &plugin.s3_key, tampered).await?;

        let invocation = invoker
            .invoke(
                &db_pool,
                Some(&s3),
                Arc::new(CapabilityRegistry::new()),
                Arc::new(LlmRegistry::new()),
                plugin.id,
                "unused_after_sha_mismatch",
                "{}".into(),
                DispatchCtx {
                    execution_context: RuntimeExecutionContext::best_effort(
                        Some("t167-sha-mismatch".into()),
                        None,
                    ),
                    agent_id: 0,
                    plugin_id: plugin.id,
                    function_id: None,
                    permissions: Vec::new(),
                },
            )
            .await;

        Ok((
            invocation,
            pool.metrics_snapshot().await,
            pool.per_plugin_snapshot().await,
        ))
    }
    .await;

    let delete_object = hiveweb::storage::s3::delete_wasm(&s3, &plugin.s3_key).await;
    let delete_row = sqlx::query("DELETE FROM plugins WHERE id = ?")
        .bind(plugin.id)
        .execute(&db_pool)
        .await;
    let (invocation, metrics, per_plugin) = match observations {
        Ok(observations) => observations,
        Err(error) => {
            if let Err(cleanup_error) = &delete_object {
                tracing::warn!(error = %cleanup_error, "failed to clean up T167 object");
            }
            if let Err(cleanup_error) = &delete_row {
                tracing::warn!(error = %cleanup_error, "failed to clean up T167 database row");
            }
            return Err(error);
        }
    };
    delete_object?;
    delete_row?;

    assert!(
        matches!(
            &invocation,
            Err(InvokerError::Pool(PoolError::Sha256Mismatch))
        ),
        "real Invoker cold load must reject the tampered object, got {invocation:?}"
    );
    assert_eq!(metrics.created_total, 0);
    assert_eq!(metrics.cache_misses, 0);
    assert_eq!(metrics.in_use, 0);
    assert_eq!(
        metrics.idle, 0,
        "SHA mismatch must not create or return an idle instance"
    );
    assert!(
        per_plugin.is_empty(),
        "SHA mismatch occurs before a per-plugin pool entry is created"
    );

    Ok(())
}
