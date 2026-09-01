# `wayland-scanner` Quick-XML 0.41 Compatibility Patch

This file records the reviewed upstream source, the minimal compatibility
patch, and the audit trail for the local copy of `wayland-scanner` that
the HiveGUI CI uses as the `RUSTSEC-2026-0194/0195` remediation.

## Upstream source

The vendored crate is a byte-for-byte copy of the upstream release
published on `crates.io`, except for the minimal `quick-xml` 0.39 → 0.41
compatibility patch documented below. The original upstream repository
is the smithay/wayland-rs workspace.

```toml
upstream_repository = "https://github.com/smithay/wayland-rs"
upstream_crate = "wayland-scanner"
upstream_version = "0.31.10"
license = "MIT"
```

## Minimal compatibility patch

`quick-xml` 0.41 renamed the `GeneralRef::xml_content` helper to
`xml10_content` to make the XML version explicit. The single call site
in `wayland-scanner/src/parse.rs` is the only behavioural change:

```rust
// before
} else if let Ok(content) = byte_ref.xml_content() {
    if let Some(s) = quick_xml::escape::resolve_xml_entity(&content) {
        copyright.push_str(s);
    }
}

// after
} else if let Ok(content) = byte_ref.xml10_content() {
    if let Some(s) = quick_xml::escape::resolve_xml_entity(&content) {
        copyright.push_str(s);
    }
}
```

The corresponding `Cargo.toml` change pins `quick-xml` to `0.41` so
the patched call compiles against the new API.

No other behavioural change is introduced. All other source files in
`src/` are byte-for-byte identical to the upstream release tarball.

## CI integration

- `Cargo.toml` `[patch.crates-io]` routes `wayland-scanner` to this
  local copy so the foundation dependency graph resolves the audited
  source.
- The Foundation CI is the only consumer. The local copy is excluded
  from the `cargo vendor` step because the patch is reviewable in
  place.
- Any future `wayland-scanner` upgrade must be reviewed against the
  `quick-xml` 0.41 contract and re-baselined here.
