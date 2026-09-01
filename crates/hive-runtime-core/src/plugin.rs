//! Product-neutral Plugin manifest and execution policy contracts.
//!
//! This module describes validated manifests, exports, resource limits,
//! capability requirements, and cache identity. It does not open artifact paths,
//! persist Plugin records, or construct a product runtime. Hosts must validate
//! artifacts and permissions before supplying controlled WASM bytes for use.
//!
//! The manifest contract is the v1 surface documented in
//! `specs/011-hivegui-standalone-mode/contracts/plugin-abi.md`. Validation
//! accumulates every independently detectable compatibility issue; the
//! resulting [`ManifestError`] exposes them as a slice of [`Issue`] values
//! so callers can decide which issues are fatal for their context.
//!
//! Re-encoding through [`PluginManifestV1::encode_json`] preserves any
//! manifest fields that the v1 contract does not know about (e.g.
//! `future_metadata`). The known fields (`abi_version`,
//! `required_capabilities`, `exports`) are written in the canonical order
//! so two equal manifests produce byte-stable serialisations.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use serde::Serialize;

use crate::abi::HIVE_EXTISM_ABI_V1;

/// A single export declared in a v1 Plugin manifest.
///
/// The wire format for each export is `{"name": "...", "input": "json",
/// "output": "json"}`. The v1 contract accepts only `json` for both
/// `input` and `output`; any other value is rejected at
/// [`PluginManifestV1::parse_and_validate`] time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportV1 {
    name: String,
    input: String,
    output: String,
}

impl ExportV1 {
    /// The export name (e.g. `lookup`).
    ///
    /// Inputs: none.
    /// Outputs: the canonical, trimmed export name.
    /// Error modes: this method is total.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The input encoding accepted by the export. v1 only supports `json`.
    ///
    /// Inputs: none.
    /// Outputs: the input encoding tag.
    /// Error modes: this method is total.
    pub fn input(&self) -> &str {
        &self.input
    }

    /// The output encoding produced by the export. v1 only supports `json`.
    ///
    /// Inputs: none.
    /// Outputs: the output encoding tag.
    /// Error modes: this method is total.
    pub fn output(&self) -> &str {
        &self.output
    }
}

/// Validated v1 Plugin manifest.
///
/// The manifest is built exclusively through
/// [`PluginManifestV1::parse_and_validate`], which rejects malformed
/// inputs *before* any host file or database write. Once constructed,
/// the manifest is immutable: re-encoding through
/// [`PluginManifestV1::encode_json`] preserves the validated fields and
/// any unknown fields that the host was carrying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifestV1 {
    abi_version: String,
    required_capabilities: Vec<String>,
    exports: Vec<ExportV1>,
    extra_fields: BTreeMap<String, serde_json::Value>,
}

impl PluginManifestV1 {
    /// Parse and validate a v1 manifest from raw bytes.
    ///
    /// Inputs:
    /// - `bytes`: the raw manifest body.
    /// - `host_capabilities`: the set of capability identifiers the host
    ///   is willing to grant. Manifest capabilities not present in this
    ///   set trigger the `capability_unavailable` issue.
    ///
    /// Outputs:
    /// - A validated [`PluginManifestV1`] on success.
    ///
    /// Error modes:
    /// - [`ManifestError`] listing every independently detectable
    ///   compatibility issue. The returned manifest is *not* used in
    ///   this case; hosts MUST surface the issues and refuse to import
    ///   the artifact.
    ///
    /// Validation is additive: structural failures (parse error,
    /// non-object root, wrong field type) are reported as a single
    /// stable issue and the parser stops. Once the envelope shape is
    /// correct, capability issues are accumulated (every bad
    /// capability is reported) and exports are validated one by one
    /// (the first structural issue short-circuits the per-export loop
    /// but the duplicate-name check still runs).
    pub fn parse_and_validate(
        bytes: &[u8],
        host_capabilities: &BTreeSet<String>,
    ) -> Result<Self, ManifestError> {
        let mut issues: Vec<Issue> = Vec::new();

        // 1. Parse JSON.
        let value: serde_json::Value = match serde_json::from_slice(bytes) {
            Ok(value) => value,
            Err(_) => {
                issues.push(Issue::new("manifest_json_invalid"));
                return Err(ManifestError { issues });
            }
        };

        // 2. Root must be a JSON object.
        let map = match value.as_object() {
            Some(map) => map,
            None => {
                issues.push(Issue::new("manifest_not_object"));
                return Err(ManifestError { issues });
            }
        };

        // 3. `abi_version` must be a string.
        let abi_version = match map.get("abi_version") {
            Some(serde_json::Value::String(s)) => s.clone(),
            _ => {
                issues.push(Issue::new("abi_version_type"));
                return Err(ManifestError { issues });
            }
        };

        // 4. `abi_version` must be the v1 constant.
        if abi_version != HIVE_EXTISM_ABI_V1 {
            issues.push(Issue::new("unsupported_abi"));
            // Continue: capabilities and exports still need to be checked
            // so the host has the full set of compatibility issues.
        }

        // 5. `required_capabilities` must be an array.
        let capabilities_value = match map.get("required_capabilities") {
            Some(serde_json::Value::Array(array)) => array.clone(),
            _ => {
                issues.push(Issue::new("required_capabilities_type"));
                return Err(ManifestError { issues });
            }
        };

        // 6. Walk every capability and accumulate issues.
        let mut seen_caps: BTreeSet<String> = BTreeSet::new();
        let mut normalized_caps: Vec<String> = Vec::new();
        for cap in &capabilities_value {
            let cap_str = match cap {
                serde_json::Value::String(s) => s,
                _ => {
                    issues.push(Issue::new("capability_type"));
                    continue;
                }
            };
            let normalized = cap_str.trim().to_owned();
            if normalized.is_empty() {
                issues.push(Issue::new("capability_blank"));
                continue;
            }
            if !host_capabilities.contains(&normalized) {
                issues.push(Issue::new("capability_unavailable"));
                continue;
            }
            if !seen_caps.insert(normalized.clone()) {
                issues.push(Issue::new("capability_duplicate"));
                continue;
            }
            normalized_caps.push(normalized);
        }

        // 7. `exports` must be an array.
        let exports_value = match map.get("exports") {
            Some(serde_json::Value::Array(array)) => array.clone(),
            _ => {
                issues.push(Issue::new("exports_type"));
                return Err(ManifestError { issues });
            }
        };

        // 8. `exports` must be non-empty.
        if exports_value.is_empty() {
            issues.push(Issue::new("exports_empty"));
            return Err(ManifestError { issues });
        }

        // 9. Validate each export. The first structural issue wins;
        // once we hit a structural error, subsequent exports are
        // skipped but the duplicate-name check still runs against the
        // exports we did accept.
        let mut exports: Vec<ExportV1> = Vec::new();
        let mut seen_names: BTreeSet<String> = BTreeSet::new();
        let mut first_structural: Option<&'static str> = None;
        let mut saw_duplicate = false;

        for export in &exports_value {
            if let Some(code) = first_structural {
                let _ = code;
                continue;
            }
            let export_map = match export.as_object() {
                Some(map) => map,
                None => {
                    first_structural = Some("export_type");
                    continue;
                }
            };
            let name = match export_map.get("name") {
                Some(serde_json::Value::String(s)) => s,
                _ => {
                    first_structural = Some("export_name_type");
                    continue;
                }
            };
            let name_trimmed = name.trim();
            if name_trimmed.is_empty() {
                first_structural = Some("export_name_blank");
                continue;
            }
            let input = match export_map.get("input") {
                Some(serde_json::Value::String(s)) => s,
                _ => {
                    first_structural = Some("export_input_type");
                    continue;
                }
            };
            if input != "json" {
                first_structural = Some("export_input_unsupported");
                continue;
            }
            let output = match export_map.get("output") {
                Some(serde_json::Value::String(s)) => s,
                _ => {
                    first_structural = Some("export_output_type");
                    continue;
                }
            };
            if output != "json" {
                first_structural = Some("export_output_unsupported");
                continue;
            }
            let normalized = name_trimmed.to_owned();
            if !seen_names.insert(normalized.clone()) {
                saw_duplicate = true;
                continue;
            }
            exports.push(ExportV1 {
                name: normalized,
                input: "json".to_owned(),
                output: "json".to_owned(),
            });
        }

        if let Some(code) = first_structural {
            issues.push(Issue::new(code));
            return Err(ManifestError { issues });
        }
        if saw_duplicate {
            issues.push(Issue::new("export_duplicate"));
            return Err(ManifestError { issues });
        }
        if !issues.is_empty() {
            return Err(ManifestError { issues });
        }

        // 10. Collect unknown fields for round-trip preservation.
        let extra_fields: BTreeMap<String, serde_json::Value> = map
            .iter()
            .filter(|(key, _)| {
                let key = key.as_str();
                key != "abi_version" && key != "required_capabilities" && key != "exports"
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();

        Ok(PluginManifestV1 {
            abi_version,
            required_capabilities: normalized_caps,
            exports,
            extra_fields,
        })
    }

    /// The `abi_version` of the manifest.
    ///
    /// Inputs: none.
    /// Outputs: the validated ABI version string.
    /// Error modes: this method is total.
    pub fn abi_version(&self) -> &str {
        &self.abi_version
    }

    /// The validated and de-duplicated capabilities required by the Plugin.
    ///
    /// Inputs: none.
    /// Outputs: the canonical (sorted-by-insertion-after-dedup, trimmed)
    /// capability list.
    /// Error modes: this method is total.
    pub fn required_capabilities(&self) -> &[String] {
        &self.required_capabilities
    }

    /// The validated exports declared by the Plugin.
    ///
    /// Inputs: none.
    /// Outputs: a slice of [`ExportV1`].
    /// Error modes: this method is total.
    pub fn exports(&self) -> &[ExportV1] {
        &self.exports
    }

    /// Re-encode the manifest as JSON, preserving any unknown fields.
    ///
    /// Inputs: none.
    /// Outputs: the canonical JSON serialisation of the manifest.
    /// Error modes: this method is total; the only failure mode
    /// (`serde_json` serialisation of primitive maps) is unreachable
    /// for the validated types.
    pub fn encode_json(&self) -> Vec<u8> {
        let mut map = serde_json::Map::new();
        map.insert(
            "abi_version".to_owned(),
            serde_json::Value::String(self.abi_version.clone()),
        );
        map.insert(
            "required_capabilities".to_owned(),
            serde_json::Value::Array(
                self.required_capabilities
                    .iter()
                    .map(|cap| serde_json::Value::String(cap.clone()))
                    .collect(),
            ),
        );
        map.insert(
            "exports".to_owned(),
            serde_json::to_value(&self.exports)
                .expect("ExportV1 serialises to a fixed JSON object"),
        );
        for (key, value) in &self.extra_fields {
            map.insert(key.clone(), value.clone());
        }
        serde_json::to_vec(&serde_json::Value::Object(map))
            .expect("a serialised manifest is a JSON object with primitive values")
    }
}

/// A single compatibility issue detected during manifest validation.
///
/// Each issue has a stable, snake-case code. The code is the only payload
/// the host needs to pattern-match on; the optional human-readable
/// explanation is purely for logs and UI text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    code: &'static str,
}

impl Issue {
    /// Construct a new issue with a stable, snake-case code.
    ///
    /// The set of codes is closed and documented in the manifest
    /// contract. Hosts MUST treat unknown codes as a forward-compat
    /// warning rather than a hard failure.
    pub(crate) fn new(code: &'static str) -> Self {
        Self { code }
    }

    /// The stable, snake-case code for this issue.
    ///
    /// Inputs: none.
    /// Outputs: the issue code as a static string.
    /// Error modes: this method is total.
    pub fn code(&self) -> &str {
        self.code
    }
}

/// Aggregate of every compatibility issue detected by
/// [`PluginManifestV1::parse_and_validate`].
///
/// Hosts MUST surface every issue before refusing to import the
/// manifest, so the operator can fix all problems in one pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError {
    issues: Vec<Issue>,
}

impl ManifestError {
    /// The list of issues detected.
    ///
    /// Inputs: none.
    /// Outputs: a slice of [`Issue`]. The order is stable for a given
    /// input, but hosts SHOULD treat the set as unordered.
    /// Error modes: this method is total.
    pub fn issues(&self) -> &[Issue] {
        &self.issues
    }
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let codes: Vec<&str> = self.issues.iter().map(Issue::code).collect();
        write!(formatter, "manifest validation failed: {codes:?}")
    }
}

impl std::error::Error for ManifestError {}
