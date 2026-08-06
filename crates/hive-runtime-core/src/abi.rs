//! Versioned Plugin ABI and `host_call` wire contracts.
//!
//! This module is the product-neutral home for the [`HIVE_EXTISM_ABI_V1`]
//! manifest identifier, the [`HostCallRequest`]/[`HostCallReply`] envelopes
//! exchanged with Plugins, the [`StableErrorKind`] category list, and the
//! validation hooks shared by HiveWeb and HiveGUI. The full Plugin manifest
//! surface (capabilities, exports, future-metadata round-trip) is implemented
//! alongside T018 and exercised by `tests/abi_contract.rs`.
//!
//! The wire envelope shapes are an explicit contract:
//!
//! - [`HostCallRequest`] is decoded from `{"capability": "...", "args": ...}`.
//!   The capability is stored as a string; the `args` JSON value is preserved
//!   verbatim as bytes (validated as JSON) so Plugins receive a canonical
//!   payload regardless of the host's serialisation quirks.
//! - [`HostCallReply`] serialises to one of two stable shapes:
//!   `{"ok":true,"data":<raw-json-bytes>}` on success, or
//!   `{"ok":false,"code":<numeric>,"message":"<sanitised>"}` on failure.
//!   Failure replies never carry a `data` field.
//! - [`StableErrorKind`] is the public enumeration of error categories. The
//!   `host_call_code()` mapping is append-only; new categories without a
//!   code (e.g. [`StableErrorKind::FunctionNotExecutable`]) are caught by
//!   the host before `host_call` is dispatched.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use serde::{Deserialize, Serialize};
use std::fmt;

/// Canonical ABI identifier for the v1 Plugin contract.
///
/// Plugins MUST emit this string in the `abi_version` field of their manifest.
/// The value is also the public contract used by both HiveWeb and HiveGUI when
/// comparing compatibility in [`crate::plugin::PluginManifestV1::parse_and_validate`].
pub const HIVE_EXTISM_ABI_V1: &str = "hive-extism/v1";

/// Stable error categories used across the runtime.
///
/// These variants form the public enumeration that products translate into
/// either a UI message or an HTTP error envelope. The list is append-only;
/// new categories must be added at the end and documented in the
/// `specs/011-hivegui-standalone-mode/contracts/plugin-abi.md` contract.
///
/// Each variant has a stable `host_call_code()` that is used in the wire
/// envelope. The single exception is [`StableErrorKind::FunctionNotExecutable`],
/// which is a runtime-only placeholder rejection: a host that sees a call
/// for a recognised-but-not-executable function MUST short-circuit before
/// the `host_call` envelope is dispatched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StableErrorKind {
    /// The Plugin's arguments failed validation.
    InvalidArgs,
    /// The Plugin does not hold the capability required to fulfil the call.
    CapabilityDenied,
    /// The Plugin requested a capability the host does not know.
    CapabilityUnknown,
    /// The Plugin timed out while waiting for a capability response.
    CapabilityTimeout,
    /// Cancellation was observed before the call completed.
    Cancelled,
    /// Internal invariant violation; never recoverable by the caller.
    Internal,
    /// The Plugin exceeded its execution timeout.
    PluginTimeout,
    /// The Plugin exceeded its memory budget.
    PluginMemoryLimit,
    /// The Plugin produced output that exceeded the configured limit.
    PluginOutputLimit,
    /// The Plugin attempted a WASI call that is denied by the sandbox.
    WasiDenied,
    /// Placeholder rejection: the function is recognised but is not currently
    /// executable (for example, a stub during development). This category
    /// is a stable runtime error and MUST be raised before `host_call` is
    /// dispatched; it has no host call code.
    FunctionNotExecutable,
}

impl StableErrorKind {
    /// Stable byte-level identifier used in serialised envelopes.
    ///
    /// Inputs: none (uses `self`).
    /// Outputs: the canonical snake-case name of the variant.
    /// Error modes: this method is total; every variant returns a string.
    pub fn as_str(self) -> &'static str {
        match self {
            StableErrorKind::InvalidArgs => "invalid_args",
            StableErrorKind::CapabilityDenied => "capability_denied",
            StableErrorKind::CapabilityUnknown => "capability_unknown",
            StableErrorKind::CapabilityTimeout => "capability_timeout",
            StableErrorKind::Cancelled => "cancelled",
            StableErrorKind::Internal => "internal",
            StableErrorKind::PluginTimeout => "plugin_timeout",
            StableErrorKind::PluginMemoryLimit => "plugin_memory_limit",
            StableErrorKind::PluginOutputLimit => "plugin_output_limit",
            StableErrorKind::WasiDenied => "wasi_denied",
            StableErrorKind::FunctionNotExecutable => "function_not_executable",
        }
    }

    /// Numeric code used in the `host_call` failure wire envelope, if any.
    ///
    /// Inputs: none (uses `self`).
    /// Outputs: the stable numeric code, or `None` for categories that
    /// never reach the wire (placeholder rejection).
    /// Error modes: this method is total; every variant returns a value.
    pub fn host_call_code(self) -> Option<u32> {
        match self {
            StableErrorKind::InvalidArgs => Some(4001),
            StableErrorKind::CapabilityDenied => Some(4030),
            StableErrorKind::CapabilityUnknown => Some(4045),
            StableErrorKind::CapabilityTimeout => Some(4081),
            StableErrorKind::Cancelled => Some(4990),
            StableErrorKind::Internal => Some(5000),
            StableErrorKind::PluginTimeout => Some(5004),
            StableErrorKind::PluginMemoryLimit => Some(5011),
            StableErrorKind::PluginOutputLimit => Some(5012),
            StableErrorKind::WasiDenied => Some(5013),
            StableErrorKind::FunctionNotExecutable => None,
        }
    }
}

impl fmt::Display for StableErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Request envelope for a `host_call` invocation.
///
/// The wire format is `{"capability": "<id>", "args": <json>}`; the
/// capability is a string identifier and `args` is an arbitrary JSON
/// value (object, array, string, number, boolean, or null). The decoded
/// request stores the capability as a string and preserves the original
/// JSON serialisation of `args` so the Plugin receives byte-identical
/// input regardless of how the host arranged intermediate values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCallRequest {
    capability: String,
    args_json: Vec<u8>,
}

/// Failure mode for [`HostCallRequest::decode`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostCallRequestError {
    /// The envelope is not valid JSON.
    #[error("host_call request is not valid JSON: {0}")]
    InvalidJson(String),
    /// The envelope is JSON but is not an object.
    #[error("host_call request root must be a JSON object")]
    NotAnObject,
    /// The `capability` field is missing or not a string.
    #[error("host_call request must include a string `capability` field")]
    MissingCapability,
    /// The `args` field is present but is not serialisable JSON.
    #[error("host_call request `args` field is not serialisable JSON: {0}")]
    InvalidArgs(String),
}

impl HostCallRequest {
    /// Decode a request from the documented wire envelope.
    ///
    /// Inputs:
    /// - `bytes`: the raw envelope body sent by the Plugin.
    ///
    /// Outputs:
    /// - A decoded [`HostCallRequest`] on success.
    ///
    /// Error modes:
    /// - [`HostCallRequestError::InvalidJson`] if `bytes` is not valid JSON.
    /// - [`HostCallRequestError::NotAnObject`] if the JSON root is not an
    ///   object.
    /// - [`HostCallRequestError::MissingCapability`] if `capability` is
    ///   missing or not a string.
    /// - [`HostCallRequestError::InvalidArgs`] if `args` is present but
    ///   cannot be re-serialised to canonical JSON.
    pub fn decode(bytes: &[u8]) -> Result<Self, HostCallRequestError> {
        let value: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|err| HostCallRequestError::InvalidJson(err.to_string()))?;
        let map = value
            .as_object()
            .ok_or(HostCallRequestError::NotAnObject)?;
        let capability = map
            .get("capability")
            .and_then(serde_json::Value::as_str)
            .ok_or(HostCallRequestError::MissingCapability)?
            .to_owned();
        let args_value = map.get("args").cloned().unwrap_or(serde_json::Value::Null);
        let args_json = serde_json::to_vec(&args_value)
            .map_err(|err| HostCallRequestError::InvalidArgs(err.to_string()))?;
        Ok(Self {
            capability,
            args_json,
        })
    }

    /// The capability the Plugin is requesting.
    ///
    /// Inputs: none.
    /// Outputs: the canonical capability identifier from the envelope.
    /// Error modes: this method is total.
    pub fn capability(&self) -> &str {
        &self.capability
    }

    /// The `args` payload as canonical JSON bytes.
    ///
    /// Inputs: none.
    /// Outputs: a byte slice containing the original JSON value of `args`
    /// re-serialised in canonical form.
    /// Error modes: this method is total.
    pub fn args_json(&self) -> &[u8] {
        &self.args_json
    }
}

/// Reply envelope for a `host_call` invocation.
///
/// Construct via [`HostCallReply::success_json`] or [`HostCallReply::failure`];
/// the reply is serialised with [`HostCallReply::encode_json`] into the
/// stable wire shape documented in the module-level docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCallReply {
    body: HostCallReplyBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HostCallReplyBody {
    /// A successful reply. `data` is the raw JSON bytes of the success
    /// payload; they are inlined verbatim into the wire envelope so the
    /// Plugin receives the exact bytes it produced.
    Success { data: Vec<u8> },
    /// A failure reply. `code` is the numeric code from
    /// [`StableErrorKind::host_call_code`] and `message` is the
    /// human-readable, sanitised message.
    Failure { code: u32, message: String },
}

/// Failure mode for [`HostCallReply::success_json`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostCallReplyError {
    /// The success payload is not valid JSON.
    #[error("host_call reply data is not valid JSON: {0}")]
    InvalidJson(String),
    /// The error kind does not have a host_call code (placeholder
    /// rejections must be raised before envelope construction).
    #[error("error kind {kind} has no host_call_code; use it before constructing a failure reply")]
    MissingHostCallCode { kind: &'static str },
}

impl HostCallReply {
    /// Build a success reply whose `data` is the supplied JSON payload.
    ///
    /// Inputs:
    /// - `bytes`: the raw JSON bytes of the success payload.
    ///
    /// Outputs:
    /// - A [`HostCallReply`] ready to be serialised with
    ///   [`HostCallReply::encode_json`].
    ///
    /// Error modes:
    /// - [`HostCallReplyError::InvalidJson`] if `bytes` is not valid JSON.
    pub fn success_json(bytes: &[u8]) -> Result<Self, HostCallReplyError> {
        let _: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|err| HostCallReplyError::InvalidJson(err.to_string()))?;
        Ok(Self {
            body: HostCallReplyBody::Success {
                data: bytes.to_vec(),
            },
        })
    }

    /// Build a failure reply carrying the supplied error kind and message.
    ///
    /// Inputs:
    /// - `kind`: the [`StableErrorKind`] to surface. The variant MUST have a
    ///   [`StableErrorKind::host_call_code`]; placeholder rejections
    ///   ([`StableErrorKind::FunctionNotExecutable`]) are rejected at this
    ///   point because they belong to the host's pre-dispatch layer. If
    ///   `kind` has no host call code the constructor panics — the
    ///   expectation is enforced by [`StableErrorKind::host_call_code`]
    ///   returning `None` only for runtime-only categories.
    /// - `message`: the sanitised, human-readable error message.
    ///
    /// Outputs:
    /// - A [`HostCallReply`] ready to be serialised with
    ///   [`HostCallReply::encode_json`].
    ///
    /// Error modes: this constructor panics if `kind` has no
    /// `host_call_code`. Callers MUST check [`StableErrorKind::host_call_code`]
    /// before constructing the reply for a placeholder rejection.
    pub fn failure(kind: StableErrorKind, message: impl Into<String>) -> Self {
        let code = kind
            .host_call_code()
            .unwrap_or_else(|| panic!("StableErrorKind::{kind} has no host_call_code"));
        Self {
            body: HostCallReplyBody::Failure {
                code,
                message: message.into(),
            },
        }
    }

    /// Serialise the reply into the documented wire envelope.
    ///
    /// Inputs: none (uses `self`).
    /// Outputs: the byte representation of the wire envelope.
    /// Error modes: serialisation of the failure envelope uses
    /// `serde_json` and panics if the message is not UTF-8; this is
    /// unreachable for well-formed inputs.
    pub fn encode_json(&self) -> Vec<u8> {
        match &self.body {
            HostCallReplyBody::Success { data } => {
                // Inline the raw JSON payload verbatim so the Plugin sees
                // exactly what it produced.
                let mut out = Vec::with_capacity(data.len() + 16);
                out.extend_from_slice(b"{\"ok\":true,\"data\":");
                out.extend_from_slice(data);
                out.push(b'}');
                out
            }
            HostCallReplyBody::Failure { code, message } => {
                // Hand-roll the envelope so the field order is exactly
                // `ok, code, message` (serde_json's `Value::Object` map
                // is alphabetically sorted by default and would
                // produce a different order).
                let code_str = code.to_string();
                let message_json = serde_json::Value::String(message.clone());
                let message_str = serde_json::to_string(&message_json)
                    .expect("serialising a String is total");
                format!(
                    r#"{{"ok":false,"code":{code_str},"message":{message_str}}}"#
                )
                .into_bytes()
            }
        }
    }

    /// Returns `true` when this reply represents a successful dispatch.
    ///
    /// Inputs: none.
    /// Outputs: `true` if [`HostCallReply::success_json`] produced this
    /// value, `false` otherwise.
    /// Error modes: this method is total.
    pub fn is_success(&self) -> bool {
        matches!(self.body, HostCallReplyBody::Success { .. })
    }
}
