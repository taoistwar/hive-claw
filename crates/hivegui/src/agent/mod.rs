//! Local agent runtime (T115-T136 boundary).
//!
//! This module exposes the [`session`] sub-module that hosts the
//! Red public boundaries of the local agent runtime, the
//! [`local_agent`] runtime that T116/T125 drive, and the
//! [`agent_content`] content helpers used to build per-turn
//! immutable snapshots.

#![warn(missing_docs)]

pub mod agent_content;
pub mod local_agent;
pub mod session;
