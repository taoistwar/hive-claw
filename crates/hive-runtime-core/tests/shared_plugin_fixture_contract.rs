use std::collections::BTreeSet;

use hive_runtime_core::{
    plugin::PluginManifestV1,
    wasm::{WasmModuleShape, validate_wasm_shape},
};

const SHARED_SMOKE_WASM: &[u8] =
    include_bytes!("../../hivegui/tests/fixtures/plugins/shared-smoke/plugin.wasm");
const SHARED_SMOKE_MANIFEST: &[u8] =
    include_bytes!("../../hivegui/tests/fixtures/plugins/shared-smoke/manifest.json");

#[test]
fn shared_smoke_fixture_satisfies_the_v1_manifest_and_wasm_shape() {
    let host_capabilities = [
        "fs.read",
        "fs.write",
        "log.emit",
        "network.http",
        "time.now",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<BTreeSet<_>>();

    PluginManifestV1::parse_and_validate(SHARED_SMOKE_MANIFEST, &host_capabilities)
        .expect("the shared smoke manifest must satisfy hive-extism/v1");

    let validation = validate_wasm_shape(SHARED_SMOKE_WASM, false);
    assert!(
        validation.is_ok(),
        "the shared smoke WASM must carry the host surface and ABI export: {validation}"
    );
    assert_eq!(validation.rejection_kind(), None::<WasmModuleShape>);
}
