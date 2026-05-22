//! Network security utilities — SSRF protection and internal URL detection
//! (Rust port of `nanobot.security.network`).

pub mod network;

pub use network::{
    clear_ssrf_whitelist, configure_ssrf_whitelist, contains_internal_url,
    validate_resolved_url, validate_url_target,
};
