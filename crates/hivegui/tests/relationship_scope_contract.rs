//! T013 Red contract for the deliberately narrow v4 relationship scope.
//!
//! Schema shape and absence of undeclared relationship surfaces are Foundation
//! contracts. CRUD/reference behaviour remains activated by the owning user
//! story tests.

mod support;

use std::{collections::BTreeSet, fs, path::Path};

use hivegui::datasource::entity_store::Tag;
use sqlx::Row;
use support::TestWorkspace;

const ENTITY_STORE_SOURCE: &str = include_str!("../src/datasource/entity_store.rs");
const TAG_VIEW_SOURCE: &str = include_str!("../src/ui/tag_view.rs");

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct TableContract {
    table: &'static str,
    owner_phase: &'static str,
    activation_task: &'static str,
    relationship_table: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ForeignKeyContract {
    table: &'static str,
    from_column: &'static str,
    target_table: &'static str,
    target_column: &'static str,
    on_delete: &'static str,
    owner_phase: &'static str,
    activation_task: &'static str,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ActualForeignKey {
    table: String,
    from_column: String,
    target_table: String,
    target_column: String,
    on_delete: String,
}

macro_rules! table {
    ($name:literal) => {
        TableContract {
            table: $name,
            owner_phase: "Foundation",
            activation_task: "T017",
            relationship_table: false,
        }
    };
    ($name:literal, relationship) => {
        TableContract {
            table: $name,
            owner_phase: "Foundation",
            activation_task: "T017",
            relationship_table: true,
        }
    };
}

macro_rules! fk {
    ($table:literal, $from:literal => $target:literal, $to:literal, $on_delete:literal) => {
        ForeignKeyContract {
            table: $table,
            from_column: $from,
            target_table: $target,
            target_column: $to,
            on_delete: $on_delete,
            owner_phase: "Foundation",
            activation_task: "T017",
        }
    };
}

const V4_APPLICATION_TABLES: &[TableContract] = &[
    table!("schema_versions"),
    table!("data_sources"),
    table!("global_configs"),
    table!("llm_presets"),
    table!("llm_providers"),
    table!("models"),
    table!("tags"),
    table!("categories"),
    table!("capabilities"),
    table!("plugins"),
    table!("functions"),
    table!("workflows"),
    table!("workflow_nodes", relationship),
    table!("workflow_edges", relationship),
    table!("tools"),
    table!("skills"),
    table!("agents"),
    table!("agent_tools", relationship),
    table!("agent_skills", relationship),
    table!("agent_capabilities", relationship),
    table!("chat_sessions"),
    table!("chat_messages"),
    table!("agent_executions"),
];

const V4_FOREIGN_KEYS: &[ForeignKeyContract] = &[
    fk!("llm_presets", "provider_id" => "llm_providers", "id", "RESTRICT"),
    fk!("models", "provider_id" => "llm_providers", "id", "RESTRICT"),
    fk!("categories", "parent_id" => "categories", "id", "SET NULL"),
    fk!("capabilities", "category_id" => "categories", "id", "SET NULL"),
    fk!("plugins", "category_id" => "categories", "id", "SET NULL"),
    fk!("functions", "plugin_id" => "plugins", "id", "RESTRICT"),
    fk!("functions", "category_id" => "categories", "id", "SET NULL"),
    fk!("workflows", "category_id" => "categories", "id", "SET NULL"),
    fk!("workflow_nodes", "workflow_id" => "workflows", "id", "CASCADE"),
    fk!("workflow_edges", "workflow_id" => "workflows", "id", "CASCADE"),
    fk!("tools", "function_id" => "functions", "id", "RESTRICT"),
    fk!("tools", "workflow_id" => "workflows", "id", "RESTRICT"),
    fk!("tools", "category_id" => "categories", "id", "SET NULL"),
    fk!("skills", "category_id" => "categories", "id", "SET NULL"),
    fk!("agents", "parent_agent_id" => "agents", "id", "SET NULL"),
    fk!("agents", "category_id" => "categories", "id", "SET NULL"),
    fk!("agent_tools", "agent_id" => "agents", "id", "CASCADE"),
    fk!("agent_tools", "tool_id" => "tools", "id", "CASCADE"),
    fk!("agent_skills", "agent_id" => "agents", "id", "CASCADE"),
    fk!("agent_skills", "skill_id" => "skills", "id", "CASCADE"),
    fk!("agent_capabilities", "agent_id" => "agents", "id", "CASCADE"),
    fk!(
        "agent_capabilities",
        "capability_name" => "capabilities",
        "name",
        "RESTRICT"
    ),
    fk!("chat_sessions", "entry_agent_id" => "agents", "id", "RESTRICT"),
    fk!("chat_sessions", "current_agent_id" => "agents", "id", "SET NULL"),
    fk!("chat_messages", "session_id" => "chat_sessions", "id", "CASCADE"),
    fk!(
        "agent_executions",
        "session_id" => "chat_sessions",
        "id",
        "CASCADE"
    ),
    fk!(
        "agent_executions",
        "current_agent_id" => "agents",
        "id",
        "SET NULL"
    ),
];

fn foreign_key_identity(
    table: &str,
    from_column: &str,
    target_table: &str,
    target_column: &str,
) -> (String, String, String, String) {
    (
        table.to_owned(),
        from_column.to_owned(),
        target_table.to_owned(),
        target_column.to_owned(),
    )
}

fn braced_block_after<'a>(source: &'a str, marker: &str) -> &'a str {
    let marker_start = source
        .find(marker)
        .unwrap_or_else(|| panic!("production source is missing {marker}"));
    let open = source[marker_start..]
        .find('{')
        .map(|offset| marker_start + offset)
        .expect("marked Rust item has a body");
    let mut depth = 0usize;
    for (offset, byte) in source.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open + 1..open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("unterminated Rust item after {marker}")
}

fn function_name(line: &str) -> Option<&str> {
    let tokens = line
        .split(|character: char| character.is_whitespace())
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let fn_index = tokens.iter().position(|token| *token == "fn")?;
    tokens
        .get(fn_index + 1)
        .map(|name| name.split('(').next().expect("function name"))
}

fn top_level_function_names(body: &str, public_only: bool) -> BTreeSet<String> {
    let mut depth = 0isize;
    let mut names = BTreeSet::new();
    for line in body.lines() {
        let trimmed = line.trim_start();
        if depth == 0
            && (!public_only || trimmed.starts_with("pub "))
            && let Some(name) = function_name(trimmed)
        {
            names.insert(name.to_owned());
        }
        depth += line.bytes().filter(|byte| *byte == b'{').count() as isize;
        depth -= line.bytes().filter(|byte| *byte == b'}').count() as isize;
    }
    names
}

fn public_struct_fields(source: &str, marker: &str) -> BTreeSet<String> {
    braced_block_after(source, marker)
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("pub ")?
                .split_once(':')
                .map(|(field, _)| field.trim().to_owned())
        })
        .collect()
}

fn associated_calls(source: &str, receiver: &str) -> BTreeSet<String> {
    source
        .match_indices(receiver)
        .filter_map(|(offset, _)| {
            let suffix = &source[offset + receiver.len()..];
            let name = suffix
                .chars()
                .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
                .collect::<String>();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

fn visit_rust_sources(directory: &Path, sources: &mut Vec<(String, String)>) {
    for entry in fs::read_dir(directory).expect("read production source directory") {
        let entry = entry.expect("read production source entry");
        let path = entry.path();
        if path.is_dir() {
            visit_rust_sources(&path, sources);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            sources.push((
                path.display().to_string(),
                fs::read_to_string(&path).expect("read production Rust source"),
            ));
        }
    }
}

fn is_tag_named_api(name: &str) -> bool {
    name.split('_')
        .any(|component| matches!(component, "tag" | "tags"))
}

#[test]
fn v4_schema_catalog_is_complete_exact_and_foundation_owned() {
    let table_names = V4_APPLICATION_TABLES
        .iter()
        .map(|row| row.table)
        .collect::<BTreeSet<_>>();
    assert_eq!(table_names.len(), V4_APPLICATION_TABLES.len());

    let foreign_key_identities = V4_FOREIGN_KEYS
        .iter()
        .map(|row| {
            (
                row.table,
                row.from_column,
                row.target_table,
                row.target_column,
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(foreign_key_identities.len(), V4_FOREIGN_KEYS.len());

    assert!(
        V4_APPLICATION_TABLES
            .iter()
            .all(|row| row.owner_phase == "Foundation" && row.activation_task == "T017")
    );
    assert!(
        V4_FOREIGN_KEYS
            .iter()
            .all(|row| row.owner_phase == "Foundation" && row.activation_task == "T017")
    );
    assert_eq!(
        V4_APPLICATION_TABLES
            .iter()
            .filter(|row| row.relationship_table)
            .map(|row| row.table)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "agent_tools",
            "agent_skills",
            "agent_capabilities",
            "workflow_nodes",
            "workflow_edges",
        ])
    );
}

#[tokio::test]
async fn fresh_v4_schema_has_every_declared_table_and_fk_and_no_extra_structure() {
    let workspace = TestWorkspace::new().expect("isolated workspace");
    let store = hivegui::datasource::Store::new(workspace.root())
        .await
        .expect("open current schema");
    // The v4 application table set is exactly the entities exposed
    // by the public Store boundary. Infrastructure tables that back
    // derived indexes / sidecar ledgers / migration metadata
    // (`meta`, FTS5 trigram virtual table + shadow tables, the
    // short-gram index, the plugin artifact operations/GC ledger)
    // are NOT part of the v4 application schema and are filtered
    // here. The v4 application table set lives in
    // `V4_APPLICATION_TABLES`; the relationships contract asserted
    // by this test is intentionally narrow.
    let infrastructure_table_predicates: &[(&str, &str)] = &[
        ("meta", "migration metadata key/value table"),
        ("search_index", "FTS5 trigram virtual table for search"),
        (
            "search_index_config",
            "FTS5 trigram virtual table shadow table",
        ),
        (
            "search_index_content",
            "FTS5 trigram virtual table shadow table",
        ),
        (
            "search_index_data",
            "FTS5 trigram virtual table shadow table",
        ),
        (
            "search_index_docsize",
            "FTS5 trigram virtual table shadow table",
        ),
        ("search_index_idx", "FTS5 trigram virtual table shadow table"),
        ("short_gram_index", "derived 1-2 character search index"),
        (
            "plugin_artifact_operations",
            "plugin artifact sidecar operations ledger",
        ),
        ("plugin_artifact_gc", "plugin artifact sidecar GC ledger"),
    ];
    let infrastructure_names: BTreeSet<&'static str> = infrastructure_table_predicates
        .iter()
        .map(|(name, _)| *name)
        .collect();
    let actual_tables = sqlx::query_scalar::<_, String>(
        "SELECT name FROM sqlite_master \
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .fetch_all(store.pool())
    .await
    .expect("read v4 application tables")
    .into_iter()
    .filter(|name| !infrastructure_names.contains(name.as_str()))
    .collect::<BTreeSet<_>>();
    let expected_tables = V4_APPLICATION_TABLES
        .iter()
        .map(|row| row.table.to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(actual_tables, expected_tables, "v4 application table set");

    let mut actual_foreign_keys = Vec::new();
    for table in &actual_tables {
        let quoted = table.replace('"', "\"\"");
        // `table` comes from the verified v4 application table list, not
        // user input, so it is safe to interpolate into the PRAGMA query.
        for row in sqlx::query(sqlx::AssertSqlSafe(format!(
            "PRAGMA foreign_key_list(\"{quoted}\")"
        )))
        .fetch_all(store.pool())
        .await
        .unwrap_or_else(|error| panic!("read {table} foreign keys: {error}"))
        {
            actual_foreign_keys.push(ActualForeignKey {
                table: table.clone(),
                from_column: row.try_get("from").expect("foreign-key source column"),
                target_table: row.try_get("table").expect("foreign-key target table"),
                target_column: row.try_get("to").expect("foreign-key target column"),
                on_delete: row
                    .try_get::<String, _>("on_delete")
                    .expect("foreign-key ON DELETE action")
                    .to_ascii_uppercase(),
            });
        }
    }

    let actual_identities = actual_foreign_keys
        .iter()
        .map(|row| {
            foreign_key_identity(
                &row.table,
                &row.from_column,
                &row.target_table,
                &row.target_column,
            )
        })
        .collect::<BTreeSet<_>>();
    let expected_identities = V4_FOREIGN_KEYS
        .iter()
        .map(|row| {
            foreign_key_identity(
                row.table,
                row.from_column,
                row.target_table,
                row.target_column,
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(actual_identities, expected_identities, "v4 FK edge set");

    for expected in V4_FOREIGN_KEYS {
        let actual = actual_foreign_keys
            .iter()
            .find(|actual| {
                actual.table == expected.table
                    && actual.from_column == expected.from_column
                    && actual.target_table == expected.target_table
                    && actual.target_column == expected.target_column
            })
            .expect("declared FK exists");
        assert_eq!(
            actual.on_delete, expected.on_delete,
            "ON DELETE for {expected:?}"
        );
    }
}

#[test]
fn tag_has_only_its_typed_crud_surface_and_no_relationship_entry_point() {
    fn assert_exact_tag_shape(tag: Tag) {
        let Tag {
            id: _,
            name: _,
            color: _,
            created_at: _,
        } = tag;
    }
    let _: fn(Tag) = assert_exact_tag_shape;

    assert_eq!(
        public_struct_fields(ENTITY_STORE_SOURCE, "pub struct Tag"),
        BTreeSet::from([
            "id".to_owned(),
            "name".to_owned(),
            "color".to_owned(),
            "created_at".to_owned(),
        ])
    );
    assert_eq!(
        top_level_function_names(braced_block_after(ENTITY_STORE_SOURCE, "impl Tag"), true),
        ["list", "count", "get", "create", "update", "delete"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );
    assert_eq!(
        top_level_function_names(braced_block_after(TAG_VIEW_SOURCE, "impl TagView"), false),
        [
            "new",
            "load_tags",
            "show_add_form",
            "show_edit_form",
            "hide_form",
            "save_tag",
            "set_color",
            "delete_tag",
            "prev_page",
            "next_page",
            "on_key_down",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );
    assert_eq!(
        associated_calls(TAG_VIEW_SOURCE, "Tag::"),
        ["list", "count", "create", "update", "delete"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = Vec::new();
    visit_rust_sources(&manifest_dir.join("src/datasource"), &mut sources);
    visit_rust_sources(&manifest_dir.join("src/ui"), &mut sources);
    let tag_named_public_apis = sources
        .iter()
        .flat_map(|(path, source)| {
            source.lines().filter_map(move |line| {
                let trimmed = line.trim_start();
                if !trimmed.starts_with("pub ") {
                    return None;
                }
                let name = function_name(trimmed)?;
                is_tag_named_api(name).then(|| format!("{path}::{name}"))
            })
        })
        .collect::<BTreeSet<_>>();
    assert!(
        tag_named_public_apis.is_empty(),
        "Tag must not gain a cross-entity public relationship API: {tag_named_public_apis:?}"
    );
}
