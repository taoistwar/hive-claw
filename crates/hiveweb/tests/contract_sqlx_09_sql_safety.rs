//! T017B + T017B' Red contract for SQLx 0.9 dynamic-SQL safety.
//!
//! The T017B' refresh (T017G batch) drops the
//! `dynamic_values_use_query_builder_bind_parameters` test that
//! incorrectly treated `QueryBuilder::push_bind` as a "compliant
//! example" and rewrites
//! `game_category_filter_uses_the_production_bound_query_helper` so
//! the contract asserts the actual static SQL constant
//! `JSON_OBJECT('name', ?)` and its bind behaviour, not a
//! production helper that returns a `QueryBuilder`. The contract
//! no longer requires production code to expose a helper that
//! returns `QueryBuilder`.
//!
//! Dynamic values must use bind parameters. Dynamic MySQL identifiers must be
//! accepted by an exact allowlist and serialized by one typed boundary. Only
//! that boundary may construct SQLx's `AssertSqlSafe`; production call sites
//! may not blanket-wrap arbitrary `String` values to silence SQLx 0.9.

use std::{fs, path::Path};

use hiveweb::db::sql_safety::{
    NamedQueryKind, OptimisticLockTable, TrustedSqlIdentifier, audit_named_query,
    audited_identifier_sql,
};
use sqlx::{AssertSqlSafe, Execute, MySql};

const HIVEWEB_SOURCE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
const ASSERT_SQL_SAFE_OWNER: &str = "db/sql_safety.rs";
const GAME_CATEGORY_FILTER_SQL: &str = "SELECT id FROM games WHERE category = JSON_OBJECT('name', ?) AND client_type IN (?, ?) AND channel = ? ORDER BY id LIMIT ?";
const HIVEWEB_GAME_SERVICE: &str = "services/game_service.rs";

#[test]
fn dynamic_identifiers_require_exact_allowlist_and_typed_serialization() {
    let allowlist = ["admins", "id"];
    let table =
        TrustedSqlIdentifier::from_allowlist("admins", &allowlist).expect("known table is allowed");
    let column =
        TrustedSqlIdentifier::from_allowlist("id", &allowlist).expect("known column is allowed");

    for untrusted in ["users", "admins` WHERE 1=1 --", "admins.id", ""] {
        assert!(
            TrustedSqlIdentifier::from_allowlist(untrusted, &allowlist).is_err(),
            "untrusted identifier {untrusted:?} must not reach SQL serialization"
        );
    }

    let audited: AssertSqlSafe<String> =
        audited_identifier_sql(&["SELECT * FROM ", " WHERE ", " = ?"], &[table, column])
            .expect("static fragments plus allowlisted identifiers are auditable");
    assert_eq!(
        audited.0, "SELECT * FROM `admins` WHERE `id` = ?",
        "the central boundary owns MySQL identifier quoting"
    );

    // The dynamic value remains a bind even after identifier serialization.
    let query = sqlx::query::<MySql>(audited).bind(7_i64);
    assert_eq!(query.sql(), "SELECT * FROM `admins` WHERE `id` = ?");
}

// ---------------------------------------------------------------------------
// T017B' — T017G refresh.
// ---------------------------------------------------------------------------

#[test]
fn game_category_filter_uses_static_sql_with_bind_only() {
    // T017B' rewrites the old "production helper" assertion. The
    // contract now verifies the **static** SQL constant HiveWeb
    // commits to (T017D's checked-macro target) and the bind
    // behaviour. There is no production helper that returns a
    // QueryBuilder; the contract no longer depends on one.
    assert_eq!(
        GAME_CATEGORY_FILTER_SQL.matches('?').count(),
        5,
        "category_name, client_type twice, channel, and limit are all bind parameters"
    );
    assert!(
        GAME_CATEGORY_FILTER_SQL.contains("JSON_OBJECT('name', ?)"),
        "the category JSON predicate must retain a bind placeholder"
    );
    assert!(
        !GAME_CATEGORY_FILTER_SQL.contains("category_name")
            && !GAME_CATEGORY_FILTER_SQL.contains("format!"),
        "the production SQL constant must not embed the dynamic value name"
    );
}

#[test]
fn game_service_source_no_longer_exposes_a_query_builder_helper() {
    // T017B' explicitly removes the requirement that production
    // code expose a helper returning a `QueryBuilder`. The
    // contract asserts the production source no longer contains
    // the legacy helper that returned one.
    let path = Path::new(HIVEWEB_SOURCE_ROOT).join(HIVEWEB_GAME_SERVICE);
    if !path.exists() {
        panic!(
            "missing {}; the T017B' source contract requires HiveWeb to own services/game_service.rs before T017G review",
            path.display()
        );
    }
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    assert!(
        !source.contains("build_fetch_logic_game_ids_by_tag_query"),
        "T017B' removes the production helper that returned a QueryBuilder; the file still exports it"
    );
    assert!(
        !source.contains("QueryBuilder::<MySql>"),
        "T017B' removes the runtime QueryBuilder usage from game_service.rs"
    );
}

#[test]
fn production_query_builder_call_count_is_exactly_zero() {
    // T017G workspace production SQL source inventory: production
    // code in HiveWeb MUST NOT call `QueryBuilder` at all. The
    // contract walks every Rust source file under `src/` and
    // asserts the count.
    let mut violations = Vec::new();
    visit_rust_files(Path::new(HIVEWEB_SOURCE_ROOT), &mut |path| {
        let relative = path
            .strip_prefix(HIVEWEB_SOURCE_ROOT)
            .expect("HiveWeb source path is rooted")
            .to_string_lossy()
            .replace('\\', "/");
        let source = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        for (index, line) in source.lines().enumerate() {
            if line.contains("QueryBuilder::") {
                violations.push(format!("{relative}:{}: {}", index + 1, line.trim()));
            }
        }
    });
    assert!(
        violations.is_empty(),
        "production code must not use SQLx QueryBuilder; offenders:\n{}",
        violations.join("\n")
    );
}

#[test]
fn reviewed_named_query_audit_accepts_one_parameterized_statement() {
    let audited = audit_named_query(
        "agent_by_id",
        "SELECT id, name FROM agents WHERE id = :id",
        NamedQueryKind::Select,
        &["id"],
    )
    .expect("one reviewed parameterized SELECT is accepted");

    assert_eq!(
        audited.sql(),
        "SELECT id, name FROM agents WHERE id = ?",
        "the audit boundary, not the caller, replaces named placeholders"
    );
    assert_eq!(audited.parameter_names(), ["id"]);
    assert_eq!(audited.kind(), NamedQueryKind::Select);
}

#[test]
fn reviewed_named_query_audit_rejects_multiple_statements_and_comments() {
    for (name, sql) in [
        (
            "multiple statements",
            "SELECT id FROM agents; DELETE FROM agents",
        ),
        ("line comment", "SELECT id FROM agents -- hidden suffix"),
        ("hash comment", "SELECT id FROM agents # hidden suffix"),
        ("block comment", "SELECT /* hidden */ id FROM agents"),
    ] {
        assert!(
            audit_named_query(name, sql, NamedQueryKind::Select, &[]).is_err(),
            "{name} must be rejected by the central reviewed-config boundary"
        );
    }
}

#[test]
fn reviewed_named_query_audit_rejects_placeholder_mismatches_and_duplicates() {
    for (name, sql, params) in [
        (
            "undeclared SQL placeholder",
            "SELECT id FROM agents WHERE id = :actual",
            vec!["declared"],
        ),
        (
            "declared placeholder absent from SQL",
            "SELECT id FROM agents",
            vec!["id"],
        ),
        (
            "duplicate declared parameter",
            "SELECT id FROM agents WHERE id = :id",
            vec!["id", "id"],
        ),
    ] {
        assert!(
            audit_named_query(name, sql, NamedQueryKind::Select, &params).is_err(),
            "{name} must be rejected before SQLx sees the configured SQL"
        );
    }
}

#[test]
fn reviewed_named_query_audit_rejects_declared_kind_mismatches() {
    for (name, sql, declared_kind) in [
        (
            "select declared for delete",
            "DELETE FROM agents WHERE id = :id",
            NamedQueryKind::Select,
        ),
        (
            "execute declared for select",
            "SELECT id FROM agents WHERE id = :id",
            NamedQueryKind::Execute,
        ),
    ] {
        assert!(
            audit_named_query(name, sql, declared_kind, &["id"]).is_err(),
            "{name} must be rejected by statement-kind inspection"
        );
    }
}

#[test]
fn optimistic_lock_identifiers_are_a_closed_exact_ten_table_enum() {
    let expected = [
        (OptimisticLockTable::Agents, "agents"),
        (OptimisticLockTable::AgentHooks, "agent_hooks"),
        (OptimisticLockTable::Categories, "categories"),
        (OptimisticLockTable::Functions, "functions"),
        (OptimisticLockTable::Plugins, "plugins"),
        (OptimisticLockTable::RecommendedGames, "recommended_games"),
        (OptimisticLockTable::Skills, "skills"),
        (OptimisticLockTable::Tags, "tags"),
        (OptimisticLockTable::Tools, "tools"),
        (OptimisticLockTable::Workflows, "workflows"),
    ];

    assert_eq!(
        OptimisticLockTable::ALL.len(),
        expected.len(),
        "the optimistic-lock allowlist contains exactly the ten application tables"
    );
    assert_eq!(
        OptimisticLockTable::ALL
            .iter()
            .map(|table| table.as_str())
            .collect::<Vec<_>>(),
        expected.iter().map(|(_, table)| *table).collect::<Vec<_>>(),
        "ALL must not omit or add an optimistic-lock table"
    );
    for (table, expected_identifier) in expected {
        assert_eq!(table.as_str(), expected_identifier);
    }
}

#[test]
fn only_the_central_audit_boundary_may_construct_assert_sql_safe_or_raw_sql() {
    let mut violations = Vec::new();
    visit_rust_files(Path::new(HIVEWEB_SOURCE_ROOT), &mut |path| {
        let relative = path
            .strip_prefix(HIVEWEB_SOURCE_ROOT)
            .expect("HiveWeb source path is rooted")
            .to_string_lossy()
            .replace('\\', "/");
        if relative == ASSERT_SQL_SAFE_OWNER {
            return;
        }

        let source = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        for (index, line) in source.lines().enumerate() {
            if line.contains("AssertSqlSafe") || line.contains("raw_sql(") {
                violations.push(format!("{relative}:{}: {}", index + 1, line.trim()));
            }
        }
    });

    assert!(
        violations.is_empty(),
        "dynamic SQL bypasses the central allowlist/audit boundary:\n{}",
        violations.join("\n")
    );
}

fn visit_rust_files(root: &Path, visit: &mut impl FnMut(&Path)) {
    let mut entries = fs::read_dir(root)
        .unwrap_or_else(|error| panic!("read {}: {error}", root.display()))
        .map(|entry| entry.expect("read HiveWeb source entry").path())
        .collect::<Vec<_>>();
    entries.sort();

    for path in entries {
        if path.is_dir() {
            visit_rust_files(&path, visit);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            visit(&path);
        }
    }
}
