//! Versioned Plugin ABI and `host_call` wire contracts.
//!
//! This module is the product-neutral home for the `hive-extism/v1` manifest,
//! request and response envelopes, stable error categories, and compatibility
//! validation shared by HiveWeb and HiveGUI. Validation must be deterministic
//! and local; it must not discover capabilities through a product service.
