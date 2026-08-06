//! Storage- and transport-independent Agent runtime contracts.
//!
//! This crate is the shared protocol and pure-algorithm boundary used by both
//! HiveWeb and HiveGUI. It owns no database, filesystem, network client, UI, or
//! product-specific lifecycle. Callers provide those concerns through adapters.
//! In particular, sharing this crate never permits HiveGUI to request or fall
//! back to HiveWeb.
//!
//! The modules intentionally begin as documented public seams. Versioned ABI
//! validation, execution cancellation, capability dispatch, plugin isolation,
//! WASM validation, workflow scheduling, and persisted Tool DTOs are implemented
//! in their dedicated feature tasks without adding product storage or transport
//! dependencies here.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod abi;
pub mod capability;
pub mod execution;
pub mod persisted_tool;
pub mod plugin;
pub mod wasm;
pub mod workflow;
