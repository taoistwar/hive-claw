//! Product-neutral execution context and cancellation contracts.
//!
//! Shared execution state propagates an execution identifier, immutable
//! permission snapshot, event sink, segmented timings, and derived cancellation
//! signals through Agent, LLM, Tool, Workflow, Plugin, and Capability calls.
//! Persistence, task spawning, and UI delivery remain caller-provided concerns.
