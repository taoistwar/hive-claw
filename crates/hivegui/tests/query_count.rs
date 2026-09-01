//! Red query-count contract. A larger fixture may change rows returned, never
//! the number of round trips used to load one aggregate.

mod support;

use std::collections::{BTreeMap, BTreeSet};

use hivegui::datasource::{
    query_count::{
        QueryCountFailureKind, QueryCountObserver, QueryCountSample, QueryCountVerdict,
        evaluate_query_count, production_query_count_catalog,
    },
    store::{Store, StoreOpenOptions},
};
use support::TestWorkspace;

const OWNER_PHASE: &str = "Foundation";
const APPROVAL_TASK: &str = "T012";

#[test]
fn query_count_catalog_has_explicit_owner_phase_and_activation_metadata() {
    let catalog = production_query_count_catalog();
    let foundation = catalog
        .iter()
        .filter(|contract| contract.owner_phase == OWNER_PHASE)
        .collect::<Vec<_>>();
    assert!(!foundation.is_empty());

    let mut ids = BTreeSet::new();
    for contract in foundation {
        assert!(
            ids.insert(contract.id),
            "duplicate query-count id {}",
            contract.id
        );
        assert!(!contract.owner_phase.trim().is_empty());
        assert!(!contract.activation_task.trim().is_empty());
        assert!(contract.maximum_queries > 0);

        assert!(
            contract.active,
            "Foundation contract {} is deferred",
            contract.id
        );
        assert!(
            matches!(contract.activation_task, "T012" | "T028"),
            "Foundation contract {} is owned by {}",
            contract.id,
            contract.activation_task
        );
    }

    let expected_future_owners = BTreeMap::from([
        (
            "category.tree",
            ("US6", "T060", 1, &["category.tree_all"][..]),
        ),
        (
            "workflow.graph",
            (
                "US10",
                "T090",
                3,
                &[
                    "workflow.by_id",
                    "workflow.nodes_by_workflow",
                    "workflow.edges_by_workflow",
                ][..],
            ),
        ),
        (
            "agent.resource_snapshot",
            (
                "US13",
                "T115",
                4,
                &[
                    "agent.by_id",
                    "agent.tools_by_agent",
                    "agent.skills_by_agent",
                    "agent.capabilities_by_agent",
                ][..],
            ),
        ),
        (
            "conversation.session_bundle",
            (
                "US13",
                "T117",
                2,
                &[
                    "conversation.messages_by_session",
                    "conversation.executions_by_session",
                ][..],
            ),
        ),
    ]);
    for (id, (phase, task, maximum_queries, expected_query_ids)) in expected_future_owners {
        assert!(!id.trim().is_empty());
        assert_ne!(phase, OWNER_PHASE);
        assert_ne!(task, APPROVAL_TASK);
        assert!(maximum_queries > 0);
        assert!(!expected_query_ids.is_empty());
    }
}

#[test]
fn constant_query_count_passes_when_fixture_cardinality_grows() {
    let small = QueryCountSample {
        scenario_id: "workflow.graph",
        fixture_items: 1,
        total_queries: 3,
        query_ids: vec![
            "workflow.by_id",
            "workflow.nodes_by_workflow",
            "workflow.edges_by_workflow",
        ],
    };
    let large = QueryCountSample {
        scenario_id: "workflow.graph",
        fixture_items: 100,
        total_queries: 3,
        query_ids: vec![
            "workflow.by_id",
            "workflow.nodes_by_workflow",
            "workflow.edges_by_workflow",
        ],
    };

    let verdict = evaluate_query_count(3, &small, &large);

    assert!(verdict.is_accepted(), "{:?}", verdict.failures());
    assert_eq!(verdict.query_growth(), 0);
}

#[test]
fn any_linear_or_cardinality_dependent_query_growth_is_rejected_as_n_plus_one() {
    let matrices = [
        ("category.tree", 1, 2, 100, 101),
        ("workflow.graph", 1, 3, 100, 102),
        ("agent.resource_snapshot", 1, 4, 100, 103),
        ("conversation.session_bundle", 2, 3, 200, 201),
    ];

    for (scenario, small_items, small_queries, large_items, large_queries) in matrices {
        let small = QueryCountSample {
            scenario_id: scenario,
            fixture_items: small_items,
            total_queries: small_queries,
            query_ids: vec!["aggregate.root"; small_queries],
        };
        let large = QueryCountSample {
            scenario_id: scenario,
            fixture_items: large_items,
            total_queries: large_queries,
            query_ids: vec!["aggregate.child_by_id"; large_queries],
        };

        let verdict = evaluate_query_count(small_queries, &small, &large);

        assert!(
            verdict.has_failure(QueryCountFailureKind::NPlusOne),
            "{scenario}: {:?}",
            verdict.failures()
        );
        assert!(verdict.query_growth() > 0);
    }
}

#[test]
fn exceeding_fixed_budget_fails_even_when_both_fixture_sizes_match() {
    let small = QueryCountSample {
        scenario_id: "agent.resource_snapshot",
        fixture_items: 1,
        total_queries: 5,
        query_ids: vec!["unexpected.extra"; 5],
    };
    let large = QueryCountSample {
        scenario_id: "agent.resource_snapshot",
        fixture_items: 100,
        total_queries: 5,
        query_ids: vec!["unexpected.extra"; 5],
    };

    let verdict = evaluate_query_count(4, &small, &large);

    assert!(verdict.has_failure(QueryCountFailureKind::BudgetExceeded {
        maximum: 4,
        actual: 5,
    }));
}

#[test]
fn missing_or_mismatched_samples_are_indeterminate_and_blocking() {
    let missing = QueryCountSample {
        scenario_id: "workflow.graph",
        fixture_items: 0,
        total_queries: 0,
        query_ids: vec![],
    };
    let valid = QueryCountSample {
        scenario_id: "workflow.graph",
        fixture_items: 100,
        total_queries: 3,
        query_ids: vec!["workflow.by_id", "workflow.nodes", "workflow.edges"],
    };
    let verdict = evaluate_query_count(3, &missing, &valid);
    assert!(verdict.has_failure(QueryCountFailureKind::Indeterminate));

    let wrong_scenario = QueryCountSample {
        scenario_id: "category.tree",
        ..valid.clone()
    };
    let baseline = QueryCountSample {
        fixture_items: 1,
        ..valid
    };
    let verdict = evaluate_query_count(3, &baseline, &wrong_scenario);
    assert!(verdict.has_failure(QueryCountFailureKind::ScenarioMismatch));
}

#[test]
fn observer_counts_checked_queries_by_operation_scope_without_sql_text() {
    let observer = QueryCountObserver::new();
    let scope = observer.start_scope("workflow.graph", 100);
    observer.record_checked_query("workflow.by_id");
    observer.record_checked_query("workflow.nodes_by_workflow");
    observer.record_checked_query("workflow.edges_by_workflow");
    let sample = scope.finish();

    assert_eq!(sample.scenario_id, "workflow.graph");
    assert_eq!(sample.fixture_items, 100);
    assert_eq!(sample.total_queries, 3);
    assert_eq!(
        sample.query_ids,
        [
            "workflow.by_id",
            "workflow.nodes_by_workflow",
            "workflow.edges_by_workflow"
        ]
    );
}

#[tokio::test]
async fn real_foundation_store_routes_queries_through_the_public_observer() {
    let contract = production_query_count_catalog()
        .iter()
        .find(|contract| contract.id == "foundation.store.schema_version")
        .expect("Foundation Store query-count contract");
    assert_eq!(contract.owner_phase, OWNER_PHASE);
    assert!(contract.active);

    let workspace = TestWorkspace::new().expect("isolated workspace");
    let observer = QueryCountObserver::new();
    let store = Store::open_local(
        StoreOpenOptions::new(workspace.database_path(), workspace.plugin_root())
            .with_query_count_observer(observer.clone()),
    )
    .await
    .expect("open real Store with query observer");

    let small_scope = observer.start_scope(contract.id, 1);
    assert_eq!(store.schema_version().await.unwrap(), 4);
    let small = small_scope.finish();

    let large_scope = observer.start_scope(contract.id, 100);
    assert_eq!(store.schema_version().await.unwrap(), 4);
    let large = large_scope.finish();

    let verdict: QueryCountVerdict = evaluate_query_count(contract.maximum_queries, &small, &large);
    assert!(verdict.is_accepted(), "{:?}", verdict.failures());
}
