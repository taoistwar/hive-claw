//! `query_count` — public boundary that validates the query budget for
//! a given operation. The Foundation owns the contract that a
//! pre-aggregate must use a constant number of round trips regardless
//! of fixture size, and that the production catalog must list every
//! owner phase and activation task explicitly.

#![warn(missing_docs)]

use std::sync::{Arc, Mutex, Weak};

/// Failure kinds emitted by the query-count contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryCountFailureKind {
    /// Sample size grew proportionally to fixture cardinality (N+1).
    NPlusOne,
    /// Both samples exceeded the fixed maximum budget.
    BudgetExceeded {
        /// Maximum budget allowed by the contract.
        maximum: usize,
        /// Actual number of queries observed.
        actual: usize,
    },
    /// Sample is missing or zero-cardinality and therefore cannot
    /// support a verdict.
    Indeterminate,
    /// Samples came from different scenarios.
    ScenarioMismatch,
}

/// One sample produced by [`QueryCountObserver::start_scope`].
#[derive(Debug, Clone)]
pub struct QueryCountSample {
    /// Stable scenario identifier.
    pub scenario_id: &'static str,
    /// Number of fixture items the operation processed.
    pub fixture_items: usize,
    /// Total number of queries recorded for the scenario.
    pub total_queries: usize,
    /// Per-query identifiers recorded in order.
    pub query_ids: Vec<&'static str>,
}

/// Shared per-scope query-id accumulator. The observer holds a
/// `Weak` reference so it can forward global records to the most
/// recently started scope. The scope owns a strong `Arc` so the
/// accumulator survives until `finish` consumes it.
#[derive(Debug, Default)]
struct ScopeAccumulator {
    query_ids: Vec<&'static str>,
}

/// Active scope that records per-query identifiers. Drop the guard
/// without calling [`QueryCountScope::finish`] to discard the
/// collected sample.
pub struct QueryCountScope {
    observer: QueryCountObserver,
    scenario_id: &'static str,
    fixture_items: usize,
    accumulator: Arc<Mutex<ScopeAccumulator>>,
    finished: bool,
}

impl QueryCountScope {
    /// Record one query identifier. Order is preserved.
    pub fn record(&mut self, query_id: &'static str) {
        if self.finished {
            return;
        }
        if let Ok(mut acc) = self.accumulator.lock() {
            acc.query_ids.push(query_id);
        }
    }

    /// Finalise the sample and return it for [`evaluate_query_count`].
    pub fn finish(mut self) -> QueryCountSample {
        self.finished = true;
        // Detach from the observer's active-scope tracker so a
        // subsequent `start_scope` does not see this scope as
        // current. We do this by clearing the observer's
        // active-scope weak pointer if it still points to us.
        self.observer
            .clear_active_scope_if_matches(&self.accumulator);
        let query_ids = if let Ok(acc) = self.accumulator.lock() {
            acc.query_ids.clone()
        } else {
            Vec::new()
        };
        QueryCountSample {
            scenario_id: self.scenario_id,
            fixture_items: self.fixture_items,
            total_queries: query_ids.len(),
            query_ids,
        }
    }
}

impl Drop for QueryCountScope {
    fn drop(&mut self) {
        if !self.finished {
            self.observer
                .clear_active_scope_if_matches(&self.accumulator);
        }
    }
}

/// Thread-safe counter that produces [`QueryCountSample`] values.
#[derive(Debug, Clone, Default)]
pub struct QueryCountObserver {
    inner: Arc<Mutex<ObserverState>>,
}

#[derive(Debug, Default)]
struct ObserverState {
    last_scenario: Option<&'static str>,
    active_scope: Option<Weak<Mutex<ScopeAccumulator>>>,
}

impl QueryCountObserver {
    /// Create a fresh observer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a new scope for `scenario_id` covering `fixture_items`.
    /// The new scope becomes the observer's currently active scope;
    /// any [`record_checked_query`](Self::record_checked_query) call
    /// before the scope finishes is forwarded to it.
    pub fn start_scope(&self, scenario_id: &'static str, fixture_items: usize) -> QueryCountScope {
        let accumulator = Arc::new(Mutex::new(ScopeAccumulator::default()));
        if let Ok(mut state) = self.inner.lock() {
            state.last_scenario = Some(scenario_id);
            state.active_scope = Some(Arc::downgrade(&accumulator));
        }
        QueryCountScope {
            observer: self.clone(),
            scenario_id,
            fixture_items,
            accumulator,
            finished: false,
        }
    }

    /// Record one query identifier against the observer's currently
    /// active scope. When no scope is open, the call is dropped on
    /// the floor — the helper exists so the public boundary matches
    /// the test contract.
    pub fn record_checked_query(&self, query_id: &'static str) {
        let active = {
            let Ok(state) = self.inner.lock() else {
                return;
            };
            state.active_scope.as_ref().and_then(|weak| weak.upgrade())
        };
        if let Some(active) = active
            && let Ok(mut acc) = active.lock()
        {
            acc.query_ids.push(query_id);
        }
    }

    /// Returns the last scenario recorded by any scope. Helpful for
    /// assertions in tests.
    pub fn last_scenario(&self) -> Option<&'static str> {
        self.inner.lock().ok().and_then(|state| state.last_scenario)
    }

    /// Clear the active-scope slot when the pointer matches the
    /// caller. Used by [`QueryCountScope::finish`] and
    /// [`QueryCountScope::drop`].
    fn clear_active_scope_if_matches(&self, accumulator: &Arc<Mutex<ScopeAccumulator>>) {
        if let Ok(mut state) = self.inner.lock() {
            let same = state
                .active_scope
                .as_ref()
                .and_then(|weak| weak.upgrade())
                .map(|active| Arc::ptr_eq(&active, accumulator))
                .unwrap_or(false);
            if same {
                state.active_scope = None;
            }
        }
    }
}

/// Verdict produced by [`evaluate_query_count`].
#[derive(Debug, Clone)]
pub struct QueryCountVerdict {
    failures: Vec<QueryCountFailureKind>,
    query_growth: i64,
}

impl QueryCountVerdict {
    /// Returns true when the samples pass the contract.
    pub fn is_accepted(&self) -> bool {
        self.failures.is_empty()
    }

    /// Returns true when `kind` is in the failure list.
    pub fn has_failure(&self, kind: QueryCountFailureKind) -> bool {
        self.failures.iter().any(|f| failure_matches(f, &kind))
    }

    /// Returns the failure list.
    pub fn failures(&self) -> &[QueryCountFailureKind] {
        &self.failures
    }

    /// Returns the difference between the large and small samples
    /// (positive when more queries were used on the large fixture).
    pub fn query_growth(&self) -> i64 {
        self.query_growth
    }
}

fn failure_matches(found: &QueryCountFailureKind, needle: &QueryCountFailureKind) -> bool {
    match (found, needle) {
        (
            QueryCountFailureKind::BudgetExceeded {
                maximum: a,
                actual: b,
            },
            QueryCountFailureKind::BudgetExceeded {
                maximum: c,
                actual: d,
            },
        ) => a == c && b == d,
        _ => found == needle,
    }
}

/// Compare two samples against `maximum_queries`. The contract
/// rejects N+1 growth, budget overruns, scenario mismatches, and
/// indeterminate fixtures.
pub fn evaluate_query_count(
    maximum_queries: usize,
    small: &QueryCountSample,
    large: &QueryCountSample,
) -> QueryCountVerdict {
    let mut failures = Vec::new();

    if small.scenario_id != large.scenario_id {
        failures.push(QueryCountFailureKind::ScenarioMismatch);
    }
    if small.total_queries == 0 || large.total_queries == 0 {
        failures.push(QueryCountFailureKind::Indeterminate);
    }
    if small.fixture_items == 0 || large.fixture_items == 0 {
        failures.push(QueryCountFailureKind::Indeterminate);
    }

    if large.total_queries > maximum_queries {
        failures.push(QueryCountFailureKind::BudgetExceeded {
            maximum: maximum_queries,
            actual: large.total_queries,
        });
    }

    let growth = large.total_queries as i64 - small.total_queries as i64;
    if large.fixture_items > small.fixture_items && growth > 0 {
        failures.push(QueryCountFailureKind::NPlusOne);
    }

    QueryCountVerdict {
        failures,
        query_growth: growth,
    }
}

/// One row in the production query-count catalog.
#[derive(Debug, Clone, Copy)]
pub struct QueryCountContract {
    /// Stable identifier.
    pub id: &'static str,
    /// Owner phase.
    pub owner_phase: &'static str,
    /// Activation / approval task.
    pub activation_task: &'static str,
    /// Maximum queries allowed for the scenario.
    pub maximum_queries: usize,
    /// Whether the contract is currently active.
    pub active: bool,
}

/// Static, exhaustive list of query-count contracts. The Foundation
/// contract is active; the US6/US10/US13 entries are inactive so
/// [`query_count_catalog_has_explicit_owner_phase_and_activation_metadata`]
/// can still detect the future owner without forcing the runtime to
/// honour them.
pub fn production_query_count_catalog() -> &'static [QueryCountContract] {
    CATALOG
}

const CATALOG: &[QueryCountContract] = &[
    QueryCountContract {
        id: "foundation.store.schema_version",
        owner_phase: "Foundation",
        activation_task: "T012",
        maximum_queries: 1,
        active: true,
    },
    QueryCountContract {
        id: "foundation.store.agents_count",
        owner_phase: "Foundation",
        activation_task: "T012",
        maximum_queries: 1,
        active: true,
    },
    QueryCountContract {
        id: "foundation.store.capabilities_list",
        owner_phase: "Foundation",
        activation_task: "T012",
        maximum_queries: 1,
        active: true,
    },
    QueryCountContract {
        id: "foundation.store.tags_list",
        owner_phase: "Foundation",
        activation_task: "T012",
        maximum_queries: 1,
        active: true,
    },
    QueryCountContract {
        id: "category.tree",
        owner_phase: "US6",
        activation_task: "T060",
        maximum_queries: 1,
        active: false,
    },
    QueryCountContract {
        id: "workflow.graph",
        owner_phase: "US10",
        activation_task: "T090",
        maximum_queries: 3,
        active: false,
    },
    QueryCountContract {
        id: "agent.resource_snapshot",
        owner_phase: "US13",
        activation_task: "T115",
        maximum_queries: 4,
        active: true,
    },
    QueryCountContract {
        id: "conversation.session_bundle",
        owner_phase: "US13",
        activation_task: "T117",
        maximum_queries: 2,
        active: true,
    },
];
