//! Plugin manifest v1 helpers for HiveGUI.
//!
//! Bridges the HiveGUI-side WASM inspection (which only extracts function
//! exports) to the shared `hive-runtime-core::plugin` v1 manifest contract.
//! Provides three total, side-effect-free operations:
//! - [`build_v1_manifest`] — construct a v1 manifest from exports + capabilities.
//! - [`manifest_exports`] — extract export names from either the legacy
//!   string-array shape or the v1 object-array shape.
//! - [`validate_manifest`] — validate a manifest against a host capability
//!   set and return the stable issue codes on failure.

use std::collections::BTreeSet;

use hive_runtime_core::abi::HIVE_EXTISM_ABI_V1;
use hive_runtime_core::plugin::PluginManifestV1;

/// Build a v1 Plugin manifest from extracted WASM exports and declared
/// capability requirements.
///
/// The `abi_version` is pinned to [`HIVE_EXTISM_ABI_V1`]; every export is
/// emitted as `{"name": ..., "input": "json", "output": "json"}` because
/// the v1 contract only supports the `json` encoding.
pub fn build_v1_manifest(exports: &[String], required_capabilities: &[String]) -> String {
    let exports_json: Vec<serde_json::Value> = exports
        .iter()
        .map(|name| {
            serde_json::json!({
                "name": name,
                "input": "json",
                "output": "json",
            })
        })
        .collect();
    serde_json::json!({
        "abi_version": HIVE_EXTISM_ABI_V1,
        "required_capabilities": required_capabilities,
        "exports": exports_json,
    })
    .to_string()
}

/// Extract export names from a manifest, accepting both the legacy
/// `"exports": ["run", "health"]` string-array shape and the v1
/// `"exports": [{"name": "run", ...}]` object-array shape.
///
/// Unknown or malformed entries are skipped; empty names are dropped.
pub fn manifest_exports(manifest: Option<&str>) -> Vec<String> {
    let Some(value) =
        manifest.and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
    else {
        return Vec::new();
    };
    let Some(exports) = value.get("exports").and_then(|exports| exports.as_array()) else {
        return Vec::new();
    };
    exports
        .iter()
        .filter_map(|export| match export {
            serde_json::Value::String(name) => Some(name.clone()),
            serde_json::Value::Object(map) => map
                .get("name")
                .and_then(|name| name.as_str())
                .map(str::to_owned),
            _ => None,
        })
        .filter(|name| !name.is_empty())
        .collect()
}

/// Extract the declared `required_capabilities` from a manifest.
///
/// Only the v1 shape carries `required_capabilities`; legacy string-array
/// manifests (or malformed entries) yield an empty list. Non-string
/// entries are skipped.
pub fn manifest_required_capabilities(manifest: Option<&str>) -> Vec<String> {
    let Some(value) =
        manifest.and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
    else {
        return Vec::new();
    };
    let Some(caps) = value
        .get("required_capabilities")
        .and_then(|caps| caps.as_array())
    else {
        return Vec::new();
    };
    caps.iter()
        .filter_map(|cap| cap.as_str().map(str::to_owned))
        .filter(|cap| !cap.is_empty())
        .collect()
}

/// Validate a manifest against the host capability set.
///
/// Returns `Ok(())` when the manifest is compatible, or `Err` with the
/// stable issue codes (see [`PluginManifestV1::parse_and_validate`]) when
/// the manifest is malformed, targets an unsupported ABI, requires an
/// unavailable capability, or declares an invalid export.
pub fn validate_manifest(
    manifest: &str,
    host_capabilities: &BTreeSet<String>,
) -> Result<(), Vec<String>> {
    match PluginManifestV1::parse_and_validate(manifest.as_bytes(), host_capabilities) {
        Ok(_) => Ok(()),
        Err(err) => Err(err
            .issues()
            .iter()
            .map(|issue| issue.code().to_owned())
            .collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn build_v1_manifest_pins_abi_and_json_encodings() {
        let manifest =
            build_v1_manifest(&["run".into(), "health".into()], &["network.http".into()]);
        let value: serde_json::Value = serde_json::from_str(&manifest).expect("valid JSON");
        assert_eq!(value["abi_version"], "hive-extism/v1");
        assert_eq!(
            value["required_capabilities"],
            serde_json::json!(["network.http"])
        );
        assert_eq!(
            value["exports"],
            serde_json::json!([
                {"name": "run", "input": "json", "output": "json"},
                {"name": "health", "input": "json", "output": "json"},
            ])
        );
    }

    #[test]
    fn manifest_exports_reads_legacy_string_array() {
        let exports = manifest_exports(Some(r#"{"exports":["run","health"]}"#));
        assert_eq!(exports, ["run", "health"]);
    }

    #[test]
    fn manifest_exports_reads_v1_object_array() {
        let manifest = build_v1_manifest(&["run".into(), "health".into()], &[]);
        let exports = manifest_exports(Some(&manifest));
        assert_eq!(exports, ["run", "health"]);
    }

    #[test]
    fn manifest_exports_skips_malformed_entries() {
        let exports = manifest_exports(Some(
            r#"{"exports":["run",{"name":"health"},{"name":"","input":"json","output":"json"},7]}"#,
        ));
        assert_eq!(exports, ["run", "health"]);
    }

    #[test]
    fn validate_manifest_accepts_valid_v1() {
        let manifest = build_v1_manifest(&["run".into()], &["network.http".into()]);
        assert!(validate_manifest(&manifest, &caps(&["network.http"])).is_ok());
    }

    #[test]
    fn validate_manifest_reports_unavailable_capability() {
        let manifest = build_v1_manifest(&["run".into()], &["network.http".into()]);
        let err = validate_manifest(&manifest, &caps(&["fs.read"])).unwrap_err();
        assert_eq!(err, ["capability_unavailable"]);
    }

    #[test]
    fn manifest_required_capabilities_reads_v1_array() {
        let manifest =
            build_v1_manifest(&["run".into()], &["network.http".into(), "fs.read".into()]);
        let caps = manifest_required_capabilities(Some(&manifest));
        assert_eq!(caps, ["network.http", "fs.read"]);
    }

    #[test]
    fn manifest_required_capabilities_empty_for_legacy_shape() {
        let caps = manifest_required_capabilities(Some(r#"{"exports":["run"]}"#));
        assert!(caps.is_empty());
    }

    #[test]
    fn validate_manifest_reports_legacy_shape_as_invalid() {
        // The legacy `{"exports":["run"]}` shape is not a v1 manifest and
        // must be rejected (no abi_version / required_capabilities).
        let err = validate_manifest(r#"{"exports":["run"]}"#, &caps(&[])).unwrap_err();
        assert!(
            err.contains(&"abi_version_type".to_string()),
            "got: {err:?}"
        );
    }
}
