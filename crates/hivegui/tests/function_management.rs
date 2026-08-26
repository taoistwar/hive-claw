//! T083 [P] [US9] Function management contract.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T083
//! ("编写 `builtin|custom|placeholder` 稳定字符串 kind、旧整数 `1/2/3`
//! 迁移、四个下划线保留 identifier、点号记录/别名数量为零、Builtin
//! 不可变、Custom Plugin/export/schema/RESTRICT、Placeholder 三个执行
//! 关系字段为空、每页20条搜索分页、全部生产过滤/关联查询 EXPLAIN
//! 预期索引、CRUD p95≤1s 及搜索/翻页 p95≤500ms 测试").
//!
//! Red public boundaries (T087 will add these):
//!   - `hivegui::datasource::function_store::FunctionStore`
//!   - `hivegui::datasource::function_store::FunctionInput`
//!   - `hivegui::datasource::function_store::FunctionRecord`
//!   - `hivegui::datasource::function_store::FunctionKind` enum
//!     (Builtin | Custom | Placeholder)
//!   - `hivegui::datasource::function_store::RESERVED_UNDERSCORE_IDENTIFIERS`
//!
//! T086 reviewer signs §T083.11 + §T084.11 + §T085.11; T087-T088
//! implementation then make these tests Green; T089 reruns to record
//! the Green evidence.

mod support;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use chrono::{NaiveDate, Utc};
use hive_builtins::{format_template, json_parse, json_stringify, text_regex_match};
use hivegui::datasource::function_store::{
    FunctionInput, FunctionKind, FunctionPage, FunctionRecord, FunctionStore,
    RESERVED_UNDERSCORE_IDENTIFIERS,
};
use hivegui::datasource::{
    entity_store::{
        Category as EntityCategory, Tool as EntityTool, Workflow as EntityWorkflow,
        WorkflowNode as EntityWorkflowNode, register_runtime_capabilities,
    },
    plugin_manifest::validate_manifest,
    query_plan::{QueryDialect, production_query_catalog},
    store::{Store, StoreOpenOptions},
    validation::{PublicBoundaryError, PublicErrorEnvelope},
};
use sqlx::{Pool, Row, Sqlite};
use support::{
    TestWorkspace,
    performance::{
        BaselineApproval, BenchmarkBaseline, BenchmarkReport, ComparisonOutcome,
        EnvironmentFingerprint, FUNCTION_CRUD_ID, FUNCTION_FIXTURE_ROWS, FUNCTION_SEARCH_PAGE_ID,
        FUNCTION_SEARCH_PAGE_SCHEDULE, MEASURED_SAMPLES, PercentilesNs, baseline_path,
        compare_to_baseline, evaluate_benchmark_gate, regression_exception_path, run_target,
        source_revision, target_specs,
    },
};

const PAGE_SIZE: usize = 20;
const FIXTURE_TIMESTAMP: &str = "2026-08-20T00:00:00Z";
const MAX_SCHEMA_BYTES: usize = 1024 * 1024;
const MIGRATION_FIXTURE_DIRECTORY: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/migrations");

async fn open_v4_store(workspace: &TestWorkspace) -> Store {
    Store::open_local(StoreOpenOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("open real v4 Store")
}

async fn seed_known_network_http_capability(pool: &Pool<Sqlite>) {
    register_runtime_capabilities(pool)
        .await
        .expect("seed desktop Capability registry");
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM capabilities WHERE name = 'network.http'",
    )
    .fetch_one(pool)
    .await
    .expect("verify known Capability fixture");
    assert_eq!(count, 1, "network.http must be a known v4 Capability");
}

async fn seed_plugin(pool: &Pool<Sqlite>, identifier: &str, export: &str) -> i64 {
    let manifest = serde_json::json!({
        "abi_version": "hive-extism/v1",
        "exports": [{"name": export, "input": "json", "output": "json"}],
        "required_capabilities": ["network.http"]
    })
    .to_string();
    let known_capabilities = sqlx::query_scalar::<_, String>("SELECT name FROM capabilities")
        .fetch_all(pool)
        .await
        .expect("load known Capability registry")
        .into_iter()
        .collect::<BTreeSet<_>>();
    validate_manifest(&manifest, &known_capabilities)
        .unwrap_or_else(|issues| panic!("Plugin fixture manifest must satisfy ABI v1: {issues:?}"));
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO plugins (identifier, name, manifest, runtime, version, author, \
         repository_url, s3_key, sha256, size_bytes, capabilities, resource_limits, \
         created_at, updated_at) \
         VALUES (?, ?, ?, 'extism', '1.0.0', '', '', ?, ?, 1, '[\"network.http\"]', \
         '{}', ?, ?) RETURNING id",
    )
    .bind(identifier)
    .bind(format!("Plugin {identifier}"))
    .bind(manifest)
    .bind(format!("{identifier}/1.0.0/fixture/plugin.wasm"))
    .bind("0".repeat(64))
    .bind(FIXTURE_TIMESTAMP)
    .bind(FIXTURE_TIMESTAMP)
    .fetch_one(pool)
    .await
    .expect("seed Plugin fixture")
}

async fn seed_placeholder(
    pool: &Pool<Sqlite>,
    identifier: impl Into<String>,
    name: impl Into<String>,
) -> FunctionRecord {
    let input = FunctionInput::for_write(
        identifier.into(),
        name.into(),
        None,
        FunctionKind::Placeholder,
        r#"{"type":"object"}"#.to_string(),
        r#"{"type":"object"}"#.to_string(),
        None,
        None,
        None,
        None,
    )
    .expect("validate placeholder fixture");
    FunctionStore::new(pool.clone())
        .expect("Function Store")
        .create(input)
        .await
        .expect("create placeholder fixture")
}

#[derive(Debug, Clone)]
struct FunctionDraft {
    identifier: String,
    name: String,
    description: Option<String>,
    kind: FunctionKind,
    input_schema: String,
    output_schema: String,
    plugin_id: Option<i64>,
    plugin_export: Option<String>,
    category_id: Option<i64>,
    required_capabilities: Option<String>,
}

impl FunctionDraft {
    fn placeholder(identifier: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            identifier: identifier.into(),
            name: name.into(),
            description: None,
            kind: FunctionKind::Placeholder,
            input_schema: r#"{"type":"object"}"#.to_string(),
            output_schema: r#"{"type":"object"}"#.to_string(),
            plugin_id: None,
            plugin_export: None,
            category_id: None,
            required_capabilities: None,
        }
    }

    fn custom(
        identifier: impl Into<String>,
        name: impl Into<String>,
        plugin_id: i64,
        plugin_export: impl Into<String>,
    ) -> Self {
        Self {
            kind: FunctionKind::Custom,
            plugin_id: Some(plugin_id),
            plugin_export: Some(plugin_export.into()),
            ..Self::placeholder(identifier, name)
        }
    }

    fn into_input(self) -> anyhow::Result<FunctionInput> {
        Ok(FunctionInput::for_write(
            self.identifier,
            self.name,
            self.description,
            self.kind,
            self.input_schema,
            self.output_schema,
            self.plugin_id,
            self.plugin_export,
            self.category_id,
            self.required_capabilities,
        )?)
    }
}

async fn create_function(store: &FunctionStore, draft: FunctionDraft) -> anyhow::Result<i64> {
    let created = store.create(draft.into_input()?).await?;
    Ok(created.id())
}

async fn create_placeholder_function(
    store: &FunctionStore,
    identifier: impl Into<String>,
    name: impl Into<String>,
) -> i64 {
    create_function(store, FunctionDraft::placeholder(identifier, name))
        .await
        .expect("create Placeholder through FunctionStore")
}

async fn update_function(
    store: &FunctionStore,
    id: i64,
    draft: FunctionDraft,
) -> anyhow::Result<i64> {
    let updated = store.update(id, draft.into_input()?).await?;
    Ok(updated.id())
}

async fn delete_function(store: &FunctionStore, id: i64) -> anyhow::Result<()> {
    store.delete(id).await?;
    Ok(())
}

async fn get_function(store: &FunctionStore, id: i64) -> anyhow::Result<Option<FunctionRecord>> {
    Ok(store.get(id).await?)
}

fn parse_function_kind(value: &str) -> anyhow::Result<FunctionKind> {
    Ok(FunctionKind::try_from(value)?)
}

fn object_schema_with_exact_size(bytes: usize) -> String {
    const PREFIX: &str = r#"{"type":"object","description":""#;
    const SUFFIX: &str = r#""}"#;
    assert!(bytes >= PREFIX.len() + SUFFIX.len());
    let schema = format!(
        "{PREFIX}{}{SUFFIX}",
        "x".repeat(bytes - PREFIX.len() - SUFFIX.len())
    );
    assert_eq!(schema.len(), bytes);
    schema
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FunctionSnapshot {
    id: i64,
    identifier: String,
    name: String,
    description: Option<String>,
    kind: String,
    input_schema: String,
    output_schema: String,
    plugin_id: Option<i64>,
    plugin_export: Option<String>,
    category_id: Option<i64>,
    required_capabilities: Option<String>,
    created_at: String,
    updated_at: String,
}

async fn function_snapshot(pool: &Pool<Sqlite>) -> Vec<FunctionSnapshot> {
    sqlx::query(
        "SELECT id, identifier, name, description, kind, input_schema, output_schema, \
         plugin_id, plugin_export, category_id, required_capabilities, created_at, updated_at \
         FROM functions ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .expect("snapshot Function rows")
    .into_iter()
    .map(|row| FunctionSnapshot {
        id: row.get("id"),
        identifier: row.get("identifier"),
        name: row.get("name"),
        description: row.get("description"),
        kind: row.get("kind"),
        input_schema: row.get("input_schema"),
        output_schema: row.get("output_schema"),
        plugin_id: row.get("plugin_id"),
        plugin_export: row.get("plugin_export"),
        category_id: row.get("category_id"),
        required_capabilities: row.get("required_capabilities"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
    .collect()
}

#[derive(Debug, Clone, PartialEq)]
struct WorkflowNodeSnapshot {
    id: i64,
    workflow_id: i64,
    node_key: String,
    node_type: String,
    function_id: Option<i64>,
    position_x: f64,
    position_y: f64,
    node_config: Option<String>,
    created_at: String,
}

async fn workflow_node_snapshot(
    pool: &Pool<Sqlite>,
    workflow_id: i64,
) -> Vec<WorkflowNodeSnapshot> {
    sqlx::query(
        "SELECT id, workflow_id, node_key, node_type, function_id, position_x, position_y, \
         node_config, created_at FROM workflow_nodes WHERE workflow_id = ? ORDER BY id",
    )
    .bind(workflow_id)
    .fetch_all(pool)
    .await
    .expect("snapshot WorkflowNode rows")
    .into_iter()
    .map(|row| WorkflowNodeSnapshot {
        id: row.get("id"),
        workflow_id: row.get("workflow_id"),
        node_key: row.get("node_key"),
        node_type: row.get("node_type"),
        function_id: row.get("function_id"),
        position_x: row.get("position_x"),
        position_y: row.get("position_y"),
        node_config: row.get("node_config"),
        created_at: row.get("created_at"),
    })
    .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolSnapshot {
    id: i64,
    identifier: String,
    name: String,
    description: String,
    kind: String,
    source: String,
    is_always: bool,
    function_id: Option<i64>,
    workflow_id: Option<i64>,
    input_schema: String,
    output_schema: String,
    category_id: Option<i64>,
    required_capabilities: Option<String>,
    created_at: String,
    updated_at: String,
}

async fn tool_snapshot(pool: &Pool<Sqlite>, id: i64) -> Option<ToolSnapshot> {
    sqlx::query(
        "SELECT id, identifier, name, description, kind, source, is_always, function_id, \
         workflow_id, input_schema, output_schema, category_id, required_capabilities, \
         created_at, updated_at FROM tools WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .expect("snapshot Tool row")
    .map(|row| ToolSnapshot {
        id: row.get("id"),
        identifier: row.get("identifier"),
        name: row.get("name"),
        description: row.get("description"),
        kind: row.get("kind"),
        source: row.get("source"),
        is_always: row.get::<i64, _>("is_always") != 0,
        function_id: row.get("function_id"),
        workflow_id: row.get("workflow_id"),
        input_schema: row.get("input_schema"),
        output_schema: row.get("output_schema"),
        category_id: row.get("category_id"),
        required_capabilities: row.get("required_capabilities"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn builtin_contracts() -> BTreeMap<&'static str, (&'static str, &'static str)> {
    BTreeMap::from([
        (
            "format_template",
            (
                format_template::FORMAT_TEMPLATE_INPUT_SCHEMA,
                format_template::FORMAT_TEMPLATE_OUTPUT_SCHEMA,
            ),
        ),
        (
            "json_parse",
            (
                json_parse::JSON_PARSE_INPUT_SCHEMA,
                json_parse::JSON_PARSE_OUTPUT_SCHEMA,
            ),
        ),
        (
            "json_stringify",
            (
                json_stringify::JSON_STRINGIFY_INPUT_SCHEMA,
                json_stringify::JSON_STRINGIFY_OUTPUT_SCHEMA,
            ),
        ),
        (
            "text_regex_match",
            (
                text_regex_match::TEXT_REGEX_MATCH_INPUT_SCHEMA,
                text_regex_match::TEXT_REGEX_MATCH_OUTPUT_SCHEMA,
            ),
        ),
    ])
}

async fn seed_canonical_builtins(pool: &Pool<Sqlite>) {
    for (identifier, (input_schema, output_schema)) in builtin_contracts() {
        sqlx::query(
            "INSERT OR IGNORE INTO functions (identifier, name, kind, input_schema, \
             output_schema, plugin_id, plugin_export, required_capabilities, created_at, updated_at) \
             VALUES (?, ?, 'builtin', ?, ?, NULL, NULL, NULL, ?, ?)",
        )
        .bind(identifier)
        .bind(identifier)
        .bind(input_schema)
        .bind(output_schema)
        .bind(FIXTURE_TIMESTAMP)
        .bind(FIXTURE_TIMESTAMP)
        .execute(pool)
        .await
        .expect("seed trusted Builtin fixture");
    }
}

async fn function_story_page(
    store: &FunctionStore,
    search: Option<&str>,
    page: i64,
) -> anyhow::Result<FunctionPage> {
    Ok(store.list(search.map(str::to_owned), page).await?)
}

fn is_public_invalid_input(
    error: &anyhow::Error,
    expected_field: &str,
    expected_reason: &str,
) -> bool {
    error
        .downcast_ref::<PublicBoundaryError>()
        .map(PublicBoundaryError::envelope)
        .is_some_and(|envelope| {
            matches!(
                envelope,
                PublicErrorEnvelope::InvalidInput { field, reason }
                    if field == expected_field && reason == expected_reason
            )
        })
}

fn is_public_invalid_field(error: &anyhow::Error, expected_field: &str) -> bool {
    error
        .downcast_ref::<PublicBoundaryError>()
        .map(PublicBoundaryError::envelope)
        .is_some_and(|envelope| {
            matches!(
                envelope,
                PublicErrorEnvelope::InvalidInput { field, reason }
                    if field == expected_field && !reason.trim().is_empty()
            )
        })
}

fn is_public_boundary_error(error: &anyhow::Error) -> bool {
    error.downcast_ref::<PublicBoundaryError>().is_some()
}

fn is_public_reference_conflict(
    error: &anyhow::Error,
    expected_reason: &str,
    expected_references: &[&str],
) -> bool {
    let Some(public_error) = error.downcast_ref::<PublicBoundaryError>() else {
        return false;
    };
    let envelope_matches = matches!(
        public_error.envelope(),
        PublicErrorEnvelope::Conflict {
            shape,
            field,
            reason,
        } if shape == "references" && field == "id" && reason == expected_reason
    );
    let references = public_error.references();
    let values_match = references
        .iter()
        .map(String::as_str)
        .eq(expected_references.iter().copied());
    let values_are_safe = references.iter().all(|reference| {
        !reference.is_empty()
            && reference
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_-".contains(character))
    });

    envelope_matches && values_match && values_are_safe
}

fn is_public_value_conflict(
    error: &anyhow::Error,
    expected_field: &str,
    expected_value: &str,
) -> bool {
    let Some(public_error) = error.downcast_ref::<PublicBoundaryError>() else {
        return false;
    };
    matches!(
        public_error.envelope(),
        PublicErrorEnvelope::Conflict {
            shape,
            field,
            reason,
        } if shape == "value" && field == expected_field && reason == "duplicate"
    ) && public_error.value() == Some(expected_value)
        && expected_value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_-".contains(character))
}

#[tokio::test(flavor = "current_thread")]
async fn function_store_create_is_durable_and_duplicate_is_stable_across_instances() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let store = open_v4_store(&workspace).await;
    let first = FunctionStore::new(store.pool().clone()).expect("first FunctionStore");
    let first_result = create_function(
        &first,
        FunctionDraft::placeholder("durable_placeholder", "Durable placeholder"),
    )
    .await;
    let visible_after_first: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM functions WHERE identifier = 'durable_placeholder'",
    )
    .fetch_one(store.pool())
    .await
    .expect("observe persisted Function row");

    drop(first);
    let reopened = FunctionStore::new(store.pool().clone()).expect("second FunctionStore");
    let readable_after_reopen = match first_result.as_ref() {
        Ok(id) => get_function(&reopened, *id).await,
        Err(_) => Ok(None),
    };
    let duplicate = create_function(
        &reopened,
        FunctionDraft::placeholder("durable_placeholder", "Duplicate placeholder"),
    )
    .await;
    let visible_after_duplicate: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM functions WHERE identifier = 'durable_placeholder'",
    )
    .fetch_one(store.pool())
    .await
    .expect("observe duplicate outcome");
    drop(reopened);
    let reopened_again = FunctionStore::new(store.pool().clone()).expect("third FunctionStore");
    let repeated_duplicate = create_function(
        &reopened_again,
        FunctionDraft::placeholder("durable_placeholder", "Repeated duplicate placeholder"),
    )
    .await;
    let visible_after_repeated_duplicate: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM functions WHERE identifier = 'durable_placeholder'",
    )
    .fetch_one(store.pool())
    .await
    .expect("observe repeated duplicate outcome");
    let duplicate_is_stable =
        duplicate.as_ref().err().is_some_and(|error| {
            is_public_value_conflict(error, "identifier", "durable_placeholder")
        }) && repeated_duplicate.as_ref().err().is_some_and(|error| {
            is_public_value_conflict(error, "identifier", "durable_placeholder")
        });

    assert!(
        first_result.is_ok()
            && visible_after_first == 1
            && readable_after_reopen
                .as_ref()
                .ok()
                .and_then(Option::as_ref)
                .is_some_and(|record| record.identifier() == "durable_placeholder")
            && duplicate_is_stable
            && visible_after_duplicate == 1
            && visible_after_repeated_duplicate == 1,
        "FunctionStore::create/get must share the supplied v4 pool and repeated duplicates across fresh instances must return the same safe public conflict/zero-modification; first={first_result:?}, visible_after_first={visible_after_first}, readable_after_reopen={readable_after_reopen:?}, duplicate={duplicate:?}, visible_after_duplicate={visible_after_duplicate}, repeated_duplicate={repeated_duplicate:?}, visible_after_repeated_duplicate={visible_after_repeated_duplicate}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn canonical_store_open_and_reopen_register_exactly_four_schema_correct_builtins() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let first_store = open_v4_store(&workspace).await;
    let first = function_snapshot(first_store.pool()).await;
    drop(first_store);

    let reopened_store = open_v4_store(&workspace).await;
    let second = function_snapshot(reopened_store.pool()).await;
    let expected = builtin_contracts();
    let reserved_identifiers = RESERVED_UNDERSCORE_IDENTIFIERS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let builtin_rows = second
        .iter()
        .filter(|row| row.kind == "builtin")
        .collect::<Vec<_>>();
    let identifiers = builtin_rows
        .iter()
        .map(|row| row.identifier.as_str())
        .collect::<BTreeSet<_>>();
    let dotted_count = second
        .iter()
        .filter(|row| row.identifier.contains('.'))
        .count();
    let schema_mismatches = builtin_rows
        .iter()
        .filter_map(|row| {
            let (expected_input, expected_output) = expected.get(row.identifier.as_str())?;
            let input_matches = serde_json::from_str::<serde_json::Value>(&row.input_schema).ok()
                == serde_json::from_str::<serde_json::Value>(expected_input).ok();
            let output_matches = serde_json::from_str::<serde_json::Value>(&row.output_schema).ok()
                == serde_json::from_str::<serde_json::Value>(expected_output).ok();
            (!input_matches
                || !output_matches
                || row.plugin_id.is_some()
                || row.plugin_export.is_some()
                || row.required_capabilities.is_some())
            .then_some(row.identifier.as_str())
        })
        .collect::<Vec<_>>();

    assert_eq!(reserved_identifiers, expected.keys().copied().collect());
    assert_eq!(
        first, second,
        "Builtin registration must be restart-idempotent"
    );
    assert_eq!(
        builtin_rows.len(),
        4,
        "Store must register exactly four Builtins"
    );
    assert_eq!(identifiers, expected.keys().copied().collect());
    assert_eq!(
        dotted_count, 0,
        "dotted Builtin records/aliases are forbidden"
    );
    assert!(
        schema_mismatches.is_empty(),
        "Builtin schemas/relations drifted: {schema_mismatches:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn v2_kind_and_dotted_migration_is_smoked_through_public_store_open() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let fixture = Path::new(MIGRATION_FIXTURE_DIRECTORY).join("v2-valid.sqlite");
    fs::copy(&fixture, workspace.database_path())
        .unwrap_or_else(|error| panic!("copy migration fixture {}: {error}", fixture.display()));

    let store = open_v4_store(&workspace).await;
    let rows = function_snapshot(store.pool()).await;
    let kinds = rows
        .iter()
        .map(|row| row.kind.as_str())
        .collect::<BTreeSet<_>>();
    let identifiers = rows
        .iter()
        .map(|row| row.identifier.as_str())
        .collect::<BTreeSet<_>>();

    assert_eq!(kinds, BTreeSet::from(["builtin", "custom", "placeholder"]));
    assert!(rows.iter().all(|row| !row.identifier.contains('.')));
    assert!(
        RESERVED_UNDERSCORE_IDENTIFIERS
            .iter()
            .all(|identifier| identifiers.contains(identifier)),
        "migration_compatibility owns the exhaustive matrix; this public-open smoke proves its migrated rows remain readable"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn builtin_mutations_and_reserved_user_kinds_are_rejected_with_zero_modification() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let store = open_v4_store(&workspace).await;
    let function_store = FunctionStore::new(store.pool().clone()).expect("Function Store");
    seed_known_network_http_capability(store.pool()).await;
    let plugin_id = seed_plugin(store.pool(), "builtin_relation_plugin", "handle").await;
    seed_canonical_builtins(store.pool()).await;
    let before = function_snapshot(store.pool()).await;
    let by_identifier = before
        .iter()
        .map(|row| (row.identifier.as_str(), row.id))
        .collect::<BTreeMap<_, _>>();

    let mut builtin_update = FunctionDraft::placeholder("format_template", "mutated name");
    builtin_update.kind = FunctionKind::Builtin;
    builtin_update.input_schema = "{}".to_string();
    builtin_update.output_schema = "{}".to_string();
    let mut builtin_rename = FunctionDraft::placeholder("json_parse_renamed", "json_parse");
    builtin_rename.kind = FunctionKind::Builtin;
    builtin_rename.input_schema = "{}".to_string();
    builtin_rename.output_schema = "{}".to_string();
    let mut builtin_relation =
        FunctionDraft::custom("text_regex_match", "text_regex_match", plugin_id, "handle");
    builtin_relation.kind = FunctionKind::Builtin;
    builtin_relation.input_schema = "{}".to_string();
    builtin_relation.output_schema = "{}".to_string();
    builtin_relation.required_capabilities = Some(r#"["network.http"]"#.to_string());
    let mut user_builtin = FunctionDraft::placeholder("user_builtin", "User Builtin");
    user_builtin.kind = FunctionKind::Builtin;

    let attempts = [
        (
            "Builtin update",
            update_function(
                &function_store,
                by_identifier["format_template"],
                builtin_update,
            )
            .await
            .map(|_| ()),
        ),
        (
            "Builtin rename",
            update_function(&function_store, by_identifier["json_parse"], builtin_rename)
                .await
                .map(|_| ()),
        ),
        (
            "Builtin delete",
            delete_function(&function_store, by_identifier["json_stringify"]).await,
        ),
        (
            "Builtin executable relation mutation",
            update_function(
                &function_store,
                by_identifier["text_regex_match"],
                builtin_relation,
            )
            .await
            .map(|_| ()),
        ),
        (
            "user-created Builtin",
            create_function(&function_store, user_builtin)
                .await
                .map(|_| ()),
        ),
        (
            "Custom using freed reserved identifier",
            create_function(
                &function_store,
                FunctionDraft::custom(
                    "json_parse".to_string(),
                    "Reserved Custom".to_string(),
                    plugin_id,
                    "handle",
                ),
            )
            .await
            .map(|_| ()),
        ),
        (
            "Placeholder using freed reserved identifier",
            create_function(
                &function_store,
                FunctionDraft::placeholder("json_stringify", "Reserved Placeholder"),
            )
            .await
            .map(|_| ()),
        ),
    ];
    let accepted = attempts
        .iter()
        .filter_map(|(label, result)| result.is_ok().then_some(*label))
        .collect::<Vec<_>>();
    let untyped = attempts
        .iter()
        .filter_map(|(label, result)| {
            result
                .as_ref()
                .err()
                .filter(|error| !is_public_boundary_error(error))
                .map(|_| *label)
        })
        .collect::<Vec<_>>();
    let after = function_snapshot(store.pool()).await;

    assert!(
        accepted.is_empty() && untyped.is_empty() && after == before,
        "Builtin/reserved mutations must return the shared public error type and fail atomically; accepted={accepted:?}, untyped={untyped:?}, before={before:?}, after={after:?}"
    );
}

// ---------------------------------------------------------------------------
// 2026-08-20 supplemental T083 Red: continue exercising the same SQL-backed
// FunctionStore boundary against production-v4 relationship and validation fixtures.
// The migration-owned exhaustive legacy matrix intentionally is not duplicated here.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn custom_function_roundtrips_live_plugin_export_and_both_schemas() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let store = open_v4_store(&workspace).await;
    let function_store = FunctionStore::new(store.pool().clone()).expect("Function Store");
    seed_known_network_http_capability(store.pool()).await;
    let plugin_id = seed_plugin(store.pool(), "custom_contract_plugin", "handle").await;
    let category = EntityCategory::create(
        store.pool(),
        None,
        "Function contracts".to_string(),
        "function-contracts".to_string(),
        Some("Custom Function fixture".to_string()),
    )
    .await
    .expect("create Function Category fixture");
    let input_schema = r#"{"type":"object","required":["query"]}"#;
    let output_schema = r#"{"type":"object","required":["answer"]}"#;
    let capabilities = r#"[" time.now ","network.http"]"#;
    let canonical_capabilities = r#"["time.now","network.http"]"#;
    let mut draft = FunctionDraft::custom(
        "custom_contract_function",
        "Custom contract function",
        plugin_id,
        " handle ",
    );
    draft.description = Some("round-trips every executable relation".to_string());
    draft.input_schema = input_schema.to_string();
    draft.output_schema = output_schema.to_string();
    draft.category_id = Some(category.id);
    draft.required_capabilities = Some(capabilities.to_string());

    let created_id = create_function(&function_store, draft)
        .await
        .expect("create valid Custom Function");
    let reopened = function_snapshot(store.pool())
        .await
        .into_iter()
        .find(|row| row.id == created_id)
        .expect("Custom Function exists");
    assert_eq!(reopened.kind, "custom");
    assert_eq!(reopened.plugin_id, Some(plugin_id));
    assert_eq!(reopened.plugin_export.as_deref(), Some("handle"));
    assert_eq!(reopened.category_id, Some(category.id));
    assert_eq!(reopened.input_schema, input_schema);
    assert_eq!(reopened.output_schema, output_schema);
    assert_eq!(
        reopened.required_capabilities.as_deref(),
        Some(canonical_capabilities),
        "unique known capabilities must be trimmed while preserving their canonical input order; they need not equal the Plugin manifest set"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn custom_function_trims_and_accepts_a_255_byte_declared_export() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let store = open_v4_store(&workspace).await;
    let function_store = FunctionStore::new(store.pool().clone()).expect("Function Store");
    seed_known_network_http_capability(store.pool()).await;
    let max_export = "e".repeat(255);
    let plugin_id = seed_plugin(store.pool(), "max_export_plugin", &max_export).await;

    let created_id = create_function(
        &function_store,
        FunctionDraft::custom(
            "max_export_function",
            "Maximum export Function",
            plugin_id,
            format!(" {max_export} "),
        ),
    )
    .await
    .expect("trimmed 255-byte declared export is valid");
    let created = function_snapshot(store.pool())
        .await
        .into_iter()
        .find(|row| row.id == created_id)
        .expect("maximum export Function exists");

    assert_eq!(created.plugin_export.as_deref(), Some(max_export.as_str()));
    assert_eq!(created.plugin_export.as_deref().map(str::len), Some(255));
}

#[tokio::test(flavor = "current_thread")]
async fn custom_function_prevalidates_plugin_and_declared_export_with_zero_modification() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let store = open_v4_store(&workspace).await;
    let function_store = FunctionStore::new(store.pool().clone()).expect("Function Store");
    seed_known_network_http_capability(store.pool()).await;
    let live_plugin_id = seed_plugin(store.pool(), "live_function_plugin", "handle").await;
    let deleted_plugin_id = seed_plugin(store.pool(), "deleted_function_plugin", "handle").await;
    sqlx::query("UPDATE plugins SET deleted_at = ? WHERE id = ?")
        .bind(FIXTURE_TIMESTAMP)
        .bind(deleted_plugin_id)
        .execute(store.pool())
        .await
        .expect("soft-delete Plugin fixture");
    let before = function_snapshot(store.pool()).await;
    let mut missing_plugin = FunctionDraft::custom(
        "custom_missing_plugin",
        "Missing plugin",
        live_plugin_id,
        "handle",
    );
    missing_plugin.plugin_id = None;
    let mut missing_export = FunctionDraft::custom(
        "custom_missing_export",
        "Missing export",
        live_plugin_id,
        "handle",
    );
    missing_export.plugin_export = None;
    let cases = [
        ("missing plugin_id", "plugin_id", missing_plugin),
        ("missing plugin_export", "plugin_export", missing_export),
        (
            "empty plugin_export",
            "plugin_export",
            FunctionDraft::custom("custom_empty_export", "Empty export", live_plugin_id, ""),
        ),
        (
            "whitespace plugin_export",
            "plugin_export",
            FunctionDraft::custom(
                "custom_whitespace_export",
                "Whitespace export",
                live_plugin_id,
                "   ",
            ),
        ),
        (
            "plugin_export longer than 255 bytes",
            "plugin_export",
            FunctionDraft::custom(
                "custom_long_export",
                "Long export",
                live_plugin_id,
                "e".repeat(256),
            ),
        ),
        (
            "export absent from manifest",
            "plugin_export",
            FunctionDraft::custom(
                "custom_unknown_export",
                "Unknown export",
                live_plugin_id,
                "not_declared",
            ),
        ),
        (
            "missing Plugin row",
            "plugin_id",
            FunctionDraft::custom(
                "custom_unknown_plugin",
                "Unknown Plugin",
                i64::MAX,
                "handle",
            ),
        ),
        (
            "soft-deleted plugin",
            "plugin_id",
            FunctionDraft::custom(
                "custom_deleted_plugin",
                "Deleted plugin",
                deleted_plugin_id,
                "handle",
            ),
        ),
    ];
    let mut accepted = Vec::new();
    let mut untyped = Vec::new();
    for (label, expected_field, draft) in cases {
        match create_function(&function_store, draft).await {
            Ok(_) => accepted.push(label),
            Err(error) if !is_public_invalid_field(&error, expected_field) => {
                untyped.push((label, error));
            }
            Err(_) => {}
        }
    }
    let after = function_snapshot(store.pool()).await;

    assert!(
        accepted.is_empty() && untyped.is_empty() && after == before,
        "invalid Custom Function inputs must return invalid_input for the failing field before any row is written; accepted={accepted:?}, untyped={untyped:?}, before={before:?}, after={after:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn function_write_matrix_rejects_kind_schema_size_category_and_capability_errors_atomically()
{
    let workspace = TestWorkspace::new().expect("test workspace");
    let store = open_v4_store(&workspace).await;
    let function_store = FunctionStore::new(store.pool().clone()).expect("Function Store");
    seed_known_network_http_capability(store.pool()).await;
    let plugin_id = seed_plugin(store.pool(), "write_matrix_plugin", "handle").await;
    let oversized_schema = format!(r#"{{"description":"{}"}}"#, "x".repeat(MAX_SCHEMA_BYTES));
    let before = function_snapshot(store.pool()).await;

    let mut input_shape = FunctionDraft::placeholder("matrix_input_shape", "Input shape");
    input_shape.input_schema = "[]".to_string();
    let mut output_shape = FunctionDraft::placeholder("matrix_output_shape", "Output shape");
    output_shape.output_schema = "[]".to_string();
    let mut large_input = FunctionDraft::placeholder("matrix_large_input", "Large input schema");
    large_input.input_schema = oversized_schema.clone();
    let mut large_output = FunctionDraft::placeholder("matrix_large_output", "Large output schema");
    large_output.output_schema = oversized_schema;
    let mut missing_category =
        FunctionDraft::placeholder("matrix_missing_category", "Missing category");
    missing_category.category_id = Some(i64::MAX);
    let mut capability_json = FunctionDraft::custom(
        "matrix_capability_json",
        "Capability JSON",
        plugin_id,
        "handle",
    );
    capability_json.required_capabilities = Some("not-json".to_string());
    let mut capability_shape = FunctionDraft::custom(
        "matrix_capability_shape",
        "Capability shape",
        plugin_id,
        "handle",
    );
    capability_shape.required_capabilities = Some("{}".to_string());
    let mut capability_empty = FunctionDraft::custom(
        "matrix_capability_empty",
        "Capability empty",
        plugin_id,
        "handle",
    );
    capability_empty.required_capabilities = Some(r#"["   "]"#.to_string());
    let mut capability_unknown = FunctionDraft::custom(
        "matrix_capability_unknown",
        "Capability unknown",
        plugin_id,
        "handle",
    );
    capability_unknown.required_capabilities = Some(r#"["unknown.capability"]"#.to_string());
    let mut capability_duplicate = FunctionDraft::custom(
        "matrix_capability_duplicate",
        "Capability duplicate",
        plugin_id,
        "handle",
    );
    capability_duplicate.required_capabilities =
        Some(r#"[" network.http ","network.http"]"#.to_string());

    let mut accepted = Vec::new();
    let unknown_result = match parse_function_kind("unknown") {
        Ok(kind) => {
            let mut draft = FunctionDraft::placeholder("matrix_unknown_kind", "Unknown kind");
            draft.kind = kind;
            create_function(&function_store, draft).await
        }
        Err(error) => Err(error),
    };
    if unknown_result.is_ok() {
        accepted.push("unknown kind");
    }
    let unknown_is_public_invalid = unknown_result
        .as_ref()
        .err()
        .is_some_and(|error| is_public_invalid_field(error, "kind"));
    let duplicate_result = create_function(&function_store, capability_duplicate).await;
    let duplicate_is_public_invalid = duplicate_result
        .as_ref()
        .err()
        .is_some_and(|error| is_public_invalid_input(error, "required_capabilities", "duplicate"));
    if duplicate_result.is_ok() {
        accepted.push("required_capabilities duplicates after trim");
    }
    let mut untyped = Vec::new();
    for (label, expected_field, draft) in [
        (
            "dotted identifier",
            "identifier",
            FunctionDraft::placeholder("format.template", "Dotted identifier"),
        ),
        ("input_schema is not an object", "input_schema", input_shape),
        (
            "output_schema is not an object",
            "output_schema",
            output_shape,
        ),
        ("input_schema exceeds 1 MiB", "input_schema", large_input),
        ("output_schema exceeds 1 MiB", "output_schema", large_output),
        ("missing category FK", "category_id", missing_category),
        (
            "required_capabilities invalid JSON",
            "required_capabilities",
            capability_json,
        ),
        (
            "required_capabilities not an array",
            "required_capabilities",
            capability_shape,
        ),
        (
            "required_capabilities contains empty name",
            "required_capabilities",
            capability_empty,
        ),
        (
            "required_capabilities contains unknown name",
            "required_capabilities",
            capability_unknown,
        ),
    ] {
        match create_function(&function_store, draft).await {
            Ok(_) => accepted.push(label),
            Err(error) if !is_public_invalid_field(&error, expected_field) => {
                untyped.push((label, error));
            }
            Err(_) => {}
        }
    }
    let after = function_snapshot(store.pool()).await;

    assert!(
        accepted.is_empty()
            && untyped.is_empty()
            && unknown_is_public_invalid
            && duplicate_is_public_invalid
            && after == before,
        "invalid Function writes must return field-specific public errors before modification and trim-duplicate Capability must be public invalid_input {{ field: required_capabilities, reason: duplicate }}; accepted={accepted:?}, untyped={untyped:?}, unknown_result={unknown_result:?}, duplicate_result={duplicate_result:?}, before_count={}, after_count={}",
        before.len(),
        after.len()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn function_schema_accepts_exactly_one_mib_and_rejects_one_byte_more_atomically() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let store = open_v4_store(&workspace).await;
    let function_store = FunctionStore::new(store.pool().clone()).expect("Function Store");
    let at_limit = object_schema_with_exact_size(MAX_SCHEMA_BYTES);
    let over_limit = object_schema_with_exact_size(MAX_SCHEMA_BYTES + 1);

    let mut at_limit_draft =
        FunctionDraft::placeholder("schema_size_boundary", "Schema size boundary");
    at_limit_draft.input_schema = at_limit.clone();
    at_limit_draft.output_schema = at_limit.clone();
    let created_id = create_function(&function_store, at_limit_draft)
        .await
        .expect("a structurally valid schema of exactly 1 MiB is accepted");

    let mut over_limit_draft =
        FunctionDraft::placeholder("schema_size_boundary", "Schema size boundary");
    over_limit_draft.input_schema = over_limit;
    over_limit_draft.output_schema = at_limit.clone();
    let rejected = update_function(&function_store, created_id, over_limit_draft).await;
    let after = function_snapshot(store.pool())
        .await
        .into_iter()
        .find(|row| row.id == created_id)
        .expect("Function remains");

    assert!(
        rejected
            .as_ref()
            .err()
            .is_some_and(|error| is_public_invalid_field(error, "input_schema")),
        "1 MiB + 1 byte must return invalid_input for input_schema: {rejected:?}"
    );
    assert_eq!(after.input_schema.len(), MAX_SCHEMA_BYTES);
    assert_eq!(after.output_schema.len(), MAX_SCHEMA_BYTES);
    assert_eq!(after.input_schema, at_limit);
}

#[tokio::test(flavor = "current_thread")]
async fn placeholder_create_clears_all_three_executable_relationship_fields() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let store = open_v4_store(&workspace).await;
    let function_store = FunctionStore::new(store.pool().clone()).expect("Function Store");
    seed_known_network_http_capability(store.pool()).await;
    let plugin_id = seed_plugin(store.pool(), "placeholder_relation_plugin", "handle").await;
    let mut with_plugin =
        FunctionDraft::placeholder("placeholder_with_plugin", "Placeholder with Plugin");
    with_plugin.plugin_id = Some(plugin_id);
    let mut with_export =
        FunctionDraft::placeholder("placeholder_with_export", "Placeholder with export");
    with_export.plugin_export = Some("handle".to_string());
    let mut with_capability =
        FunctionDraft::placeholder("placeholder_with_capability", "Placeholder with capability");
    with_capability.required_capabilities = Some(r#"["network.http"]"#.to_string());
    let mut created_ids = BTreeSet::new();
    for draft in [with_plugin, with_export, with_capability] {
        created_ids.insert(
            create_function(&function_store, draft)
                .await
                .expect("Placeholder save clears hidden executable relationships"),
        );
    }
    let created = function_snapshot(store.pool())
        .await
        .into_iter()
        .filter(|row| created_ids.contains(&row.id))
        .collect::<Vec<_>>();

    assert_eq!(created.len(), 3);
    assert!(
        created.iter().all(|row| {
            row.kind == "placeholder"
                && row.plugin_id.is_none()
                && row.plugin_export.is_none()
                && row.required_capabilities.is_none()
        }),
        "Placeholder save must clear plugin_id, plugin_export and required_capabilities after validating the known Capability fixture: {created:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn placeholder_update_and_custom_conversion_clear_executable_relationships() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let store = open_v4_store(&workspace).await;
    let function_store = FunctionStore::new(store.pool().clone()).expect("Function Store");
    seed_known_network_http_capability(store.pool()).await;
    let plugin_id = seed_plugin(store.pool(), "placeholder_update_plugin", "handle").await;

    let placeholder_id = create_function(
        &function_store,
        FunctionDraft::placeholder("placeholder_update_contract", "Placeholder update contract"),
    )
    .await
    .expect("create Placeholder fixture");
    let mut placeholder_with_hidden_relations =
        FunctionDraft::placeholder("placeholder_update_contract", "Placeholder update contract");
    placeholder_with_hidden_relations.plugin_id = Some(plugin_id);
    placeholder_with_hidden_relations.plugin_export = Some("handle".to_string());
    placeholder_with_hidden_relations.required_capabilities =
        Some(r#"["network.http"]"#.to_string());
    let updated_placeholder_id = update_function(
        &function_store,
        placeholder_id,
        placeholder_with_hidden_relations,
    )
    .await
    .expect("Placeholder update clears hidden executable relationships");
    let updated_placeholder = function_snapshot(store.pool())
        .await
        .into_iter()
        .find(|row| row.id == placeholder_id)
        .expect("updated Placeholder remains");

    let mut custom = FunctionDraft::custom(
        "custom_to_placeholder",
        "Custom to Placeholder",
        plugin_id,
        "handle",
    );
    custom.required_capabilities = Some(r#"["network.http"]"#.to_string());
    let custom_id = create_function(&function_store, custom)
        .await
        .expect("create valid Custom fixture");
    let mut conversion =
        FunctionDraft::placeholder("custom_to_placeholder", "Custom to Placeholder");
    conversion.plugin_id = Some(plugin_id);
    conversion.plugin_export = Some("handle".to_string());
    conversion.required_capabilities = Some(r#"["network.http"]"#.to_string());
    update_function(&function_store, custom_id, conversion)
        .await
        .expect("Custom→Placeholder conversion succeeds");
    let converted = function_snapshot(store.pool())
        .await
        .into_iter()
        .find(|row| row.id == custom_id)
        .expect("converted Function remains");

    assert_eq!(updated_placeholder_id, placeholder_id);
    assert_eq!(updated_placeholder.kind, "placeholder");
    assert_eq!(updated_placeholder.plugin_id, None);
    assert_eq!(updated_placeholder.plugin_export, None);
    assert_eq!(updated_placeholder.required_capabilities, None);
    assert_eq!(converted.kind, "placeholder");
    assert_eq!(converted.plugin_id, None);
    assert_eq!(converted.plugin_export, None);
    assert_eq!(converted.required_capabilities, None);
}

#[tokio::test(flavor = "current_thread")]
async fn workflow_node_reference_restricts_function_delete_without_nulling_relation() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let store = open_v4_store(&workspace).await;
    let function_store = FunctionStore::new(store.pool().clone()).expect("Function Store");
    let function = seed_placeholder(
        store.pool(),
        "workflow_restricted_function",
        "Workflow restricted function",
    )
    .await;
    let workflow = EntityWorkflow::create(
        store.pool(),
        "function_restrict_workflow".to_string(),
        "Function RESTRICT workflow".to_string(),
        None,
        30_000,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .expect("create Workflow fixture");
    EntityWorkflowNode::upsert(
        store.pool(),
        workflow.id,
        "run_function".to_string(),
        "function_node".to_string(),
        Some(function.id()),
        0.0,
        0.0,
        Some("{}".to_string()),
    )
    .await
    .expect("create WorkflowNode reference");

    let functions_before = function_snapshot(store.pool()).await;
    let nodes_before = workflow_node_snapshot(store.pool(), workflow.id).await;
    let deletion = delete_function(&function_store, function.id()).await;
    let public_conflict = deletion.as_ref().err().is_some_and(|error| {
        is_public_reference_conflict(
            error,
            "referenced_by_workflow_node",
            &["function_restrict_workflow"],
        )
    });
    let functions_after = function_snapshot(store.pool()).await;
    let nodes_after = workflow_node_snapshot(store.pool(), workflow.id).await;

    assert!(
        public_conflict && functions_after == functions_before && nodes_after == nodes_before,
        "WorkflowNode→Function must return public conflict {{ shape: references, field: id, reason: referenced_by_workflow_node, references: [function_restrict_workflow] }} with the safe referencing Workflow identifier and a full zero-modification snapshot; delete={deletion:?}, functions_before={functions_before:?}, functions_after={functions_after:?}, nodes_before={nodes_before:?}, nodes_after={nodes_after:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn tool_reference_restricts_function_delete_with_zero_modification() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let store = open_v4_store(&workspace).await;
    let function_store = FunctionStore::new(store.pool().clone()).expect("Function Store");
    let function = seed_placeholder(
        store.pool(),
        "tool_restricted_function",
        "Tool restricted function",
    )
    .await;
    let tool = EntityTool::create(
        store.pool(),
        "function_restrict_tool".to_string(),
        "Function RESTRICT Tool".to_string(),
        "references one Function".to_string(),
        "function-wrap".to_string(),
        "workspace".to_string(),
        false,
        Some(function.id()),
        None,
        "{}".to_string(),
        "{}".to_string(),
        None,
        None,
    )
    .await
    .expect("create Tool reference");

    let functions_before = function_snapshot(store.pool()).await;
    let tool_before = tool_snapshot(store.pool(), tool.id)
        .await
        .expect("Tool snapshot exists before delete");
    let deletion = delete_function(&function_store, function.id()).await;
    let public_conflict = deletion.as_ref().err().is_some_and(|error| {
        is_public_reference_conflict(error, "referenced_by_tool", &["function_restrict_tool"])
    });
    let functions_after = function_snapshot(store.pool()).await;
    let tool_after = tool_snapshot(store.pool(), tool.id)
        .await
        .expect("Tool snapshot exists after rejected delete");

    assert!(
        public_conflict && functions_after == functions_before && tool_after == tool_before,
        "Tool→Function must return public conflict {{ shape: references, field: id, reason: referenced_by_tool, references: [function_restrict_tool] }} with a full zero-modification snapshot; delete={deletion:?}, functions_before={functions_before:?}, functions_after={functions_after:?}, tool_before={tool_before:?}, tool_after={tool_after:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn function_search_paginates_in_exact_twenty_row_pages_without_overlap() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let store = open_v4_store(&workspace).await;
    let function_store = FunctionStore::new(store.pool().clone()).expect("Function story store");
    for row in 0..41 {
        create_function(
            &function_store,
            FunctionDraft::placeholder(
                format!("page_marker_{row:02}"),
                format!("Page marker {row:02}"),
            ),
        )
        .await
        .expect("create indexed pagination fixture through FunctionStore");
    }

    let mut seen = BTreeSet::new();
    let mut identifiers_in_order = Vec::new();
    for (page_number, expected_len) in [(1, PAGE_SIZE), (2, PAGE_SIZE), (3, 1)] {
        let page = function_story_page(&function_store, Some("page_marker"), page_number)
            .await
            .expect("load Function search page");
        assert_eq!(page.page(), page_number, "story page is 1-based");
        assert_eq!(page.page_size(), PAGE_SIZE as i64);
        assert_eq!(page.total(), 41);
        assert_eq!(page.items().len(), expected_len, "page={page_number}");
        for function in page.items() {
            assert!(seen.insert(function.id()), "pages must not overlap");
            identifiers_in_order.push(function.identifier().to_string());
        }
    }
    assert_eq!(seen.len(), 41);
    assert_eq!(
        identifiers_in_order,
        (0..41)
            .map(|row| format!("page_marker_{row:02}"))
            .collect::<Vec<_>>(),
        "fixed pages preserve the normalized name/identifier/id total order"
    );

    for invalid_page in [0, -1] {
        let error = function_story_page(&function_store, Some("page_marker"), invalid_page)
            .await
            .expect_err("page below 1 must be rejected");
        assert!(
            is_public_invalid_input(&error, "page", "out_of_range"),
            "page below 1 must return public invalid_input {{ field: page, reason: out_of_range }}; got {error:?}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn function_search_covers_nfkc_short_gram_literal_operator_and_dedup_matrix() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let store = open_v4_store(&workspace).await;
    let function_store = FunctionStore::new(store.pool().clone()).expect("Function story store");
    let percent =
        create_placeholder_function(&function_store, "literal-percent", "Literal 100% marker")
            .await;
    let underscore = create_placeholder_function(
        &function_store,
        "literal-underscore",
        "Literal under_score marker",
    )
    .await;
    let quoted = create_placeholder_function(
        &function_store,
        "literal-quote",
        "Literal \"quoted\" marker",
    )
    .await;
    let fts_or = create_placeholder_function(
        &function_store,
        "literal-fts-or",
        "Literal alpha OR beta marker",
    )
    .await;
    let fts_star =
        create_placeholder_function(&function_store, "literal-fts-star", "Literal token* marker")
            .await;
    let fts_colon =
        create_placeholder_function(&function_store, "literal-fts-colon", "Literal a:b marker")
            .await;
    let nfkc = create_placeholder_function(&function_store, "nfkc-casefold", "Straße 模型Ａ").await;
    let one_scalar = create_placeholder_function(&function_store, "short-one", "一字符").await;
    let two_scalars =
        create_placeholder_function(&function_store, "short-two", "中文 fixture").await;
    let trigram =
        create_placeholder_function(&function_store, "trigram-needle", "needle fixture").await;
    let dedup =
        create_placeholder_function(&function_store, "dedup-token", "dedup token in name").await;
    create_placeholder_function(&function_store, "literal-plain", "Literal plain marker").await;

    let mut underscore_matches =
        sqlx::query_scalar::<_, i64>("SELECT id FROM functions WHERE kind = 'builtin' ORDER BY id")
            .fetch_all(store.pool())
            .await
            .expect("load registry-owned Builtins that literally contain underscore")
            .into_iter()
            .collect::<BTreeSet<_>>();
    underscore_matches.insert(underscore);

    let cases = [
        ("%", BTreeSet::from([percent])),
        ("_", underscore_matches),
        ("\"quoted\"", BTreeSet::from([quoted])),
        ("alpha OR beta", BTreeSet::from([fts_or])),
        ("token*", BTreeSet::from([fts_star])),
        ("a:b", BTreeSet::from([fts_colon])),
        ("strasse", BTreeSet::from([nfkc])),
        ("模型a", BTreeSet::from([nfkc])),
        ("一", BTreeSet::from([one_scalar])),
        ("中文", BTreeSet::from([two_scalars])),
        ("needle", BTreeSet::from([trigram])),
        ("DEDUP", BTreeSet::from([dedup])),
    ];
    let mut mismatches = Vec::new();
    for (query, expected) in cases {
        let observed = function_story_page(&function_store, Some(query), 1)
            .await
            .unwrap_or_else(|error| panic!("search {query:?}: {error}"))
            .items()
            .iter()
            .map(|function| function.id())
            .collect::<BTreeSet<_>>();
        if observed != expected {
            mismatches.push(format!(
                "{query:?}: expected={expected:?}, observed={observed:?}"
            ));
        }
    }

    assert!(
        mismatches.is_empty(),
        "Function search must use NFKC_CF, 1/2-char short grams, 3+ trigram, literal operator semantics and multi-field dedup:\n{}",
        mismatches.join("\n")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn function_search_has_stable_total_order_and_crud_is_immediately_visible() {
    let workspace = TestWorkspace::new().expect("test workspace");
    let store = open_v4_store(&workspace).await;
    let function_store = FunctionStore::new(store.pool().clone()).expect("Function story store");
    let zeta =
        create_placeholder_function(&function_store, "zeta_order", "Same normalized name").await;
    let alpha =
        create_placeholder_function(&function_store, "alpha_order", "Same normalized name").await;

    let ordered = function_story_page(&function_store, Some("same normalized"), 1)
        .await
        .expect("stable ordered Function search")
        .items()
        .iter()
        .map(|function| function.id())
        .collect::<Vec<_>>();

    let created = create_placeholder_function(
        &function_store,
        "search_lifecycle_old",
        "Lifecycle alpha marker",
    )
    .await;
    let visible_after_create = function_story_page(&function_store, Some("Lifecycle alpha"), 1)
        .await
        .expect("search after create")
        .items()
        .iter()
        .map(|row| row.id())
        .collect::<Vec<_>>();
    let updated = update_function(
        &function_store,
        created,
        FunctionDraft::placeholder("search_lifecycle_new", "Lifecycle beta marker"),
    )
    .await
    .expect("update searchable Function");
    let old_after_update = function_story_page(&function_store, Some("Lifecycle alpha"), 1)
        .await
        .expect("old search after update");
    let new_after_update = function_story_page(&function_store, Some("Lifecycle beta"), 1)
        .await
        .expect("new search after update")
        .items()
        .iter()
        .map(|row| row.id())
        .collect::<Vec<_>>();
    delete_function(&function_store, updated)
        .await
        .expect("delete searchable Function");
    let after_delete = function_story_page(&function_store, Some("Lifecycle beta"), 1)
        .await
        .expect("search after delete");

    assert_eq!(
        ordered,
        vec![alpha, zeta],
        "total order is normalized display name, normalized identifier, numeric id"
    );
    assert_eq!(visible_after_create, vec![created]);
    assert!(old_after_update.items().is_empty());
    assert_eq!(new_after_update, vec![updated]);
    assert!(after_delete.items().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn function_query_catalog_owns_every_story_filter_and_association_route() {
    let owned_catalog_rows = production_query_catalog()
        .iter()
        .filter(|query| {
            query.owner_phase == "US9"
                && query.activation_task == "T083"
                && query.active
                && query.dialect == QueryDialect::Sqlite
        })
        .collect::<Vec<_>>();
    let requirement_tables = owned_catalog_rows
        .iter()
        .flat_map(|query| {
            query
                .requirements
                .iter()
                .map(|requirement| requirement.table)
        })
        .collect::<BTreeSet<_>>();
    let required_tables = BTreeSet::from([
        "search_documents_fts",
        "search_short_grams",
        "search_documents",
        "functions",
        "workflow_nodes",
        "tools",
    ]);
    assert!(
        !owned_catalog_rows.is_empty()
            && owned_catalog_rows
                .iter()
                .all(|query| !query.requirements.is_empty())
            && required_tables.is_subset(&requirement_tables),
        "active US9/T083 catalog is incomplete: required={required_tables:?}, observed={requirement_tables:?}; actual production SQL EXPLAIN is centralized in storage_query_plans and must not be synthesized here"
    );
}

#[test]
fn function_performance_uses_exactly_two_canonical_t005_targets() {
    let function_targets = target_specs()
        .into_iter()
        .filter(|target| target.owner_task == "T083")
        .collect::<Vec<_>>();
    let target_ids = function_targets
        .iter()
        .map(|target| target.id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        target_ids,
        BTreeSet::from([FUNCTION_CRUD_ID, FUNCTION_SEARCH_PAGE_ID]),
        "T005 owns exactly the Function CRUD and combined search/page targets"
    );
    assert_eq!(
        function_targets.len(),
        2,
        "Function performance targets must not be duplicated"
    );

    let crud = function_targets
        .iter()
        .find(|target| target.id == FUNCTION_CRUD_ID)
        .expect("canonical Function CRUD target");
    assert_eq!(
        crud.timing_boundary,
        "Function CRUD lifecycle accepted through validation, create/get/update/delete transactions, and final absence materialized"
    );
    assert_eq!(crud.p95_budget_ns, 1_000_000_000);

    let search_page = function_targets
        .iter()
        .find(|target| target.id == FUNCTION_SEARCH_PAGE_ID)
        .expect("canonical combined Function search/page target");
    assert_eq!(
        search_page.timing_boundary,
        "Function search/page request accepted through normalization and routing to one fixed 20-row page and total-order metadata materialized"
    );
    assert_eq!(search_page.p95_budget_ns, 500_000_000);
    assert_eq!(
        FUNCTION_FIXTURE_ROWS, 10_000,
        "the canonical runner must load the versioned Function fixture through the typed store boundary"
    );
    assert_eq!(
        FUNCTION_SEARCH_PAGE_SCHEDULE.len(),
        MEASURED_SAMPLES,
        "each sample must follow the canonical search + unfiltered-page schedule"
    );

    assert!(function_targets.iter().all(|target| {
        target.warmup_iterations == 10
            && target.measured_samples == MEASURED_SAMPLES
            && target.workflow_node_count.is_none()
            && target
                .excluded_time
                .iter()
                .any(|value| value == "external I/O")
            && target
                .excluded_time
                .iter()
                .any(|value| value == "Plugin/WASM execution")
    }));
}

#[cfg_attr(
    debug_assertions,
    ignore = "release-only T005 benchmark gate; debug has a distinct environment fingerprint"
)]
#[tokio::test(flavor = "current_thread")]
async fn function_performance_runner_meets_budgets_and_approved_baselines() {
    let current_source_revision = source_revision(Path::new(env!("CARGO_MANIFEST_DIR")))
        .expect("capture repository-anchored benchmark source revision");
    let as_of = Utc::now().date_naive();
    let environment = EnvironmentFingerprint::capture();
    for target in target_specs()
        .into_iter()
        .filter(|target| target.id == FUNCTION_CRUD_ID || target.id == FUNCTION_SEARCH_PAGE_ID)
    {
        let report = run_target(&target, &environment)
            .await
            .unwrap_or_else(|error| panic!("run canonical {} benchmark: {error}", target.id))
            .unwrap_or_else(|| {
                panic!(
                    "canonical T005 runner must own the {} operation and load its fixed typed-Store fixture",
                    target.id
                )
        });
        assert_eq!(report.target, target);
        assert_eq!(report.environment, environment);
        assert_eq!(report.sample_count, MEASURED_SAMPLES);

        let baseline_path = baseline_path(&target, &environment);
        let exception_path = regression_exception_path(&target, &environment);
        let evaluation = evaluate_benchmark_gate(
            &report,
            &current_source_revision,
            &baseline_path,
            &exception_path,
            as_of,
        )
        .unwrap_or_else(|error| {
            panic!(
                "{} performance gate evaluation failed: baseline_path={}, exception_path={}, evaluation=Err({error})",
                target.id,
                baseline_path.display(),
                exception_path.display(),
            )
        });
        assert!(
            matches!(
                evaluation.outcome,
                ComparisonOutcome::Passed | ComparisonOutcome::ApprovedException
            ),
            "{} performance gate requires Passed or ApprovedException: baseline_path={}, exception_path={}, evaluation={evaluation:?}",
            target.id,
            baseline_path.display(),
            exception_path.display(),
        );
    }
}

#[test]
fn function_performance_comparator_blocks_any_percentile_above_ten_percent() {
    let target = target_specs()
        .into_iter()
        .find(|target| target.id == FUNCTION_CRUD_ID)
        .expect("canonical Function CRUD target");
    let environment = EnvironmentFingerprint {
        os: "fixture-os".to_string(),
        architecture: "fixture-arch".to_string(),
        rustc: "fixture-rustc".to_string(),
        build_profile: "test".to_string(),
        cpu_model: "fixture-cpu".to_string(),
        logical_cpus: 1,
    };
    let baseline_report = BenchmarkReport::from_samples(
        target.clone(),
        environment.clone(),
        &vec![100; MEASURED_SAMPLES],
    )
    .expect("baseline report");
    let current = BenchmarkReport::from_samples(target, environment, &vec![111; MEASURED_SAMPLES])
        .expect("regressed report");
    let baseline = BenchmarkBaseline {
        report: baseline_report,
        approval: BaselineApproval {
            reviewer: "t083-reviewer".to_string(),
            approved_at: "2026-08-20".to_string(),
            source_revision: concat!(
                "git:0123456789abcdef0123456789abcdef01234567",
                "+hivegui-source-v1:",
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            )
            .to_string(),
        },
    };
    let result = compare_to_baseline(
        &current,
        Some(&baseline),
        None,
        NaiveDate::from_ymd_opt(2026, 8, 20).expect("valid review date"),
    )
    .expect("compatible comparison");

    assert_eq!(result.outcome, ComparisonOutcome::Blocked);
    assert_eq!(result.regressions.len(), 3, "p50, p95 and p99 all regress");
    assert_eq!(
        current.percentiles_ns,
        PercentilesNs {
            p50: 111,
            p95: 111,
            p99: 111,
        }
    );
}
