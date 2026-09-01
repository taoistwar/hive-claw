//! T013 Red contract for public write validation and stable error envelopes.
//!
//! The draft catalog is deliberately complete, but each row names the reviewer
//! gate that activates its behavioural tests. Foundation assertions filter to
//! `owner_phase=Foundation`; future rows remain reviewable test data and cannot
//! block or be counted in the Foundation Green result. Later user-story test
//! batches activate their own rows without `#[ignore]` or pretending that an
//! unimplemented entity is covered.

use std::collections::BTreeSet;

use hivegui::datasource::{
    Store,
    validation::{
        PublicBoundaryError, PublicErrorEnvelope, public_conflict_catalog, public_enum_catalog,
        public_query_dto_fields, public_query_field_catalog, public_write_dto_fields,
        public_write_field_catalog,
    },
};

#[derive(Clone, Copy)]
struct EntityFields {
    entity: &'static str,
    owner_phase: &'static str,
    activation_task: &'static str,
    fields: &'static [&'static str],
}

const ENTITY_FIELDS: &[EntityFields] = &[
    EntityFields {
        entity: "data_source",
        owner_phase: "US2",
        activation_task: "T037",
        fields: &["name", "host", "port", "username", "password"],
    },
    EntityFields {
        entity: "global_config",
        owner_phase: "US3",
        activation_task: "T043",
        fields: &["name", "key", "config_type", "data"],
    },
    EntityFields {
        entity: "llm_preset",
        owner_phase: "US4",
        activation_task: "T050",
        fields: &[
            "name",
            "description",
            "is_default",
            "max_tokens",
            "temperature",
        ],
    },
    EntityFields {
        entity: "llm_provider",
        owner_phase: "US4",
        activation_task: "T050",
        fields: &["name", "category", "base_url", "token", "token_env"],
    },
    EntityFields {
        entity: "model",
        owner_phase: "US4",
        activation_task: "T050",
        fields: &["name", "preset_id", "provider_id", "priority"],
    },
    EntityFields {
        entity: "tag",
        owner_phase: "US5",
        activation_task: "T057",
        fields: &["name", "color"],
    },
    EntityFields {
        entity: "category",
        owner_phase: "US6",
        activation_task: "T062",
        fields: &["parent_id", "name", "slug", "description"],
    },
    EntityFields {
        entity: "capability",
        owner_phase: "US7",
        activation_task: "T068",
        fields: &["name", "description", "is_dangerous", "category_id"],
    },
    EntityFields {
        entity: "plugin",
        owner_phase: "US8",
        activation_task: "T076",
        fields: &[
            "identifier",
            "name",
            "description",
            "manifest",
            "runtime",
            "version",
            "author",
            "repository_url",
            "s3_key",
            "sha256",
            "size_bytes",
            "category_id",
            "timeout_ms",
            "memory_limit_mb",
            "output_limit_bytes",
        ],
    },
    EntityFields {
        entity: "function",
        owner_phase: "US9",
        activation_task: "T086",
        fields: &[
            "identifier",
            "name",
            "description",
            "kind",
            "input_schema",
            "output_schema",
            "plugin_id",
            "plugin_export",
            "category_id",
            "required_capabilities",
        ],
    },
    EntityFields {
        entity: "workflow",
        owner_phase: "US10",
        activation_task: "T094",
        fields: &[
            "identifier",
            "name",
            "description",
            "timeout_ms",
            "category_id",
            "input_schema",
            "start_description",
            "output_schema",
            "required_capabilities",
        ],
    },
    EntityFields {
        entity: "workflow_node",
        owner_phase: "US10",
        activation_task: "T094",
        fields: &[
            "workflow_id",
            "node_key",
            "node_type",
            "function_id",
            "position_x",
            "position_y",
            "node_config",
        ],
    },
    EntityFields {
        entity: "workflow_edge",
        owner_phase: "US10",
        activation_task: "T094",
        fields: &["workflow_id", "src_node_key", "dst_node_key", "mapping"],
    },
    EntityFields {
        entity: "tool",
        owner_phase: "US11",
        activation_task: "T104",
        fields: &[
            "identifier",
            "name",
            "description",
            "kind",
            "source",
            "is_always",
            "function_id",
            "workflow_id",
            "input_schema",
            "output_schema",
            "category_id",
            "required_capabilities",
        ],
    },
    EntityFields {
        entity: "skill",
        owner_phase: "US12",
        activation_task: "T111",
        fields: &[
            "identifier",
            "name",
            "description",
            "frontmatter",
            "content",
            "source",
            "is_always",
            "category_id",
            "required_capabilities",
        ],
    },
    EntityFields {
        entity: "agent",
        owner_phase: "US13",
        activation_task: "T123",
        fields: &[
            "identifier",
            "name",
            "description",
            "system_prompt",
            "parent_agent_id",
            "is_default",
            "model_preset",
        ],
    },
    EntityFields {
        entity: "start_session",
        owner_phase: "US13",
        activation_task: "T123",
        fields: &["user_message"],
    },
    EntityFields {
        entity: "continue_session",
        owner_phase: "US13",
        activation_task: "T123",
        fields: &["session_id", "user_message"],
    },
    EntityFields {
        entity: "stop_execution",
        owner_phase: "US13",
        activation_task: "T123",
        fields: &["execution_id"],
    },
    EntityFields {
        entity: "delete_session",
        owner_phase: "US13",
        activation_task: "T123",
        fields: &["session_id"],
    },
    EntityFields {
        entity: "clear_history",
        owner_phase: "US13",
        activation_task: "T123",
        fields: &["retention_filter"],
    },
];

const QUERY_FIELDS: &[EntityFields] = &[
    EntityFields {
        entity: "paged_list",
        owner_phase: "Foundation",
        activation_task: "T017",
        fields: &["search", "page", "page_size"],
    },
    EntityFields {
        entity: "category_search",
        owner_phase: "US6",
        activation_task: "T062",
        fields: &["search"],
    },
    EntityFields {
        entity: "mysql_table_query",
        owner_phase: "US2",
        activation_task: "T037",
        fields: &[
            "database", "table", "filters", "order_by", "limit", "offset",
        ],
    },
];

fn expected_field_rows_for_phase(
    phase: &str,
) -> BTreeSet<(&'static str, &'static str, &'static str, &'static str)> {
    ENTITY_FIELDS
        .iter()
        .filter(|entity| entity.owner_phase == phase)
        .flat_map(|entity| {
            entity.fields.iter().map(|field| {
                (
                    entity.entity,
                    *field,
                    entity.owner_phase,
                    entity.activation_task,
                )
            })
        })
        .collect()
}

fn assert_public_error(error: &anyhow::Error, expected: PublicErrorEnvelope) {
    let public = error
        .downcast_ref::<PublicBoundaryError>()
        .expect("public boundary must return PublicBoundaryError, not raw SQL/anyhow text");
    assert_eq!(public.envelope(), &expected);
}

fn assert_foundation_gate(owner_phase: &str, activation_task: &str) {
    assert_eq!(owner_phase, "Foundation");
    assert_eq!(activation_task, "T017");
}

#[test]
fn foundation_write_field_rows_are_the_only_rows_activated_by_t017() {
    let actual = public_write_field_catalog()
        .iter()
        .filter(|field| field.owner_phase == "Foundation")
        .map(|field| {
            (
                field.entity,
                field.field,
                field.owner_phase,
                field.activation_task,
            )
        })
        .collect::<BTreeSet<_>>();

    assert_eq!(actual, expected_field_rows_for_phase("Foundation"));
    assert!(
        actual
            .iter()
            .all(|(_, _, owner, task)| { !owner.trim().is_empty() && task.starts_with('T') })
    );
    for (_, _, owner, task) in &actual {
        assert_foundation_gate(owner, task);
    }
}

#[test]
fn foundation_catalog_fields_exactly_match_its_public_write_dto_descriptors() {
    for expected in ENTITY_FIELDS
        .iter()
        .filter(|entity| entity.owner_phase == "Foundation")
    {
        let catalog_fields = public_write_field_catalog()
            .iter()
            .filter(|row| row.entity == expected.entity)
            .map(|row| row.field)
            .collect::<BTreeSet<_>>();
        let dto_fields = public_write_dto_fields(expected.entity)
            .unwrap_or_else(|| panic!("missing public DTO descriptor for {}", expected.entity))
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();

        assert_eq!(catalog_fields, dto_fields, "entity: {}", expected.entity);
        assert_eq!(dto_fields, expected.fields.iter().copied().collect());
    }
}

#[test]
fn pagination_search_and_mysql_query_inputs_have_their_own_exact_catalog() {
    let actual = public_query_field_catalog()
        .iter()
        .filter(|field| field.owner_phase == "Foundation")
        .map(|field| {
            (
                field.entity,
                field.field,
                field.owner_phase,
                field.activation_task,
            )
        })
        .collect::<BTreeSet<_>>();
    let expected = QUERY_FIELDS
        .iter()
        .filter(|entity| entity.owner_phase == "Foundation")
        .flat_map(|entity| {
            entity.fields.iter().map(|field| {
                (
                    entity.entity,
                    *field,
                    entity.owner_phase,
                    entity.activation_task,
                )
            })
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected);
    for (_, _, owner, task) in &actual {
        assert_foundation_gate(owner, task);
    }

    for expected in QUERY_FIELDS
        .iter()
        .filter(|entity| entity.owner_phase == "Foundation")
    {
        assert_eq!(
            public_query_dto_fields(expected.entity)
                .unwrap_or_else(|| panic!("missing query DTO descriptor for {}", expected.entity))
                .iter()
                .copied()
                .collect::<BTreeSet<_>>(),
            expected.fields.iter().copied().collect(),
            "query DTO: {}",
            expected.entity
        );
    }
}

#[tokio::test]
async fn public_pagination_and_search_enforce_the_complete_foundation_query_boundary() {
    let directory = tempfile::tempdir().expect("temporary Store directory");
    let store = Store::new(directory.path()).await.expect("open Store");

    for (search, page, page_size, field, reason) in [
        ("".to_owned(), 0, 20, "page", "out_of_range"),
        ("".to_owned(), 1, 19, "page_size", "fixed_value_required"),
        ("".to_owned(), 1, 21, "page_size", "fixed_value_required"),
        ("x".repeat(256), 1, 20, "search", "too_long"),
        (
            "contains\0nul".to_owned(),
            1,
            20,
            "search",
            "control_character",
        ),
    ] {
        let error = store
            .list_global_configs(&search, page, page_size)
            .await
            .expect_err("invalid public query input must return an error envelope");
        assert_public_error(
            &error,
            PublicErrorEnvelope::InvalidInput {
                field: field.into(),
                reason: reason.into(),
            },
        );
    }

    let (rows, total) = store
        .list_global_configs("", 1, 20)
        .await
        .expect("the fixed page size remains accepted");
    assert!(rows.is_empty());
    assert_eq!(total, 0);
}

#[test]
fn conflict_catalog_is_complete_and_separates_value_from_reference_shapes() {
    let expected = BTreeSet::from([
        ("global_config", "key", "duplicate", "value", "US3", "T043"),
        ("plugin", "identifier", "duplicate", "value", "US8", "T076"),
        (
            "function",
            "identifier",
            "duplicate",
            "value",
            "US9",
            "T086",
        ),
        (
            "workflow",
            "identifier",
            "duplicate",
            "value",
            "US10",
            "T094",
        ),
        (
            "workflow",
            "id",
            "referenced_by_tool",
            "references",
            "US10",
            "T094",
        ),
        ("tool", "identifier", "duplicate", "value", "US11", "T104"),
        ("skill", "identifier", "duplicate", "value", "US12", "T111"),
        ("agent", "identifier", "duplicate", "value", "US13", "T123"),
        (
            "category",
            "id",
            "has_children",
            "references",
            "US6",
            "T062",
        ),
        (
            "llm_provider",
            "id",
            "referenced_by_model",
            "references",
            "US4",
            "T050",
        ),
        (
            "plugin",
            "id",
            "referenced_by_function",
            "references",
            "US8",
            "T076",
        ),
        (
            "function",
            "id",
            "referenced_by_workflow_node",
            "references",
            "US9",
            "T086",
        ),
        (
            "function",
            "id",
            "referenced_by_tool",
            "references",
            "US9",
            "T086",
        ),
        (
            "agent",
            "is_default",
            "replacement_required",
            "references",
            "US13",
            "T123",
        ),
        (
            "llm_preset",
            "name",
            "referenced_by_agent",
            "references",
            "US4",
            "T050",
        ),
    ]);

    let actual = public_conflict_catalog()
        .iter()
        .filter(|row| row.owner_phase == "Foundation")
        .map(|row| {
            (
                row.entity,
                row.field,
                row.reason,
                row.shape.as_str(),
                row.owner_phase,
                row.activation_task,
            )
        })
        .collect::<BTreeSet<_>>();
    let expected = expected
        .into_iter()
        .filter(|(_, _, _, _, owner, _)| *owner == "Foundation")
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected);
    for (_, _, _, _, owner, task) in &actual {
        assert_foundation_gate(owner, task);
    }
    assert!(
        actual
            .iter()
            .all(|(_, _, _, shape, _, _)| { matches!(*shape, "value" | "references") })
    );
}

#[test]
fn stable_enum_and_interpreter_sensitive_fields_are_present_in_the_catalog() {
    let catalog = ENTITY_FIELDS
        .iter()
        .flat_map(|entity| {
            entity.fields.iter().map(|field| {
                (
                    entity.entity,
                    *field,
                    entity.owner_phase,
                    entity.activation_task,
                )
            })
        })
        .collect::<BTreeSet<_>>();
    for required in [
        ("function", "kind", "US9", "T086"),
        ("function", "plugin_export", "US9", "T086"),
        ("workflow_node", "node_type", "US10", "T094"),
        ("workflow_node", "position_x", "US10", "T094"),
        ("workflow_edge", "mapping", "US10", "T094"),
        ("tool", "kind", "US11", "T104"),
        ("tool", "source", "US11", "T104"),
        ("skill", "source", "US12", "T111"),
        ("plugin", "manifest", "US8", "T076"),
        ("function", "required_capabilities", "US9", "T086"),
        ("start_session", "user_message", "US13", "T123"),
        ("clear_history", "retention_filter", "US13", "T123"),
    ] {
        assert!(
            catalog.contains(&required),
            "missing high-risk field {required:?}"
        );
    }
}

#[test]
fn stable_enum_catalog_has_no_legacy_or_short_aliases() {
    let expected = BTreeSet::from([
        ("plugin", "runtime", vec!["extism"], "US8", "T076"),
        (
            "function",
            "kind",
            vec!["builtin", "custom", "placeholder"],
            "US9",
            "T086",
        ),
        (
            "function",
            "builtin_identifier",
            vec![
                "format_template",
                "json_parse",
                "json_stringify",
                "text_regex_match",
            ],
            "US9",
            "T086",
        ),
        (
            "workflow_node",
            "node_type",
            vec![
                "start_node",
                "end_node",
                "function_node",
                "generate_answer_node",
            ],
            "US10",
            "T094",
        ),
        (
            "tool",
            "kind",
            vec!["function-wrap", "workflow-wrap"],
            "US11",
            "T104",
        ),
        (
            "tool",
            "source",
            vec!["workspace", "builtin"],
            "US11",
            "T104",
        ),
        (
            "skill",
            "source",
            vec!["workspace", "builtin"],
            "US12",
            "T111",
        ),
    ]);
    let actual = public_enum_catalog()
        .iter()
        .filter(|row| row.owner_phase == "Foundation")
        .map(|row| {
            assert!(
                !row.allow_aliases,
                "aliases are forbidden for {}.{}",
                row.entity, row.field
            );
            (
                row.entity,
                row.field,
                row.values.to_vec(),
                row.owner_phase,
                row.activation_task,
            )
        })
        .collect::<BTreeSet<_>>();
    let foundation_expected = expected
        .iter()
        .filter(|(_, _, _, owner, _)| *owner == "Foundation")
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, foundation_expected);

    let flattened_values = expected
        .iter()
        .flat_map(|(_, _, values, _, _)| values.iter().copied())
        .collect::<BTreeSet<_>>();
    for forbidden in [
        "1",
        "2",
        "3",
        "start",
        "end",
        "function",
        "generate_answer",
        "format.template",
        "json.parse",
        "json.stringify",
        "text.regex_match",
    ] {
        assert!(
            !flattened_values.contains(forbidden),
            "legacy alias leaked: {forbidden}"
        );
    }
}
