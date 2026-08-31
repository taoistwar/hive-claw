//! Plugin memory limit integration test (T166 / FR-032).
//!
//! This test exercises the real MySQL → S3 → InstancePool → Invoker → Extism
//! path with a WAT-generated Plugin. A control export grows to exactly 128 MiB
//! and succeeds; a second export requests 129 MiB and must fail with a genuine
//! linear-memory bounds trap. The HiveWeb infrastructure CI job supplies
//! disposable MySQL, Redis, and MinIO services.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use hiveweb::runtime::RuntimeExecutionContext;
use hiveweb::runtime::capability::{CapabilityRegistry, DispatchCtx};
use hiveweb::runtime::invoker::{Invoker, InvokerError};
use hiveweb::runtime::llm::LlmRegistry;
use hiveweb::runtime::pool::{InstancePool, PoolConfig};
use hiveweb::utils::error::{AppError, codes, http_status_for_code};
use sha2::{Digest, Sha256};

const MEMORY_LIMIT_PROBE_WAT: &str = r#"
    (module
      (import "extism:host/env" "alloc"
        (func $abi_alloc (param i64) (result i64)))
      (import "extism:host/env" "store_u8"
        (func $abi_store_u8 (param i64 i32)))
      (import "extism:host/env" "output_set"
        (func $abi_output_set (param i64 i64)))
      (memory (export "memory") 1)

      (func (export "_hive_plugin_abi_version") (result i32)
        (local $abi_ptr i64)
        (local.set $abi_ptr (call $abi_alloc (i64.const 14)))
        (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 0)) (i32.const 104))
        (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 1)) (i32.const 105))
        (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 2)) (i32.const 118))
        (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 3)) (i32.const 101))
        (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 4)) (i32.const 45))
        (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 5)) (i32.const 101))
        (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 6)) (i32.const 120))
        (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 7)) (i32.const 116))
        (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 8)) (i32.const 105))
        (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 9)) (i32.const 115))
        (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 10)) (i32.const 109))
        (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 11)) (i32.const 47))
        (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 12)) (i32.const 118))
        (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 13)) (i32.const 49))
        (call $abi_output_set (local.get $abi_ptr) (i64.const 14))
        i32.const 0)

      (func (export "allocate_within_limit")
        i32.const 2047
        memory.grow
        i32.const -1
        i32.eq
        if
          unreachable
        end
        i32.const 134217727
        i32.const 1
        i32.store8)

      (func (export "allocate_over_limit")
        i32.const 2063
        memory.grow
        drop
        i32.const 134217728
        i32.const 1
        i32.store8))
"#;

fn assert_memory_limit_evidence(error: &str) {
    let normalized = error.to_ascii_lowercase();
    let explicit_oom = normalized == "oom" || normalized.contains("out of memory");
    let memory_boundary = normalized.contains("memory")
        && (normalized.contains("out of bounds")
            || normalized.contains("out-of-bounds")
            || normalized.contains("limit")
            || normalized.contains("maximum")
            || normalized.contains("grow"));
    assert!(
        explicit_oom || memory_boundary,
        "trap must contain OOM or linear-memory boundary evidence: {error}"
    );
    assert!(
        !normalized.contains("unreachable"),
        "probe must not pass through an arbitrary `unreachable` trap: {error}"
    );
}

#[derive(Clone, Default)]
struct TraceWriter(Arc<Mutex<Vec<u8>>>);

struct TraceWriterGuard(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for TraceWriterGuard {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("trace buffer poisoned").extend(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TraceWriter {
    type Writer = TraceWriterGuard;

    fn make_writer(&'a self) -> Self::Writer {
        TraceWriterGuard(Arc::clone(&self.0))
    }
}

#[tokio::test]
async fn t166_real_plugin_memory_limit_maps_5000_audits_and_discards_instance() -> anyhow::Result<()>
{
    let db_pool = common::test_pool().await?;
    let s3 = hiveweb::storage::s3::create_client().await?;
    let wasm = wat::parse_str(MEMORY_LIMIT_PROBE_WAT)?;

    let identifier = format!("t166-{}", uuid::Uuid::new_v4().simple());
    let s3_key = format!("plugins/{identifier}/1.0.0.wasm");
    let sha256 = format!("{:x}", Sha256::digest(&wasm));
    let wasm_size = i64::try_from(wasm.len())?;

    hiveweb::storage::s3::put_wasm(&s3, &s3_key, wasm.clone()).await?;
    let insert = sqlx::query(
        r#"
        INSERT INTO plugins
            (identifier, version, name, description, sha256, size_bytes, s3_key,
             created_at, updated_at)
        VALUES (?, '1.0.0', ?, 'T166 real memory-limit plugin', ?, ?, ?, NOW(), NOW())
        "#,
    )
    .bind(&identifier)
    .bind(&identifier)
    .bind(&sha256)
    .bind(wasm_size)
    .bind(&s3_key)
    .execute(&db_pool)
    .await;
    let insert = match insert {
        Ok(insert) => insert,
        Err(error) => {
            if let Err(cleanup_error) = hiveweb::storage::s3::delete_wasm(&s3, &s3_key).await {
                tracing::warn!(
                    error = %cleanup_error,
                    "failed to clean up T166 object after database insert failure"
                );
            }
            return Err(error.into());
        }
    };
    let plugin_id = insert.last_insert_id() as i64;

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
    let dispatch_ctx = DispatchCtx {
        execution_context: RuntimeExecutionContext::best_effort(
            Some("t166-memory-limit".into()),
            None,
        ),
        agent_id: 0,
        plugin_id,
        function_id: None,
        permissions: Vec::new(),
    };

    let trace_writer = TraceWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_target(false)
        .with_writer(trace_writer.clone())
        .finish();
    let _trace_guard = tracing::subscriber::set_default(subscriber);

    let observations: anyhow::Result<_> = async {
        let registry = Arc::new(CapabilityRegistry::new());
        let llm = Arc::new(LlmRegistry::new());
        let control = invoker
            .invoke(
                &db_pool,
                Some(&s3),
                Arc::clone(&registry),
                Arc::clone(&llm),
                plugin_id,
                "allocate_within_limit",
                "{}".into(),
                dispatch_ctx.clone(),
            )
            .await;
        let control_metrics = pool.metrics_snapshot().await;
        let control_per_plugin = pool.per_plugin_snapshot().await;

        let invocation = invoker
            .invoke(
                &db_pool,
                Some(&s3),
                registry,
                llm,
                plugin_id,
                "allocate_over_limit",
                "{}".into(),
                dispatch_ctx,
            )
            .await;
        let metrics = pool.metrics_snapshot().await;
        let per_plugin = pool.per_plugin_snapshot().await;
        let trace_output = String::from_utf8(
            trace_writer
                .0
                .lock()
                .expect("trace buffer poisoned")
                .clone(),
        )?;
        Ok((
            control,
            control_metrics,
            control_per_plugin,
            invocation,
            metrics,
            per_plugin,
            trace_output,
        ))
    }
    .await;

    let delete_object = hiveweb::storage::s3::delete_wasm(&s3, &s3_key).await;
    let delete_row = sqlx::query("DELETE FROM plugins WHERE id = ?")
        .bind(plugin_id)
        .execute(&db_pool)
        .await;
    let (
        control,
        control_metrics,
        control_per_plugin,
        invocation,
        metrics,
        per_plugin,
        trace_output,
    ) = match observations {
        Ok(observations) => observations,
        Err(error) => {
            if let Err(cleanup_error) = &delete_object {
                tracing::warn!(error = %cleanup_error, "failed to clean up T166 object");
            }
            if let Err(cleanup_error) = &delete_row {
                tracing::warn!(error = %cleanup_error, "failed to clean up T166 database row");
            }
            return Err(error);
        }
    };
    delete_object?;
    delete_row?;

    let control_output =
        control.expect("allocation and write at the 128 MiB boundary must succeed");
    assert!(control_output.is_empty(), "probe returns no output payload");
    assert_eq!(control_metrics.in_use, 0);
    assert_eq!(
        control_metrics.idle, 1,
        "successful control instance must return to the idle pool"
    );
    assert_eq!(control_metrics.created_total, 1);
    assert_eq!(control_per_plugin.len(), 1);
    assert_eq!(control_per_plugin[0].idle, 1);
    assert_eq!(control_per_plugin[0].in_use, 0);

    let error = invocation.expect_err("129 MiB allocation must exceed the 128 MiB limit");
    let raw_error = match &error {
        InvokerError::PluginError(message) => message.clone(),
        other => panic!("memory trap must remain a plugin runtime error, got {other:?}"),
    };
    assert_memory_limit_evidence(&raw_error);

    let app_error: AppError = error.into();
    assert_eq!(app_error.code(), codes::INTERNAL);
    assert_eq!(app_error.message(), "Plugin 执行失败或超过内存上限");
    assert!(
        !app_error.message().contains(&raw_error),
        "external error must not expose the runtime trap"
    );
    assert_eq!(
        http_status_for_code(app_error.code()),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_ne!(app_error.code(), codes::PLUGIN_INVOCATION_TIMEOUT);

    assert_eq!(metrics.in_use, 0, "failed instance must release its slot");
    assert_eq!(
        metrics.idle, 0,
        "failed instance must not return to idle pool"
    );
    assert_eq!(metrics.created_total, 1);
    assert_eq!(
        metrics.reset_failures, 0,
        "a trapped invocation is a discard, not a reset failure"
    );
    assert_eq!(per_plugin.len(), 1);
    assert_eq!(per_plugin[0].idle, 0);
    assert_eq!(per_plugin[0].in_use, 0);

    assert!(trace_output.contains("runtime_audit"));
    assert!(trace_output.contains("plugin_invoke"));
    assert!(trace_output.contains("t166-memory-limit"));
    assert!(trace_output.contains("error_message"));
    assert!(
        trace_output.contains("outcome=\"error\"") || trace_output.contains("outcome=error"),
        "memory-limit failure must emit an error audit: {trace_output}"
    );

    Ok(())
}
