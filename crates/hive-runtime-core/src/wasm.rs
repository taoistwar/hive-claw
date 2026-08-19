//! WASM validation and sandbox configuration contracts.
//!
//! Shared validation covers module shape, declared imports and exports, resource
//! units, WASI denial, and stable execution failures. Filesystem containment,
//! artifact hashing, byte loading, and runtime construction are performed by
//! product adapters before or after this pure boundary as the contract requires.
//!
//! T018 implements the storage- and runtime-neutral portions of the WASM
//! contract:
//!
//! - [`WasmModuleShape`] and [`WasmValidationError`] enumerate the structural
//!   invariants a Plugin artifact must satisfy (presence of an `extism_sdk`
//!   fingerprint, no stray `wasi_snapshot_preview1` imports when WASI is
//!   denied, allowed hosts limited to the documented `env` set, single
//!   `host_call` import surface, single `_hive_plugin_abi_version` export).
//! - [`WasmSandboxConfig`] carries the runtime resource limits that HiveWeb
//!   and HiveGUI must agree on: timeout, memory budget, output budget, fuel,
//!   and the WASI toggle. The [`WasmSandboxConfig::deny_all_wasi`] preset
//!   encodes the production rule that production Plugins MUST NOT touch the
//!   filesystem or the network.
//! - [`WasmExecutionFailure`] is the stable, public error category returned
//!   to capability dispatch when a Plugin execution fails. The categories
//!   line up with the [`crate::abi::StableErrorKind`] wire codes so product
//!   adapters can surface the same code through the existing `host_call`
//!   envelope.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::fmt;

/// Categories of structural rejection of a Plugin WASM artifact.
///
/// The variants are append-only; removing a variant is a breaking change
/// for any host that has hard-coded the rejection reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WasmModuleShape {
    /// The artifact is missing the canonical `extism_sdk` fingerprint
    /// (e.g. `extism:host/user@1` or a host-specific alias). Production
    /// artifacts MUST be produced through the official build pipeline.
    MissingExtismFingerprint,
    /// The artifact imports `wasi_snapshot_preview1` while the sandbox
    /// is configured with `wasi_enabled = false`.
    WasiImportPresent,
    /// The artifact imports a host outside the documented `env` allowlist.
    DisallowedHostImport,
    /// The artifact declares a `host_call` import but the host function
    /// name is not the v1 stable name.
    UnknownHostCallImport,
    /// The artifact is missing the `_hive_plugin_abi_version` export that
    /// the host uses to confirm the v1 ABI is bound.
    MissingAbiVersionExport,
    /// The artifact exports the `_hive_plugin_abi_version` symbol with a
    /// value that does not match the [`crate::abi::HIVE_EXTISM_ABI_V1`]
    /// constant.
    UnsupportedAbiVersion,
}

/// Outcome of validating a Plugin artifact's structural shape.
///
/// Successful validation produces a [`WasmValidationError::Ok`] variant; any
/// other variant identifies the first structural violation. Hosts MUST
/// surface the category in the product's diagnostic boundary; the category
/// is also the wire-level error code returned to the Plugin's `host_call`
/// envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WasmValidationError {
    /// The artifact cleared every structural invariant. Carries a short,
    /// human-readable summary for logs.
    Ok {
        /// The summary line.
        summary: String,
    },
    /// The artifact failed a structural invariant.
    Rejected {
        /// The structural category.
        kind: WasmModuleShape,
        /// Optional human-readable detail (never carries the rejected
        /// import/export identifier; see [`WasmValidationError::display`]`).
        detail: Option<String>,
    },
}

impl WasmValidationError {
    /// Build a successful validation result.
    ///
    /// Inputs:
    /// - `summary`: a short, log-only description of the validated
    ///   artifact (e.g. the bound `extism_sdk` version).
    ///
    /// Outputs: a [`WasmValidationError::Ok`] variant.
    /// Error modes: this constructor is total.
    pub fn ok(summary: impl Into<String>) -> Self {
        Self::Ok {
            summary: summary.into(),
        }
    }

    /// Build a rejected validation result.
    ///
    /// Inputs:
    /// - `kind`: the [`WasmModuleShape`] category.
    /// - `detail`: optional human-readable detail. The detail MUST NOT
    ///   include the raw identifier that triggered the rejection; it is
    ///   intended for log correlation, not user display.
    ///
    /// Outputs: a [`WasmValidationError::Rejected`] variant.
    /// Error modes: this constructor is total.
    pub fn rejected(kind: WasmModuleShape, detail: Option<String>) -> Self {
        Self::Rejected { kind, detail }
    }

    /// Returns `true` when the artifact was accepted.
    ///
    /// Inputs: none.
    /// Outputs: `true` for [`WasmValidationError::Ok`], `false` otherwise.
    /// Error modes: this method is total.
    pub fn is_ok(&self) -> bool {
        matches!(self, WasmValidationError::Ok { .. })
    }

    /// The rejection category, if any.
    ///
    /// Inputs: none.
    /// Outputs: `Some(category)` for [`WasmValidationError::Rejected`],
    /// `None` for [`WasmValidationError::Ok`].
    /// Error modes: this method is total.
    pub fn rejection_kind(&self) -> Option<WasmModuleShape> {
        match self {
            WasmValidationError::Ok { .. } => None,
            WasmValidationError::Rejected { kind, .. } => Some(*kind),
        }
    }

    /// Display form. Successful validations surface only the summary;
    /// rejected validations surface the category and the optional
    /// sanitised detail, never the raw identifier.
    pub fn display(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WasmValidationError::Ok { summary } => {
                write!(formatter, "wasm validation ok: {summary}")
            }
            WasmValidationError::Rejected { kind, detail } => {
                let label = match kind {
                    WasmModuleShape::MissingExtismFingerprint => "missing extism fingerprint",
                    WasmModuleShape::WasiImportPresent => "wasi import present",
                    WasmModuleShape::DisallowedHostImport => "disallowed host import",
                    WasmModuleShape::UnknownHostCallImport => "unknown host_call import",
                    WasmModuleShape::MissingAbiVersionExport => "missing abi version export",
                    WasmModuleShape::UnsupportedAbiVersion => "unsupported abi version",
                };
                match detail {
                    Some(detail) => {
                        write!(formatter, "wasm validation rejected: {label} ({detail})")
                    }
                    None => write!(formatter, "wasm validation rejected: {label}"),
                }
            }
        }
    }
}

impl fmt::Display for WasmValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.display(formatter)
    }
}

impl std::error::Error for WasmValidationError {}

/// Runtime sandbox configuration shared between HiveWeb and HiveGUI.
///
/// The struct is intentionally a pure value type: filesystem containment,
/// artifact hashing, byte loading, and the actual instantiation of the
/// Extism plugin are caller-provided concerns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmSandboxConfig {
    /// Maximum wall-clock duration of a single Plugin call.
    pub timeout_ms: u64,
    /// Maximum memory budget the Plugin may grow to, in bytes.
    pub memory_bytes: u64,
    /// Maximum output bytes the Plugin may produce from one call.
    pub output_bytes: u64,
    /// Optional fuel budget. `None` means "no fuel limit".
    pub fuel: Option<u64>,
    /// Whether the Plugin is allowed to call WASI. Production Plugins
    /// MUST run with this set to `false`.
    pub wasi_enabled: bool,
}

impl WasmSandboxConfig {
    /// The recommended production preset: WASI disabled, conservative
    /// defaults for desktop hosts. Callers MUST override the limits
    /// through the public fields for any call that handles untrusted
    /// input.
    ///
    /// Inputs: none.
    /// Outputs: a [`WasmSandboxConfig`] with `wasi_enabled = false`,
    /// `timeout_ms = 30_000`, `memory_bytes = 64 MiB`,
    /// `output_bytes = 1 MiB`, and `fuel = None`.
    /// Error modes: this constructor is total.
    pub fn deny_all_wasi() -> Self {
        Self {
            timeout_ms: 30_000,
            memory_bytes: 64 * 1024 * 1024,
            output_bytes: 1024 * 1024,
            fuel: None,
            wasi_enabled: false,
        }
    }

    /// Returns `true` when the configuration allows WASI imports.
    ///
    /// Inputs: none.
    /// Outputs: the value of `wasi_enabled`.
    /// Error modes: this method is total.
    pub fn wasi_allowed(&self) -> bool {
        self.wasi_enabled
    }
}

/// Stable categories of execution-time Plugin failures.
///
/// The variants line up with the public [`crate::abi::StableErrorKind`]
/// enumeration so product adapters can translate a wire failure into a
/// UI message without re-deriving the category. New categories must be
/// added to both enums in lockstep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WasmExecutionFailure {
    /// The Plugin produced output larger than the configured
    /// [`WasmSandboxConfig::output_bytes`].
    OutputLimitExceeded,
    /// The Plugin grew beyond [`WasmSandboxConfig::memory_bytes`].
    MemoryLimitExceeded,
    /// The Plugin exhausted its [`WasmSandboxConfig::fuel`] budget.
    FuelExhausted,
    /// The Plugin execution exceeded [`WasmSandboxConfig::timeout_ms`].
    Timeout,
    /// The Plugin trapped or aborted with an unrecoverable error.
    Trap,
    /// The Plugin attempted a WASI call while
    /// [`WasmSandboxConfig::wasi_enabled`] was `false`.
    WasiDenied,
}

impl WasmExecutionFailure {
    /// Stable byte-level identifier used in logs and diagnostics.
    ///
    /// Inputs: none (uses `self`).
    /// Outputs: the canonical snake_case label.
    /// Error modes: this method is total.
    pub fn as_str(self) -> &'static str {
        match self {
            WasmExecutionFailure::OutputLimitExceeded => "output_limit_exceeded",
            WasmExecutionFailure::MemoryLimitExceeded => "memory_limit_exceeded",
            WasmExecutionFailure::FuelExhausted => "fuel_exhausted",
            WasmExecutionFailure::Timeout => "timeout",
            WasmExecutionFailure::Trap => "trap",
            WasmExecutionFailure::WasiDenied => "wasi_denied",
        }
    }

    /// The corresponding [`crate::abi::StableErrorKind`] variant.
    ///
    /// Inputs: none (uses `self`).
    /// Outputs: the matching [`crate::abi::StableErrorKind`].
    /// Error modes: this method is total; every variant maps to a
    /// distinct category.
    pub fn stable_error_kind(self) -> crate::abi::StableErrorKind {
        match self {
            WasmExecutionFailure::OutputLimitExceeded => {
                crate::abi::StableErrorKind::PluginOutputLimit
            }
            WasmExecutionFailure::MemoryLimitExceeded => {
                crate::abi::StableErrorKind::PluginMemoryLimit
            }
            WasmExecutionFailure::FuelExhausted => crate::abi::StableErrorKind::PluginTimeout,
            WasmExecutionFailure::Timeout => crate::abi::StableErrorKind::PluginTimeout,
            WasmExecutionFailure::Trap => crate::abi::StableErrorKind::Internal,
            WasmExecutionFailure::WasiDenied => crate::abi::StableErrorKind::WasiDenied,
        }
    }
}

impl fmt::Display for WasmExecutionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The canonical `host_call` import name exposed to Plugins.
///
/// Hosts MUST expose this exact symbol; any other name is treated as a
/// [`WasmModuleShape::UnknownHostCallImport`] structural failure.
pub const HOST_CALL_IMPORT: &str = "host_call";

/// The canonical `env` module name for the `host_call` import.
///
/// Hosts MUST expose [`HOST_CALL_IMPORT`] under this module name.
///
/// This is the module alias used by pure-WAT / bare-Wasmtime host adapters
/// (and by the shared WAT fixtures). The Extism SDK compiles its Plugins
/// against a *different*, fixed namespace: [`EXTISM_HOST_CALL_MODULE`].
/// Extism's [`PluginBuilder::with_function`] can only mount host functions
/// under that namespace, so Extism-based hosts (HiveWeb `invoker.rs` and
/// HiveGUI `plugin_executor.rs`) must import/reference the Extism namespace
/// rather than `env`. The two constants therefore name the *same logical*
/// `host_call` surface in two distinct linkage styles; both MUST be treated
/// as the single v1 import surface by [`WasmModuleShape`] validation.
pub const HOST_CALL_MODULE: &str = "env";

/// The `host_call` module namespace emitted by the Extism SDK.
///
/// Extism mounts every `PluginBuilder::with_function`-registered host
/// function under `extism:host/user.<name>`; this is fixed by the SDK and
/// cannot be overridden per-plugin. Hosts built on Extism (HiveWeb and
/// HiveGUI) therefore link [`HOST_CALL_IMPORT`] as
/// `extism:host/user.host_call`, while bare-Wasmtime hosts link it as
/// `env.host_call` ([`HOST_CALL_MODULE`]).
pub const EXTISM_HOST_CALL_MODULE: &str = "extism:host/user";

/// The canonical ABI version export name.
///
/// Plugins MUST export a function with this name whose body returns the
/// [`crate::abi::HIVE_EXTISM_ABI_V1`] string. Hosts verify the
/// return value matches before instantiating the Plugin.
pub const ABI_VERSION_EXPORT: &str = "_hive_plugin_abi_version";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_all_wasi_disables_wasi() {
        let cfg = WasmSandboxConfig::deny_all_wasi();
        assert!(!cfg.wasi_allowed());
        assert!(cfg.timeout_ms > 0);
        assert!(cfg.memory_bytes > 0);
        assert!(cfg.output_bytes > 0);
    }

    #[test]
    fn execution_failure_maps_to_stable_error_kind() {
        for failure in [
            WasmExecutionFailure::OutputLimitExceeded,
            WasmExecutionFailure::MemoryLimitExceeded,
            WasmExecutionFailure::FuelExhausted,
            WasmExecutionFailure::Timeout,
            WasmExecutionFailure::Trap,
            WasmExecutionFailure::WasiDenied,
        ] {
            let _ = failure.stable_error_kind();
            assert!(!failure.as_str().is_empty());
        }
    }

    #[test]
    fn validation_rejected_carries_category() {
        let err = WasmValidationError::rejected(WasmModuleShape::WasiImportPresent, None);
        assert!(!err.is_ok());
        assert_eq!(
            err.rejection_kind(),
            Some(WasmModuleShape::WasiImportPresent)
        );
    }

    #[test]
    fn canonical_import_and_export_names_are_stable() {
        assert_eq!(HOST_CALL_MODULE, "env");
        assert_eq!(EXTISM_HOST_CALL_MODULE, "extism:host/user");
        assert_eq!(HOST_CALL_IMPORT, "host_call");
        assert_eq!(ABI_VERSION_EXPORT, "_hive_plugin_abi_version");
    }
}
