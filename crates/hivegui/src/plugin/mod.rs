//! US8 Plugin store (T077 boundary).
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md`
//! §T077-T081. The minimum subset required by T073 Red tests is
//! provided here so the public boundary exists; the full
//! no-replace, `row_revision` CAS, lease lifecycle and GC flows
//! land in the T077 implementation pass.

#![warn(missing_docs)]

pub mod plugin_store;
