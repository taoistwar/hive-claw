//! T011 Red contract for the storage-independent Workflow DAG.
//!
//! These tests intentionally target the public API owned by T020.  They must
//! be reviewed before they are run; the current module is still a documented
//! skeleton, so the first approved Red is expected to be a missing-API compile
//! failure rather than an assertion failure.

use std::collections::{BTreeMap, BTreeSet};

use hive_runtime_core::workflow::{
    LayerNodeOutcome, NodeFailure, NodeType, WorkflowEdge, WorkflowGraph, WorkflowNode,
    WorkflowValidationKind,
};

fn node(key: &str, node_type: NodeType) -> WorkflowNode {
    WorkflowNode {
        node_key: key.to_owned(),
        node_type,
        function_id: (node_type == NodeType::Function).then(|| format!("fn-{key}")),
    }
}

fn edge(source: &str, target: &str) -> WorkflowEdge {
    WorkflowEdge {
        source: source.to_owned(),
        target: target.to_owned(),
    }
}

fn graph(nodes: Vec<WorkflowNode>, edges: Vec<WorkflowEdge>) -> WorkflowGraph {
    WorkflowGraph::new(nodes, edges)
}

#[test]
fn node_types_use_only_the_four_stable_persisted_values() {
    let supported = [
        (NodeType::Start, "start_node"),
        (NodeType::End, "end_node"),
        (NodeType::Function, "function_node"),
        (NodeType::GenerateAnswer, "generate_answer_node"),
    ];

    for (node_type, persisted) in supported {
        assert_eq!(node_type.as_str(), persisted);
        assert_eq!(NodeType::parse(persisted), Ok(node_type));
    }

    for rejected_alias in ["start", "end", "function", "generate_answer"] {
        let error =
            NodeType::parse(rejected_alias).expect_err("short aliases are read/write errors");
        assert_eq!(error.field(), "node_type");
        assert_eq!(error.reason(), "unsupported_value");
        assert!(!error.to_string().contains(rejected_alias));
    }
}

#[test]
fn graph_validation_rejects_each_structural_violation() {
    let cases = [
        (
            "duplicate node key",
            graph(
                vec![
                    node("start", NodeType::Start),
                    node("same", NodeType::Function),
                    node("same", NodeType::End),
                ],
                vec![],
            ),
            WorkflowValidationKind::DuplicateNodeKey,
        ),
        (
            "missing edge endpoint",
            graph(
                vec![node("start", NodeType::Start), node("end", NodeType::End)],
                vec![edge("start", "missing")],
            ),
            WorkflowValidationKind::MissingEndpoint,
        ),
        (
            "self edge",
            graph(
                vec![node("start", NodeType::Start), node("end", NodeType::End)],
                vec![edge("start", "start"), edge("start", "end")],
            ),
            WorkflowValidationKind::SelfEdge,
        ),
        (
            "cycle",
            graph(
                vec![
                    node("start", NodeType::Start),
                    node("a", NodeType::Function),
                    node("b", NodeType::Function),
                    node("end", NodeType::End),
                ],
                vec![
                    edge("start", "a"),
                    edge("a", "b"),
                    edge("b", "a"),
                    edge("b", "end"),
                ],
            ),
            WorkflowValidationKind::Cycle,
        ),
        (
            "no start",
            graph(vec![node("end", NodeType::End)], vec![]),
            WorkflowValidationKind::StartCount,
        ),
        (
            "multiple starts",
            graph(
                vec![
                    node("start-a", NodeType::Start),
                    node("start-b", NodeType::Start),
                    node("end", NodeType::End),
                ],
                vec![edge("start-a", "end"), edge("start-b", "end")],
            ),
            WorkflowValidationKind::StartCount,
        ),
        (
            "no reachable end",
            graph(
                vec![
                    node("start", NodeType::Start),
                    node("work", NodeType::Function),
                    node("end", NodeType::End),
                ],
                vec![edge("start", "work")],
            ),
            WorkflowValidationKind::UnreachableEnd,
        ),
    ];

    for (name, graph, expected_kind) in cases {
        let error = graph.validate().expect_err(name);
        assert_eq!(error.kind(), expected_kind, "case: {name}");
    }
}

#[test]
fn validated_graph_returns_deterministic_parallel_layers() {
    let graph = graph(
        vec![
            node("end", NodeType::End),
            node("work-b", NodeType::Function),
            node("start", NodeType::Start),
            node("work-a", NodeType::Function),
        ],
        vec![
            edge("work-b", "end"),
            edge("start", "work-b"),
            edge("work-a", "end"),
            edge("start", "work-a"),
        ],
    );

    let validated = graph.validate().expect("valid diamond graph");
    assert_eq!(
        validated.topological_layers(),
        vec![
            vec!["start".to_owned()],
            vec!["work-a".to_owned(), "work-b".to_owned()],
            vec!["end".to_owned()],
        ]
    );
}

#[test]
fn fail_fast_waits_for_the_started_layer_and_chooses_smallest_failed_node_key() {
    let graph = graph(
        vec![
            node("start", NodeType::Start),
            node("work-a", NodeType::Function),
            node("work-b", NodeType::Function),
            node("work-c", NodeType::Function),
            node("end", NodeType::End),
        ],
        vec![
            edge("start", "work-a"),
            edge("start", "work-b"),
            edge("start", "work-c"),
            edge("work-a", "end"),
            edge("work-b", "end"),
            edge("work-c", "end"),
        ],
    )
    .validate()
    .expect("valid diamond graph");

    let completed_before = BTreeSet::from(["start".to_owned()]);
    // Deliberately insert the lexically larger failure first. Completion of
    // work-b must still be retained and the primary failure must be stable.
    let current_layer = BTreeMap::from([
        (
            "work-c".to_owned(),
            LayerNodeOutcome::Failed(NodeFailure {
                node_key: "work-c".to_owned(),
                kind: "plugin_timeout".to_owned(),
                elapsed_ms: 40,
            }),
        ),
        ("work-b".to_owned(), LayerNodeOutcome::Completed),
        (
            "work-a".to_owned(),
            LayerNodeOutcome::Failed(NodeFailure {
                node_key: "work-a".to_owned(),
                kind: "capability_denied".to_owned(),
                elapsed_ms: 25,
            }),
        ),
    ]);

    let summary = graph.summarize_layer(&completed_before, &current_layer);
    assert!(summary.stop_before_next_layer);
    let primary = summary
        .primary_failure
        .as_ref()
        .expect("a failed layer has a primary failure");
    assert_eq!(primary.node_key, "work-a");
    assert_eq!(primary.kind, "capability_denied");
    assert_eq!(primary.elapsed_ms, 25);
    assert_eq!(
        summary.failed_nodes,
        vec!["work-a".to_owned(), "work-c".to_owned()]
    );
    assert_eq!(summary.unstarted_nodes, vec!["end".to_owned()]);
    assert_eq!(
        summary.completed_nodes,
        vec!["start".to_owned(), "work-b".to_owned()]
    );
}

#[test]
fn a_successful_parallel_layer_allows_only_its_dependants_to_be_scheduled() {
    let graph = graph(
        vec![
            node("start", NodeType::Start),
            node("work-a", NodeType::Function),
            node("work-b", NodeType::Function),
            node("end", NodeType::End),
        ],
        vec![
            edge("start", "work-a"),
            edge("start", "work-b"),
            edge("work-a", "end"),
            edge("work-b", "end"),
        ],
    )
    .validate()
    .expect("valid diamond graph");

    let completed_before = BTreeSet::from(["start".to_owned()]);
    let current_layer = BTreeMap::from([
        ("work-a".to_owned(), LayerNodeOutcome::Completed),
        ("work-b".to_owned(), LayerNodeOutcome::Completed),
    ]);

    let summary = graph.summarize_layer(&completed_before, &current_layer);
    assert!(!summary.stop_before_next_layer);
    assert!(summary.primary_failure.is_none());
    assert_eq!(summary.ready_next, vec!["end".to_owned()]);
    assert!(summary.unstarted_nodes.is_empty());
}
