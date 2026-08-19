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

/// The `wasi_snapshot_preview1` module name.
///
/// Any import of this module is a [`WasmModuleShape::WasiImportPresent`]
/// violation while [`WasmSandboxConfig::wasi_enabled`] is `false`.
pub const WASI_MODULE_NAME: &str = "wasi_snapshot_preview1";

/// Validate a Plugin artifact's structural shape against the shared
/// [`WasmModuleShape`] invariants (§4 隔离, §7 兼容性).
///
/// This is the single storage-neutral source of truth for the import /
/// export invariants that HiveWeb and HiveGUI must both enforce before any
/// Plugin byte reaches a runtime. It parses the import and export sections
/// directly and fails fast on the first violation.
///
/// Inputs:
/// - `bytes`: the raw WASM binary. The caller MUST have already performed
///   the `\0asm` magic/version header check; a truncated or overflowing
///   section fails closed as [`WasmModuleShape::DisallowedHostImport`]
///   with a `"malformed …"` detail (the runtime still rejects such bytes
///   at instantiation).
/// - `wasi_enabled`: whether the sandbox permits WASI imports. Production
///   hosts pass `false` ([`WasmSandboxConfig::deny_all_wasi`]).
///
/// Outputs: [`WasmValidationError::Ok`] with a short summary, or
/// [`WasmValidationError::Rejected`] carrying the first
/// [`WasmModuleShape`] category.
/// Error modes: total. Malformed sections fail closed.
///
/// Static validation covers five of the six [`WasmModuleShape`]
/// categories. [`WasmModuleShape::UnsupportedAbiVersion`] is *not*
/// detected here: confirming that the [`ABI_VERSION_EXPORT`] function
/// actually returns [`crate::abi::HIVE_EXTISM_ABI_V1`] requires executing
/// the Plugin, which is the runtime adapter's responsibility after a
/// successful static check.
pub fn validate_wasm_shape(bytes: &[u8], wasi_enabled: bool) -> WasmValidationError {
    match parse_wasm_sections(bytes) {
        Err(detail) => {
            WasmValidationError::rejected(WasmModuleShape::DisallowedHostImport, Some(detail))
        }
        Ok(shape) => {
            // (a) WASI denial (§4 隔离).
            if !wasi_enabled && shape.has_wasi_import {
                return WasmValidationError::rejected(WasmModuleShape::WasiImportPresent, None);
            }
            // (b) Host outside the documented allowlist.
            if shape.disallowed_host {
                return WasmValidationError::rejected(WasmModuleShape::DisallowedHostImport, None);
            }
            // (c) A `host_call`-intent import with a non-stable name.
            if shape.unknown_host_call {
                return WasmValidationError::rejected(WasmModuleShape::UnknownHostCallImport, None);
            }
            // (d) No extism/`env` host surface at all (not built through the
            //     official pipeline).
            if !shape.has_host_surface {
                return WasmValidationError::rejected(
                    WasmModuleShape::MissingExtismFingerprint,
                    None,
                );
            }
            // (e) Missing the v1 ABI-version export.
            if !shape.has_abi_export {
                return WasmValidationError::rejected(
                    WasmModuleShape::MissingAbiVersionExport,
                    None,
                );
            }
            WasmValidationError::ok("validated v1 plugin shape")
        }
    }
}

/// Aggregated structural facts collected while scanning the import and
/// export sections. Purely an internal carrier for [`validate_wasm_shape`].
#[derive(Default)]
struct WasmShape {
    /// At least one `wasi_snapshot_preview1` import is present.
    has_wasi_import: bool,
    /// At least one import lives outside `extism:host/*` / `env` /
    /// `wasi_snapshot_preview1`.
    disallowed_host: bool,
    /// A function import under `extism:host/user` or `env` is named
    /// something other than [`HOST_CALL_IMPORT`].
    unknown_host_call: bool,
    /// At least one `extism:host/*` or `env` import exists (the extism_sdk
    /// or bare-host fingerprint).
    has_host_surface: bool,
    /// The [`ABI_VERSION_EXPORT`] function export is present.
    has_abi_export: bool,
}

/// Parse the WASM import and export sections into a [`WasmShape`].
///
/// Fail-closed: any truncation or LEB128 overflow is reported as an
/// `Err(String)` that [`validate_wasm_shape`] surfaces as a structural
/// rejection.
fn parse_wasm_sections(bytes: &[u8]) -> Result<WasmShape, String> {
    if bytes.len() < 8 {
        return Err("malformed wasm: shorter than 8-byte header".to_string());
    }

    let mut shape = WasmShape::default();
    let mut pos = 8usize;

    while pos < bytes.len() {
        let section_id = *bytes
            .get(pos)
            .ok_or("malformed wasm: truncated section id")?;
        pos += 1;
        let section_size = read_leb128_u32(bytes, &mut pos)
            .ok_or("malformed wasm: truncated section size")? as usize;
        let section_end = pos
            .checked_add(section_size)
            .ok_or("malformed wasm: section size overflow")?;
        if section_end > bytes.len() {
            return Err("malformed wasm: section exceeds input".to_string());
        }

        match section_id {
            2 => parse_import_section(bytes, pos, section_end, &mut shape)?,
            7 => parse_export_section(bytes, pos, section_end, &mut shape)?,
            _ => {}
        }

        pos = section_end;
    }

    Ok(shape)
}

/// Parse the import section (`id = 2`) into `shape`.
fn parse_import_section(
    bytes: &[u8],
    pos: usize,
    end: usize,
    shape: &mut WasmShape,
) -> Result<(), String> {
    let mut cur = pos;
    let num_imports =
        read_leb128_u32(bytes, &mut cur).ok_or("malformed wasm: truncated import count")?;

    for _ in 0..num_imports {
        let module = read_name(bytes, &mut cur).ok_or("malformed wasm: truncated import module")?;
        let name = read_name(bytes, &mut cur).ok_or("malformed wasm: truncated import name")?;
        let kind = *bytes
            .get(cur)
            .ok_or("malformed wasm: truncated import kind")?;
        cur += 1;
        skip_import_desc(bytes, &mut cur, kind)
            .ok_or("malformed wasm: truncated import descriptor")?;

        if module == WASI_MODULE_NAME {
            shape.has_wasi_import = true;
        }
        let is_host = module.starts_with("extism:host/") || module == HOST_CALL_MODULE;
        if is_host {
            shape.has_host_surface = true;
        }
        if module != WASI_MODULE_NAME && !is_host {
            shape.disallowed_host = true;
        }
        // Only the `extism:host/user` (Extism SDK) and `env` (bare host)
        // namespaces carry the user host function; `extism:host/env.*` is
        // Extism PDK infrastructure and is never a `host_call`.
        if kind == 0x00
            && (module == EXTISM_HOST_CALL_MODULE || module == HOST_CALL_MODULE)
            && name != HOST_CALL_IMPORT
        {
            shape.unknown_host_call = true;
        }

        if cur > end {
            return Err("malformed wasm: import section overruns its size".to_string());
        }
    }

    Ok(())
}

/// Parse the export section (`id = 7`) into `shape`.
fn parse_export_section(
    bytes: &[u8],
    pos: usize,
    end: usize,
    shape: &mut WasmShape,
) -> Result<(), String> {
    let mut cur = pos;
    let num_exports =
        read_leb128_u32(bytes, &mut cur).ok_or("malformed wasm: truncated export count")?;

    for _ in 0..num_exports {
        let name = read_name(bytes, &mut cur).ok_or("malformed wasm: truncated export name")?;
        let kind = *bytes
            .get(cur)
            .ok_or("malformed wasm: truncated export kind")?;
        cur += 1;
        read_leb128_u32(bytes, &mut cur).ok_or("malformed wasm: truncated export index")?;

        if name == ABI_VERSION_EXPORT && kind == 0x00 {
            shape.has_abi_export = true;
        }

        if cur > end {
            return Err("malformed wasm: export section overruns its size".to_string());
        }
    }

    Ok(())
}

/// Read an unsigned LEB128 `u32` from `bytes` at `*pos`, advancing `pos`.
///
/// Returns `None` on truncation or when the encoding overflows `u32`
/// (more than five significant bytes).
fn read_leb128_u32(bytes: &[u8], pos: &mut usize) -> Option<u32> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    loop {
        let byte = *bytes.get(*pos)?;
        *pos += 1;
        result |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return u32::try_from(result).ok();
        }
        shift += 7;
        if shift >= 35 {
            return None;
        }
    }
}

/// Read a length-prefixed UTF-8 name from `bytes` at `*pos`, advancing
/// `pos`. Returns `None` on truncation or non-UTF-8 content.
fn read_name<'a>(bytes: &'a [u8], pos: &mut usize) -> Option<&'a str> {
    let len = read_leb128_u32(bytes, pos)? as usize;
    let end = pos.checked_add(len)?;
    let slice = bytes.get(*pos..end)?;
    *pos = end;
    std::str::from_utf8(slice).ok()
}

/// Skip a WASM limits structure: flags byte + min, and max when present.
fn skip_limits(bytes: &[u8], pos: &mut usize) -> Option<()> {
    let flags = *bytes.get(*pos)?;
    *pos += 1;
    read_leb128_u32(bytes, pos)?;
    if flags & 0x01 != 0 {
        read_leb128_u32(bytes, pos)?;
    }
    Some(())
}

/// Skip the descriptor that follows an import's kind byte.
fn skip_import_desc(bytes: &[u8], pos: &mut usize, kind: u8) -> Option<()> {
    match kind {
        // function: type index.
        0x00 => {
            read_leb128_u32(bytes, pos)?;
            Some(())
        }
        // table: reftype (s33, read as unsigned) + limits.
        0x01 => {
            read_leb128_u32(bytes, pos)?;
            skip_limits(bytes, pos)
        }
        // memory: limits.
        0x02 => skip_limits(bytes, pos),
        // global: valtype (1 byte) + mutability (1 byte).
        0x03 => {
            let end = pos.checked_add(2)?;
            bytes.get(*pos..end)?;
            *pos = end;
            Some(())
        }
        // Unknown kind: cannot determine the descriptor size.
        _ => None,
    }
}

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
        assert_eq!(WASI_MODULE_NAME, "wasi_snapshot_preview1");
    }

    // -- minimal WASM section builders -----------------------------------

    fn leb128(mut value: u32) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let mut byte = (value & 0x7F) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if value == 0 {
                break;
            }
        }
        out
    }

    fn section(id: u8, content: &[u8]) -> Vec<u8> {
        let mut out = vec![id];
        out.extend(leb128(content.len() as u32));
        out.extend(content);
        out
    }

    /// Build the import section (`id = 2`) for a list of `(module, name)`
    /// function imports.
    fn import_section(imports: &[(&str, &str)]) -> Vec<u8> {
        let mut content = leb128(imports.len() as u32);
        for (module, name) in imports {
            content.push(module.len() as u8);
            content.extend_from_slice(module.as_bytes());
            content.push(name.len() as u8);
            content.extend_from_slice(name.as_bytes());
            content.push(0x00); // function kind
            content.push(0x00); // type index
        }
        section(2, &content)
    }

    /// Build the export section (`id = 7`) for a list of `(name, kind)`
    /// exports, all referencing index 0.
    fn export_section(exports: &[(&str, u8)]) -> Vec<u8> {
        let mut content = leb128(exports.len() as u32);
        for (name, kind) in exports {
            content.push(name.len() as u8);
            content.extend_from_slice(name.as_bytes());
            content.push(*kind);
            content.push(0x00); // index
        }
        section(7, &content)
    }

    fn module(imports: &[(&str, &str)], exports: &[(&str, u8)]) -> Vec<u8> {
        let mut wasm = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        if !imports.is_empty() {
            wasm.extend(import_section(imports));
        }
        if !exports.is_empty() {
            wasm.extend(export_section(exports));
        }
        wasm
    }

    const ABI_EXPORT: (&str, u8) = (ABI_VERSION_EXPORT, 0x00);

    #[test]
    fn valid_extism_plugin_passes() {
        let wasm = module(
            &[
                ("extism:host/user", "host_call"),
                ("extism:host/env", "input_length"),
            ],
            &[ABI_EXPORT],
        );
        assert!(validate_wasm_shape(&wasm, false).is_ok());
    }

    #[test]
    fn valid_bare_env_plugin_passes() {
        let wasm = module(&[("env", "host_call")], &[ABI_EXPORT]);
        assert!(validate_wasm_shape(&wasm, false).is_ok());
    }

    #[test]
    fn wasi_import_is_rejected_when_wasi_disabled() {
        let wasm = module(
            &[
                ("extism:host/user", "host_call"),
                ("wasi_snapshot_preview1", "random_get"),
            ],
            &[ABI_EXPORT],
        );
        assert_eq!(
            validate_wasm_shape(&wasm, false).rejection_kind(),
            Some(WasmModuleShape::WasiImportPresent)
        );
        // WASI enabled: the import is tolerated, so the plugin still passes.
        assert!(validate_wasm_shape(&wasm, true).is_ok());
    }

    #[test]
    fn disallowed_host_import_is_rejected() {
        let wasm = module(&[("extism", "time.now")], &[ABI_EXPORT]);
        assert_eq!(
            validate_wasm_shape(&wasm, false).rejection_kind(),
            Some(WasmModuleShape::DisallowedHostImport)
        );
    }

    #[test]
    fn unknown_user_host_function_is_rejected() {
        let wasm = module(&[("extism:host/user", "other_func")], &[ABI_EXPORT]);
        assert_eq!(
            validate_wasm_shape(&wasm, false).rejection_kind(),
            Some(WasmModuleShape::UnknownHostCallImport)
        );
    }

    #[test]
    fn missing_host_surface_is_rejected_as_no_fingerprint() {
        // No extism:host/* or env import at all.
        let wasm = module(&[], &[ABI_EXPORT]);
        assert_eq!(
            validate_wasm_shape(&wasm, false).rejection_kind(),
            Some(WasmModuleShape::MissingExtismFingerprint)
        );
    }

    #[test]
    fn missing_abi_export_is_rejected() {
        let wasm = module(&[("extism:host/user", "host_call")], &[]);
        assert_eq!(
            validate_wasm_shape(&wasm, false).rejection_kind(),
            Some(WasmModuleShape::MissingAbiVersionExport)
        );
    }

    #[test]
    fn truncated_section_fails_closed() {
        // A section whose declared size exceeds the input.
        let mut wasm = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        wasm.push(2); // import section id
        wasm.extend(leb128(100)); // claims 100 bytes, but none follow
        assert_eq!(
            validate_wasm_shape(&wasm, false).rejection_kind(),
            Some(WasmModuleShape::DisallowedHostImport)
        );
    }
}
