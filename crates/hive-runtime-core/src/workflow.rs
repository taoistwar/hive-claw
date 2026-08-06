//! Storage-independent Workflow graph contracts and pure scheduling algorithms.
//!
//! Workflow definitions use the stable node types `start_node`, `end_node`,
//! `function_node`, and `generate_answer_node`. Graph validation, topological
//! layering, fail-fast result aggregation, and cancellation checkpoints belong
//! here; loading records and executing product resources remain adapter duties.
