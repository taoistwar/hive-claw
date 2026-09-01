# Provenance

- Upstream package: `extism 1.30.0`
- Upstream repository: <https://github.com/extism/extism>
- Upstream release revision: `7038ad1c427fa3b25bf0f5d9439490cbb218e262`
- crates.io archive SHA-256: `4b66cd9ac5c64b49c9bac69db3d1b10d8f9386e7caab73489c2f197ba43d5e05`
- Security advisory removed: RUSTSEC-2026-0222
- Upstream migration pull request: <https://github.com/extism/extism/pull/905>
- Applied migration commits:
  - `2e660c111791ac01f8cda84ca2e140cbda1a107a`
  - `bb7752ba1ae5269d5aa3415b91fb8b8a70293b97`

The local patch applies the two upstream Wasmtime 46 migration commits to the
published Extism 1.30.0 runtime crate, pins `wasmtime` and `wasi-common` to the
reviewed advisory-free 46.x patch release (`46.0.3`), and removes the obsolete direct
`wiggle` dependency. The migration updates only Wasmtime API compatibility and
fuel accounting during Extism setup; the public Extism API and Hive Plugin ABI
are unchanged.

One build-only compatibility edit gates `std::io::Read` behind Extism's
existing `register-http` feature. HiveGUI/HiveWeb disable that feature, so the
unconditional import was otherwise unused; runtime behavior is unchanged.

The upstream pull request was still Draft when this patch was adopted. This
copy therefore remains a temporary, reviewable security patch. Remove the
workspace `[patch.crates-io]` entry and this directory once Extism publishes a
release using an advisory-free Wasmtime version and the HiveGUI/HiveWeb shared
fixture, timeout, fuel, pool, and host-call regression suites pass unchanged.
