//! Red contract for the v4 local Store migration boundary.
//!
//! These tests intentionally use the public migration API planned by T022. The
//! module and API do not exist yet: T012 must remain Red until the production
//! implementation provides the boundary and the committed golden fixtures.

mod support;

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use hivegui::datasource::entity_store::Tool;
use hivegui::datasource::function_store::FunctionStore;
use hivegui::datasource::migrations::{
    ArtifactSnapshot, CURRENT_SCHEMA_VERSION, MigrationErrorKind, MigrationFaultInjector,
    MigrationFaultPoint, MigrationOptions, MigrationStatus, capture_artifact_snapshot,
    migrate_to_current, restore_safe_snapshot, verify_safe_snapshot,
};
use hivegui::datasource::store::{Store, StoreOpenOptions};
use sqlx::{Row, SqlitePool, sqlite::SqlitePoolOptions};
use support::TestWorkspace;

const OWNER_PHASE: &str = "Foundation";
const APPROVAL_TASK: &str = "T012";
const IMPLEMENTATION_TASK: &str = "T022";
const FIXTURE_DIRECTORY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/migrations");
const SNAPSHOT_DIRECTORY: &str = "configured-migration-snapshots";

const DOTTED_BUILTIN_RENAMES: [(&str, &str); 4] = [
    ("format.template", "format_template"),
    ("json.parse", "json_parse"),
    ("json.stringify", "json_stringify"),
    ("text.regex_match", "text_regex_match"),
];

#[derive(Debug, Clone, Copy)]
struct MigrationCase {
    fixture: &'static str,
    from_version: i64,
    expected_status: MigrationStatus,
    owner_phase: &'static str,
    approval_task: &'static str,
}

const SUPPORTED_CASES: [MigrationCase; 2] = [
    MigrationCase {
        fixture: "v2-valid.sqlite",
        from_version: 2,
        expected_status: MigrationStatus::Migrated,
        owner_phase: OWNER_PHASE,
        approval_task: APPROVAL_TASK,
    },
    MigrationCase {
        fixture: "v3-valid.sqlite",
        from_version: 3,
        expected_status: MigrationStatus::Migrated,
        owner_phase: OWNER_PHASE,
        approval_task: APPROVAL_TASK,
    },
];

fn assert_case_ownership(case: &MigrationCase) {
    assert_eq!(case.owner_phase, OWNER_PHASE);
    assert_eq!(case.approval_task, APPROVAL_TASK);
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(FIXTURE_DIRECTORY).join(name)
}

fn copy_fixture(workspace: &TestWorkspace, name: &str) {
    let source = fixture_path(name);
    assert!(
        source.is_file(),
        "missing committed migration fixture {}; regenerate it only with the ignored T012 case",
        source.display()
    );
    fs::copy(&source, workspace.database_path()).unwrap_or_else(|error| {
        panic!(
            "copy fixture {} to isolated workspace: {error}",
            source.display()
        )
    });
}

fn migration_options(workspace: &TestWorkspace) -> MigrationOptions {
    MigrationOptions::new(workspace.database_path(), workspace.plugin_root())
        .with_snapshot_root(snapshot_root(workspace))
}

fn snapshot_root(workspace: &TestWorkspace) -> PathBuf {
    workspace.root().join(SNAPSHOT_DIRECTORY)
}

async fn read_schema_version(database: &Path) -> i64 {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}?mode=ro", database.display()))
        .await
        .expect("open migrated database read-only");
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'schema_version'",
    )
    .fetch_one(&pool)
    .await
    .expect("read one schema version");
    pool.close().await;
    version
}

async fn query_strings(database: &Path, sql: &str) -> Vec<String> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}?mode=ro", database.display()))
        .await
        .expect("open migrated database read-only");
    // `sql` is built by the test fixture helpers, not user input; wrap
    // with `AssertSqlSafe` to satisfy sqlx 0.9's `SqlSafeStr` contract.
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_all(&pool)
        .await
        .expect("query migrated fixture");
    let values = rows
        .into_iter()
        .map(|row| {
            // Sqlx reports the SQLite declared column type, not the
            // runtime expression type, so `CAST(kind AS TEXT)` on a
            // column declared as INTEGER still surfaces as INTEGER
            // to `try_get::<String>`. Try String first and fall back
            // to numeric conversion so the helper handles both.
            if let Ok(value) = row.try_get::<String, _>(0) {
                value
            } else if let Ok(value) = row.try_get::<i64, _>(0) {
                value.to_string()
            } else {
                panic!("first column is neither text nor integer")
            }
        })
        .collect();
    pool.close().await;
    values
}

async fn open_read_only(database: &Path) -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}?mode=ro", database.display()))
        .await
        .expect("open v4 database read-only for schema inspection")
}

fn canonical_schema_sql(sql: &str) -> String {
    sql.chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .replace('"', "'")
}

async fn schema_catalog(database: &Path) -> Vec<String> {
    query_strings(
        database,
        "SELECT type || '|' || name || '|' || tbl_name || '|' || COALESCE(sql, '') \
         FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' \
         ORDER BY type, name, tbl_name, COALESCE(sql, '')",
    )
    .await
}

async fn assert_restrict_foreign_key(
    pool: &SqlitePool,
    table: &str,
    from: &str,
    referenced_table: &str,
    referenced_column: &str,
) {
    let rows = match table {
        "functions" => {
            sqlx::query(
                "SELECT \"from\", \"table\", \"to\", on_delete \
             FROM pragma_foreign_key_list('functions')",
            )
            .fetch_all(pool)
            .await
        }
        "workflow_nodes" => {
            sqlx::query(
                "SELECT \"from\", \"table\", \"to\", on_delete \
             FROM pragma_foreign_key_list('workflow_nodes')",
            )
            .fetch_all(pool)
            .await
        }
        "tools" => {
            sqlx::query(
                "SELECT \"from\", \"table\", \"to\", on_delete \
             FROM pragma_foreign_key_list('tools')",
            )
            .fetch_all(pool)
            .await
        }
        other => panic!("unreviewed foreign-key table in T022 contract: {other}"),
    }
    .unwrap_or_else(|error| panic!("inspect {table} foreign keys: {error}"));

    let matching = rows
        .iter()
        .filter(|row| row.try_get::<String, _>("from").ok().as_deref() == Some(from))
        .collect::<Vec<_>>();
    assert_eq!(
        matching.len(),
        1,
        "{table}.{from} must have exactly one foreign key"
    );
    let row = matching[0];
    assert_eq!(
        row.try_get::<String, _>("table").expect("referenced table"),
        referenced_table,
        "{table}.{from} references the wrong table"
    );
    assert_eq!(
        row.try_get::<String, _>("to").expect("referenced column"),
        referenced_column,
        "{table}.{from} references the wrong column"
    );
    assert_eq!(
        row.try_get::<String, _>("on_delete")
            .expect("delete action"),
        "RESTRICT",
        "{table}.{from} must fail closed on delete"
    );
}

async fn assert_v4_search_and_reference_schema(database: &Path) {
    let pool = open_read_only(database).await;

    let search_documents_sql = sqlx::query_scalar::<_, String>(
        "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'search_documents'",
    )
    .fetch_one(&pool)
    .await
    .expect("v4 owns the canonical search_documents table");
    let search_documents_sql = canonical_schema_sql(&search_documents_sql);
    for required in [
        "createtablesearch_documents(",
        "idintegerprimarykey",
        "entity_typetextnotnull",
        "entity_keytextnotnull",
        "fieldtextnotnull",
        "normalized_texttextnotnull",
        "unique(entity_type,entity_key,field)",
    ] {
        assert!(
            search_documents_sql.contains(required),
            "search_documents is missing required schema fragment {required}: {search_documents_sql}"
        );
    }

    let search_fts_sql = sqlx::query_scalar::<_, String>(
        "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'search_documents_fts'",
    )
    .fetch_one(&pool)
    .await
    .expect("v4 owns the canonical search_documents_fts virtual table");
    let search_fts_sql = canonical_schema_sql(&search_fts_sql);
    for required in [
        "createvirtualtablesearch_documents_ftsusingfts5(",
        "normalized_text",
        "content='search_documents'",
        "content_rowid='id'",
        "tokenize='trigramcase_sensitive1'",
    ] {
        assert!(
            search_fts_sql.contains(required),
            "search_documents_fts is missing required external-content/trigram fragment {required}: {search_fts_sql}"
        );
    }

    let short_grams_sql = sqlx::query_scalar::<_, String>(
        "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'search_short_grams'",
    )
    .fetch_one(&pool)
    .await
    .expect("v4 owns the canonical search_short_grams table");
    let short_grams_sql = canonical_schema_sql(&short_grams_sql);
    for required in [
        "createtablesearch_short_grams(",
        "document_idinteger",
        "gram_leninteger",
        "check(gram_lenin(1,2))",
        "gramtextnotnull",
        "primarykey(document_id,gram_len,gram)",
        "withoutrowid",
    ] {
        assert!(
            short_grams_sql.contains(required),
            "search_short_grams is missing required schema fragment {required}: {short_grams_sql}"
        );
    }

    let short_gram_foreign_keys = sqlx::query(
        "SELECT \"from\", \"table\", \"to\", on_delete \
         FROM pragma_foreign_key_list('search_short_grams')",
    )
    .fetch_all(&pool)
    .await
    .expect("inspect search_short_grams foreign keys");
    assert_eq!(short_gram_foreign_keys.len(), 1);
    let short_gram_foreign_key = &short_gram_foreign_keys[0];
    assert_eq!(
        short_gram_foreign_key
            .try_get::<String, _>("from")
            .expect("short-gram foreign-key column"),
        "document_id"
    );
    assert_eq!(
        short_gram_foreign_key
            .try_get::<String, _>("table")
            .expect("short-gram parent table"),
        "search_documents"
    );
    assert_eq!(
        short_gram_foreign_key
            .try_get::<String, _>("to")
            .expect("short-gram parent column"),
        "id"
    );
    assert_eq!(
        short_gram_foreign_key
            .try_get::<String, _>("on_delete")
            .expect("short-gram delete action"),
        "CASCADE"
    );

    let explicit_indexes = sqlx::query(
        "SELECT name FROM pragma_index_list('search_short_grams') \
         WHERE origin = 'c' ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .expect("list explicit search_short_grams indexes");
    let mut has_covering_lookup = false;
    for index in explicit_indexes {
        let name = index.try_get::<String, _>("name").expect("index name");
        let columns =
            sqlx::query_scalar::<_, String>("SELECT name FROM pragma_index_info(?) ORDER BY seqno")
                .bind(&name)
                .fetch_all(&pool)
                .await
                .unwrap_or_else(|error| panic!("inspect {name}: {error}"));
        has_covering_lookup |= columns == ["gram_len", "gram", "document_id"];
    }
    assert!(
        has_covering_lookup,
        "search_short_grams requires an explicit (gram_len, gram, document_id) covering index"
    );

    let normalization_values = sqlx::query_scalar::<_, String>(
        "SELECT value FROM schema_metadata WHERE key = 'search_normalization_id'",
    )
    .fetch_all(&pool)
    .await
    .expect("read canonical search normalization metadata");
    assert_eq!(
        normalization_values,
        ["hivegui-nfkc-casefold-v1"],
        "schema_metadata must contain exactly one canonical normalization row"
    );

    assert_restrict_foreign_key(&pool, "functions", "plugin_id", "plugins", "id").await;
    assert_restrict_foreign_key(&pool, "workflow_nodes", "function_id", "functions", "id").await;
    assert_restrict_foreign_key(&pool, "tools", "function_id", "functions", "id").await;
    assert_restrict_foreign_key(&pool, "tools", "workflow_id", "workflows", "id").await;

    pool.close().await;
}

async fn assert_v4_invariants(workspace: &TestWorkspace) {
    assert_eq!(
        read_schema_version(workspace.database_path()).await,
        CURRENT_SCHEMA_VERSION
    );

    let integrity = query_strings(workspace.database_path(), "PRAGMA integrity_check").await;
    assert_eq!(integrity, ["ok"]);
    let foreign_key_failures =
        query_strings(workspace.database_path(), "PRAGMA foreign_key_check").await;
    assert!(foreign_key_failures.is_empty());
    assert_v4_search_and_reference_schema(workspace.database_path()).await;

    let node_types = query_strings(
        workspace.database_path(),
        "SELECT DISTINCT node_type FROM workflow_nodes ORDER BY node_type",
    )
    .await;
    assert_eq!(
        node_types,
        [
            "end_node",
            "function_node",
            "generate_answer_node",
            "start_node"
        ]
    );

    for (dotted, underscored) in DOTTED_BUILTIN_RENAMES {
        let old_count = query_strings(
            workspace.database_path(),
            &format!("SELECT CAST(COUNT(*) AS TEXT) FROM functions WHERE identifier = '{dotted}'"),
        )
        .await;
        let new_count = query_strings(
            workspace.database_path(),
            &format!(
                "SELECT CAST(COUNT(*) AS TEXT) FROM functions WHERE identifier = '{underscored}'"
            ),
        )
        .await;
        assert_eq!(old_count, ["0"], "dotted names are migration input only");
        assert_eq!(new_count, ["1"], "renamed Builtin must be unique");
    }
}

#[tokio::test]
async fn empty_database_is_created_directly_at_v4_and_reopening_is_idempotent() {
    let workspace = TestWorkspace::new().expect("isolated workspace");

    let first = migrate_to_current(migration_options(&workspace))
        .await
        .expect("empty database should be created");
    assert_eq!(first.status, MigrationStatus::Created);
    assert_eq!(first.from_version, None);
    assert_eq!(first.to_version, CURRENT_SCHEMA_VERSION);
    assert!(first.validation.integrity_ok);
    assert!(first.validation.foreign_keys_ok);
    assert!(
        first.snapshot_location().is_none(),
        "a newly created database does not require a migration snapshot"
    );
    assert_v4_search_and_reference_schema(workspace.database_path()).await;

    let before_reopen =
        capture_artifact_snapshot(workspace.database_path(), workspace.plugin_root())
            .await
            .expect("snapshot closed v4 database and managed plugins");
    let second = migrate_to_current(migration_options(&workspace))
        .await
        .expect("v4 reopening should be a no-op");
    let after_reopen =
        capture_artifact_snapshot(workspace.database_path(), workspace.plugin_root())
            .await
            .expect("snapshot reopened v4 database and managed plugins");

    assert_eq!(second.status, MigrationStatus::Unchanged);
    assert_eq!(second.from_version, Some(CURRENT_SCHEMA_VERSION));
    assert!(
        second.snapshot_location().is_none(),
        "an unchanged current database does not require a migration snapshot"
    );
    assert_eq!(before_reopen, after_reopen);
}

#[tokio::test]
async fn supported_v2_and_v3_fixtures_upgrade_transactionally_to_v4() {
    for case in SUPPORTED_CASES {
        assert_case_ownership(&case);
        let workspace = TestWorkspace::new().expect("isolated workspace");
        copy_fixture(&workspace, case.fixture);
        copy_managed_plugin_fixture(&workspace);
        let before = capture_artifact_snapshot(workspace.database_path(), workspace.plugin_root())
            .await
            .expect("capture pre-migration artifact snapshot");

        let report = migrate_to_current(migration_options(&workspace))
            .await
            .unwrap_or_else(|error| panic!("{} must migrate: {error}", case.fixture));

        assert_eq!(report.status, case.expected_status);
        assert_eq!(report.from_version, Some(case.from_version));
        assert_eq!(report.to_version, CURRENT_SCHEMA_VERSION);
        assert!(report.validation.integrity_ok);
        assert!(report.validation.foreign_keys_ok);
        assert!(report.validation.managed_plugins_ok);
        assert_eq!(report.pre_migration_snapshot.as_ref(), Some(&before));
        assert_v4_invariants(&workspace).await;
    }
}

#[tokio::test]
async fn current_v4_missing_a_required_search_table_fails_closed_without_partial_repair() {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    migrate_to_current(migration_options(&workspace))
        .await
        .expect("create initial v4 database");

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}", workspace.database_path().display()))
        .await
        .expect("open v4 database to inject schema drift");
    sqlx::query("DROP TABLE IF EXISTS search_documents")
        .execute(&pool)
        .await
        .expect("remove required v4 search table");
    pool.close().await;

    let before_schema = schema_catalog(workspace.database_path()).await;
    let before_artifacts = snapshot(&workspace).await;
    migrate_to_current(migration_options(&workspace))
        .await
        .expect_err("a schema marked v4 but missing required search DDL must fail closed");

    assert_eq!(
        schema_catalog(workspace.database_path()).await,
        before_schema,
        "opening a drifted v4 database must not repair or partially mutate its schema"
    );
    assert_eq!(
        snapshot(&workspace).await,
        before_artifacts,
        "opening a drifted v4 database must preserve every database and Plugin artifact"
    );
    assert_eq!(
        read_schema_version(workspace.database_path()).await,
        CURRENT_SCHEMA_VERSION
    );
}

#[tokio::test]
async fn legacy_v4_search_schema_upgrades_without_losing_primary_data() {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    migrate_to_current(migration_options(&workspace))
        .await
        .expect("create initial v4 database");

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}", workspace.database_path().display()))
        .await
        .expect("open v4 database to install the legacy search schema");
    sqlx::query(
        "INSERT INTO categories (name, slug, description, created_at, updated_at) \
         VALUES ('Legacy Search Data', 'legacy-search-data', 'must survive upgrade', \
                 '2026-09-01T00:00:00Z', '2026-09-01T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .expect("seed primary data before the compatibility upgrade");
    for statement in [
        "DROP TABLE search_documents_fts",
        "DROP TABLE search_short_grams",
        "DROP TABLE search_documents",
        "DROP TABLE schema_metadata",
        "CREATE VIRTUAL TABLE search_index USING fts5(\
             identifier, display_name, payload, tokenize = \"trigram\"\
         )",
        "CREATE TABLE short_gram_index (\
             gram TEXT NOT NULL, entity TEXT NOT NULL, primary_key INTEGER NOT NULL, \
             UNIQUE(gram, entity, primary_key)\
         )",
    ] {
        sqlx::query(statement)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("install legacy search DDL {statement:?}: {error}"));
    }
    pool.close().await;

    let before = capture_artifact_snapshot(workspace.database_path(), workspace.plugin_root())
        .await
        .expect("capture recognizable legacy-v4 snapshot");
    let report = migrate_to_current(migration_options(&workspace))
        .await
        .expect("recognizable legacy v4 search schema must upgrade at startup");

    assert_eq!(report.status, MigrationStatus::Migrated);
    assert_eq!(report.from_version, Some(CURRENT_SCHEMA_VERSION));
    assert_eq!(report.to_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(report.pre_migration_snapshot.as_ref(), Some(&before));
    assert!(report.validation.integrity_ok);
    assert!(report.validation.foreign_keys_ok);
    assert_v4_search_and_reference_schema(workspace.database_path()).await;
    assert_eq!(
        query_strings(
            workspace.database_path(),
            "SELECT slug FROM categories WHERE slug = 'legacy-search-data'",
        )
        .await,
        ["legacy-search-data"],
        "the compatibility upgrade must preserve primary entity rows"
    );
}

#[tokio::test]
async fn legacy_v4_empty_plugin_ledger_upgrades_with_the_legacy_search_schema() {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    migrate_to_current(migration_options(&workspace))
        .await
        .expect("create initial v4 database");

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}", workspace.database_path().display()))
        .await
        .expect("open v4 database to install historical v4 schemas");
    for statement in [
        "DROP TABLE plugin_artifact_gc",
        "DROP TABLE plugin_artifact_operations",
        "CREATE TABLE plugin_artifact_operations (\
             expected_old_identifier TEXT, expected_old_resource_limits TEXT, \
             expected_old_row_revision INTEGER, expected_old_s3_key TEXT, \
             expected_old_sha256 TEXT, expected_old_size INTEGER, \
             expected_old_version TEXT, \
             kind TEXT NOT NULL CHECK(kind IN ('create','replace')), \
             new_identity TEXT, new_resource_limits TEXT, new_s3_key TEXT UNIQUE, \
             new_sha256 TEXT, new_size INTEGER, operation_id TEXT PRIMARY KEY, \
             plugin_id TEXT, staging_identity TEXT, staging_name TEXT NOT NULL UNIQUE, \
             state TEXT NOT NULL CHECK(state IN \
                 ('prepared','staged','published','referenced','done','conflict'))\
         )",
        "CREATE TABLE plugin_artifact_gc (\
             artifact_key TEXT PRIMARY KEY, last_attempt_at INTEGER NOT NULL, \
             reason TEXT NOT NULL, \
             state TEXT NOT NULL CHECK(state IN ('pending','blocked'))\
         )",
        "DROP TABLE search_documents_fts",
        "DROP TABLE search_short_grams",
        "DROP TABLE search_documents",
        "DROP TABLE schema_metadata",
        "CREATE VIRTUAL TABLE search_index USING fts5(\
             identifier, display_name, payload, tokenize = \"trigram\"\
         )",
        "CREATE TABLE short_gram_index (\
             gram TEXT NOT NULL, entity TEXT NOT NULL, primary_key INTEGER NOT NULL, \
             UNIQUE(gram, entity, primary_key)\
         )",
    ] {
        sqlx::query(statement)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("install historical v4 DDL {statement:?}: {error}"));
    }
    pool.close().await;

    let before = capture_artifact_snapshot(workspace.database_path(), workspace.plugin_root())
        .await
        .expect("capture historical v4 snapshot");
    let report = migrate_to_current(migration_options(&workspace))
        .await
        .expect("exact empty historical Plugin ledger must upgrade at startup");

    assert_eq!(report.status, MigrationStatus::Migrated);
    assert_eq!(report.from_version, Some(CURRENT_SCHEMA_VERSION));
    assert_eq!(report.to_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(report.pre_migration_snapshot.as_ref(), Some(&before));
    assert!(report.validation.integrity_ok);
    assert!(report.validation.foreign_keys_ok);
    assert!(report.validation.managed_plugins_ok);
    assert_v4_search_and_reference_schema(workspace.database_path()).await;
}

#[tokio::test]
async fn v2_and_v3_fault_after_v4_ddl_rolls_back_the_exact_sqlite_schema() {
    for fixture in ["v2-valid.sqlite", "v3-valid.sqlite"] {
        let workspace = TestWorkspace::new().expect("isolated workspace");
        copy_fixture(&workspace, fixture);
        copy_managed_plugin_fixture(&workspace);
        let before_schema = schema_catalog(workspace.database_path()).await;
        let before_artifacts = snapshot(&workspace).await;

        let error = migrate_to_current(migration_options(&workspace).with_fault_injector(
            Arc::new(OneShotMigrationFault::new(MigrationFaultPoint::AfterV3ToV4)),
        ))
        .await
        .expect_err("fault after v4 DDL must abort the entire migration transaction");

        assert_eq!(error.kind(), MigrationErrorKind::InjectedFault);
        assert_eq!(
            schema_catalog(workspace.database_path()).await,
            before_schema,
            "{fixture} retained partial v4 sqlite_schema rows after rollback"
        );
        assert_eq!(
            snapshot(&workspace).await,
            before_artifacts,
            "{fixture} retained partial database or Plugin changes after rollback"
        );
    }
}

#[tokio::test]
async fn v2_maps_function_and_tool_kinds_and_legacy_llm_fields_exactly() {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    copy_fixture(&workspace, "v2-valid.sqlite");

    assert_eq!(
        query_strings(
            workspace.database_path(),
            "SELECT DISTINCT CAST(kind AS TEXT) FROM functions ORDER BY CAST(kind AS INTEGER)",
        )
        .await,
        ["1", "2", "3"]
    );
    assert_eq!(
        query_strings(
            workspace.database_path(),
            "SELECT DISTINCT CAST(kind AS TEXT) FROM tools ORDER BY CAST(kind AS INTEGER)",
        )
        .await,
        ["1", "2"]
    );
    assert_eq!(
        query_strings(
            workspace.database_path(),
            "SELECT identifier FROM functions WHERE identifier LIKE '%.%' ORDER BY identifier",
        )
        .await,
        [
            "format.template",
            "json.parse",
            "json.stringify",
            "text.regex_match"
        ]
    );
    assert_eq!(
        query_strings(
            workspace.database_path(),
            "SELECT DISTINCT node_type FROM workflow_nodes ORDER BY node_type",
        )
        .await,
        [
            "end_node",
            "function_node",
            "generate_answer_node",
            "start_node"
        ]
    );
    assert_eq!(
        query_strings(
            workspace.database_path(),
            "SELECT name || '|' || kind || '|' || hex(api_key_encrypted) || '|' || api_key_env FROM llm_providers WHERE name = 'fixture_provider'",
        )
        .await,
        ["fixture_provider|openai_compatible|5A5A5A5A|FIXTURE_LLM_TOKEN"]
    );

    migrate_to_current(migration_options(&workspace))
        .await
        .expect("v2 fixture migrates");

    let function_kinds = query_strings(
        workspace.database_path(),
        "SELECT kind FROM functions WHERE identifier IN ('format_template', 'fixture_custom', 'fixture_placeholder') ORDER BY identifier",
    )
    .await;
    assert_eq!(function_kinds, ["custom", "placeholder", "builtin"]);
    let all_function_kinds = query_strings(
        workspace.database_path(),
        "SELECT DISTINCT kind FROM functions ORDER BY kind",
    )
    .await;
    assert_eq!(all_function_kinds, ["builtin", "custom", "placeholder"]);

    let tool_kinds = query_strings(
        workspace.database_path(),
        "SELECT DISTINCT kind FROM tools ORDER BY kind",
    )
    .await;
    assert_eq!(tool_kinds, ["function-wrap", "workflow-wrap"]);

    let provider = query_strings(
        workspace.database_path(),
        "SELECT name || '|' || category || '|' || hex(token_encrypted) || '|' || token_env FROM llm_providers WHERE name = 'fixture_provider'",
    )
    .await;
    assert_eq!(
        provider,
        ["fixture_provider|openai_compatible|5A5A5A5A|FIXTURE_LLM_TOKEN"]
    );

    let provider_columns = query_strings(
        workspace.database_path(),
        "SELECT name FROM pragma_table_info('llm_providers') ORDER BY name",
    )
    .await;
    for legacy in ["kind", "api_key_encrypted", "api_key_env"] {
        assert!(!provider_columns.iter().any(|column| column == legacy));
    }
    for canonical in ["category", "token_encrypted", "token_env"] {
        assert!(provider_columns.iter().any(|column| column == canonical));
    }
}

#[tokio::test]
async fn v4_migration_decodes_string_kinds_through_entity_readers() {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    copy_fixture(&workspace, "v2-valid.sqlite");
    migrate_to_current(migration_options(&workspace))
        .await
        .expect("v2 fixture migrates to v4");

    // Open a real v4 Store through the public boundary, then read the
    // migrated functions/tools through the entity readers. This is the
    // regression guard for the kind type split: before the fix,
    // `FunctionStore::list` / `Tool::list` would try to decode the v4 TEXT
    // kind column into `i64` and fail at runtime.
    let store = Store::open_local(StoreOpenOptions::new(
        workspace.database_path(),
        workspace.plugin_root(),
    ))
    .await
    .expect("open real v4 Store");

    let function_page = FunctionStore::new(store.pool().clone())
        .expect("Function Store")
        .list(None, 1)
        .await
        .expect("list migrated functions");
    let functions = function_page.items();
    assert!(!functions.is_empty(), "v2 fixture must contain functions");
    for function in functions {
        assert!(
            ["builtin", "custom", "placeholder"].contains(&function.kind().as_str()),
            "function {} has unexpected kind {:?}",
            function.identifier(),
            function.kind()
        );
    }

    let tools = Tool::list(store.pool(), None, 100, 0)
        .await
        .expect("list migrated tools");
    assert!(!tools.is_empty(), "v2 fixture must contain tools");
    for tool in &tools {
        assert!(
            ["function-wrap", "workflow-wrap"].contains(&tool.kind.as_str()),
            "tool {} has unexpected kind {:?}",
            tool.identifier,
            tool.kind
        );
    }
}

#[tokio::test]
async fn supported_upgrade_preserves_relations_non_migrated_fields_and_managed_plugins() {
    for fixture in ["v2-valid.sqlite", "v3-valid.sqlite"] {
        let workspace = TestWorkspace::new().expect("isolated workspace");
        copy_fixture(&workspace, fixture);
        copy_managed_plugin_fixture(&workspace);

        let before = capture_artifact_snapshot(workspace.database_path(), workspace.plugin_root())
            .await
            .expect("pre-migration snapshot");
        migrate_to_current(migration_options(&workspace))
            .await
            .expect("supported migration");
        let after = capture_artifact_snapshot(workspace.database_path(), workspace.plugin_root())
            .await
            .expect("post-migration snapshot");

        assert_eq!(before.managed_plugin_digest, after.managed_plugin_digest);
        assert_eq!(
            before.managed_plugin_manifest,
            after.managed_plugin_manifest
        );
        assert_eq!(
            before.non_migrated_logical_digest,
            after.non_migrated_logical_digest
        );
        assert_eq!(before.relationship_digest, after.relationship_digest);
    }
}

#[tokio::test]
async fn v1_and_v5_are_classified_without_modifying_database_or_plugins() {
    for (fixture, kind) in [
        ("v1.sqlite", MigrationErrorKind::SchemaTooOld),
        ("v5.sqlite", MigrationErrorKind::SchemaNewerThanApplication),
    ] {
        let workspace = TestWorkspace::new().expect("isolated workspace");
        copy_fixture(&workspace, fixture);
        copy_managed_plugin_fixture(&workspace);
        let before = snapshot(&workspace).await;

        let error = migrate_to_current(migration_options(&workspace))
            .await
            .expect_err("unsupported schema must not be opened");

        assert_eq!(error.kind(), kind);
        assert!(
            error.snapshot_location().is_none(),
            "unsupported versions are rejected before a migration snapshot is needed"
        );
        assert_eq!(snapshot(&workspace).await, before);
    }
}

#[tokio::test]
async fn unknown_function_kind_rolls_back_every_artifact_and_retries_deterministically() {
    assert_rollback_fixture(
        "v2-unknown-function-kind.sqlite",
        MigrationErrorKind::UnknownFunctionKind,
    )
    .await;
}

#[tokio::test]
async fn unknown_tool_kind_rolls_back_every_artifact_and_retries_deterministically() {
    assert_rollback_fixture(
        "v2-unknown-tool-kind.sqlite",
        MigrationErrorKind::UnknownToolKind,
    )
    .await;
}

#[tokio::test]
async fn dotted_builtin_target_collision_rolls_back_without_alias_merge_or_overwrite() {
    assert_rollback_fixture(
        "v2-builtin-collision.sqlite",
        MigrationErrorKind::BuiltinIdentifierCollision,
    )
    .await;
}

#[tokio::test]
async fn every_transactional_fault_point_restores_database_and_managed_plugins() {
    const SNAPSHOT_BUILD_FAULT_POINTS: &[MigrationFaultPoint] = &[
        MigrationFaultPoint::BeforeDatabaseSnapshot,
        MigrationFaultPoint::AfterDatabaseSnapshot,
        MigrationFaultPoint::BeforeManagedPluginTreeSnapshot,
        MigrationFaultPoint::AfterManagedPluginTreeSnapshot,
        MigrationFaultPoint::BeforeSafeSnapshotVerification,
    ];
    const V2_FAULT_POINTS: &[MigrationFaultPoint] = &[
        MigrationFaultPoint::AfterSafeSnapshotVerification,
        MigrationFaultPoint::AfterArtifactSnapshot,
        MigrationFaultPoint::AfterBegin,
        MigrationFaultPoint::AfterV2ToV3,
        MigrationFaultPoint::AfterV3ToV4,
        MigrationFaultPoint::BeforeIntegrityCheck,
        MigrationFaultPoint::BeforeCommit,
        MigrationFaultPoint::AfterCommitBeforeRecovery,
        MigrationFaultPoint::DuringPostCommitIntegrityCheck,
        MigrationFaultPoint::DuringPostCommitForeignKeyCheck,
        MigrationFaultPoint::DuringPostCommitManagedPluginValidation,
    ];
    const V3_FAULT_POINTS: &[MigrationFaultPoint] = &[
        MigrationFaultPoint::AfterSafeSnapshotVerification,
        MigrationFaultPoint::AfterArtifactSnapshot,
        MigrationFaultPoint::AfterBegin,
        MigrationFaultPoint::AfterV3ToV4,
        MigrationFaultPoint::BeforeIntegrityCheck,
        MigrationFaultPoint::BeforeCommit,
        MigrationFaultPoint::AfterCommitBeforeRecovery,
        MigrationFaultPoint::DuringPostCommitIntegrityCheck,
        MigrationFaultPoint::DuringPostCommitForeignKeyCheck,
        MigrationFaultPoint::DuringPostCommitManagedPluginValidation,
    ];

    for (fixture, source_version, fault_points) in [
        ("v2-valid.sqlite", 2, V2_FAULT_POINTS),
        ("v3-valid.sqlite", 3, V3_FAULT_POINTS),
    ] {
        for &point in SNAPSHOT_BUILD_FAULT_POINTS {
            let workspace = TestWorkspace::new().expect("isolated workspace");
            copy_fixture(&workspace, fixture);
            copy_managed_plugin_fixture(&workspace);
            let before = snapshot(&workspace).await;

            let error = migrate_to_current(
                migration_options(&workspace)
                    .with_fault_injector(Arc::new(OneShotMigrationFault::new(point))),
            )
            .await
            .expect_err("an incomplete or unverified safe snapshot must stop migration");

            assert_eq!(error.kind(), MigrationErrorKind::InjectedFault);
            assert_eq!(error.fault_point(), Some(point));
            assert!(
                error.snapshot_location().is_none(),
                "an incomplete or unverified snapshot must never be advertised as recoverable"
            );
            assert_no_partial_snapshot_artifacts(&workspace);
            assert_eq!(snapshot(&workspace).await, before);
            assert_eq!(
                read_schema_version(workspace.database_path()).await,
                source_version
            );
        }

        for &point in fault_points {
            let workspace = TestWorkspace::new().expect("isolated workspace");
            copy_fixture(&workspace, fixture);
            copy_managed_plugin_fixture(&workspace);
            let before = snapshot(&workspace).await;

            for attempt in 1..=2 {
                let options = migration_options(&workspace)
                    .with_fault_injector(Arc::new(OneShotMigrationFault::new(point)));
                let error = migrate_to_current(options)
                    .await
                    .expect_err("injected migration fault must fail");

                assert_eq!(error.kind(), MigrationErrorKind::InjectedFault);
                assert_eq!(error.fault_point(), Some(point));
                let location = error
                    .snapshot_location()
                    .expect("migration fault reports the retained safe snapshot")
                    .to_path_buf();
                assert_safe_snapshot_artifact(&workspace, &location, &before).await;
                assert_eq!(
                    snapshot(&workspace).await,
                    before,
                    "{fixture} fault at {point:?}, attempt {attempt}"
                );
                assert_eq!(
                    read_schema_version(workspace.database_path()).await,
                    source_version
                );
            }
        }
    }
}

async fn assert_rollback_fixture(fixture: &str, expected_kind: MigrationErrorKind) {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    copy_fixture(&workspace, fixture);
    copy_managed_plugin_fixture(&workspace);
    let before = snapshot(&workspace).await;

    for attempt in 1..=2 {
        let error = migrate_to_current(migration_options(&workspace))
            .await
            .expect_err("invalid migration input must fail");
        assert_eq!(error.kind(), expected_kind, "attempt {attempt}");
        let location = error
            .snapshot_location()
            .expect("invalid migration input reports the retained safe snapshot")
            .to_path_buf();
        assert_safe_snapshot_artifact(&workspace, &location, &before).await;
        assert_eq!(snapshot(&workspace).await, before, "attempt {attempt}");
        assert_eq!(read_schema_version(workspace.database_path()).await, 2);
    }
}

async fn assert_safe_snapshot_artifact(
    source: &TestWorkspace,
    location: &Path,
    expected: &ArtifactSnapshot,
) {
    assert!(location.is_absolute(), "snapshot location is unambiguous");
    let configured_snapshot_root = snapshot_root(source);
    assert!(
        location.starts_with(&configured_snapshot_root),
        "snapshot must remain inside the explicitly configured snapshot root {}: {}",
        configured_snapshot_root.display(),
        location.display(),
    );
    assert!(
        location.exists(),
        "reported snapshot artifact must exist: {}",
        location.display()
    );
    assert_ne!(location, source.database_path());
    assert_ne!(location, source.plugin_root());

    let verified = verify_safe_snapshot(location)
        .await
        .expect("snapshot manifest, hashes, and logical summaries verify");
    assert!(verified.database_present);
    assert!(verified.managed_plugin_tree_present);
    assert!(verified.database_hash_valid);
    assert!(verified.managed_plugin_hashes_valid);
    assert!(verified.logical_summary_valid);
    assert!(verified.relationship_summary_valid);
    assert_eq!(
        &verified.source_artifacts, expected,
        "verified database/Plugin hashes and logical summaries match the closed source"
    );

    let recovery = TestWorkspace::new().expect("isolated recovery workspace");
    restore_safe_snapshot(location, recovery.database_path(), recovery.plugin_root())
        .await
        .expect("verified migration snapshot is directly usable for recovery");
    let restored = snapshot(&recovery).await;
    assert_eq!(
        &restored, expected,
        "recovery reproduces the source database, managed Plugin tree, and logical summaries"
    );
}

fn assert_no_partial_snapshot_artifacts(workspace: &TestWorkspace) {
    let root = snapshot_root(workspace);
    if !root.exists() {
        return;
    }

    let entries = fs::read_dir(&root)
        .unwrap_or_else(|error| panic!("read configured snapshot root {}: {error}", root.display()))
        .map(|entry| entry.expect("read snapshot-root entry").path())
        .collect::<Vec<_>>();
    assert!(
        entries.is_empty(),
        "failed snapshot construction must clean unverified staging artifacts: {entries:?}"
    );
}

async fn snapshot(workspace: &TestWorkspace) -> ArtifactSnapshot {
    capture_artifact_snapshot(workspace.database_path(), workspace.plugin_root())
        .await
        .expect("capture database, logical tables, and managed plugin tree")
}

fn copy_managed_plugin_fixture(workspace: &TestWorkspace) {
    let source_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugins/shared-smoke");
    let target_root = workspace.plugin_root().join("shared-smoke");
    fs::create_dir_all(&target_root).expect("create managed Plugin fixture directory");
    for name in ["plugin.wasm", "manifest.json", "artifact.json"] {
        let source = source_root.join(name);
        assert!(
            source.is_file(),
            "missing managed Plugin fixture {}",
            source.display()
        );
        fs::copy(&source, target_root.join(name))
            .unwrap_or_else(|error| panic!("copy {}: {error}", source.display()));
    }
}

#[derive(Debug)]
struct OneShotMigrationFault {
    point: MigrationFaultPoint,
}

impl OneShotMigrationFault {
    fn new(point: MigrationFaultPoint) -> Self {
        Self { point }
    }
}

impl MigrationFaultInjector for OneShotMigrationFault {
    fn should_fail(&self, point: MigrationFaultPoint) -> bool {
        point == self.point
    }
}

/// Explicit, never-default entrypoint for regenerating historical golden files.
/// The implementation must use versioned historical DDL, deterministic values,
/// validation and atomic replacement; it must never derive v2/v3 from Store v4.
#[tokio::test]
#[ignore = "explicit fixture regeneration only; never run in the normal test suite"]
async fn regenerate_committed_migration_fixtures_from_versioned_historical_ddl() {
    use hivegui::datasource::migrations::fixtures::{
        FixtureGenerationOptions, regenerate_golden_fixtures,
    };

    let report = regenerate_golden_fixtures(FixtureGenerationOptions {
        output_directory: Path::new(FIXTURE_DIRECTORY),
        timestamp: "2000-01-01T00:00:00Z",
        sqlite_page_size: 4096,
        owner_phase: OWNER_PHASE,
        approval_task: APPROVAL_TASK,
        implementation_task: IMPLEMENTATION_TASK,
    })
    .await
    .expect("regenerate deterministic historical fixtures");

    assert_eq!(
        report.generated_files,
        [
            "v1.sqlite",
            "v2-valid.sqlite",
            "v2-unknown-function-kind.sqlite",
            "v2-unknown-tool-kind.sqlite",
            "v2-builtin-collision.sqlite",
            "v3-valid.sqlite",
            "v5.sqlite",
        ]
    );
    assert!(report.all_integrity_checks_passed);
    assert!(report.all_foreign_key_checks_passed);
    assert!(report.no_sqlite_sidecars);
    assert!(report.atomic_replacement_used);
}
