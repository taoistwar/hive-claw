# hive-claw

Two-crate Rust workspace:

- **`crates/hiveclaw`** — placeholder agent exposing `POST /v1/responses`
  (wire-compatible with the [OpenClaw OpenResponses HTTP API](https://docs.openclaw.ai/gateway/openresponses-http-api)),
  built on `axum` + Tokio.
- **`crates/hivegui`** — Simplified-Chinese (`zh-CN`) desktop client
  built on [`gpui`](https://github.com/zed-industries/zed/tree/main/crates/gpui),
  hosting a conversation surface with HiveClaw and two helper-tool
  sections (Day+1, Hour+1) that ship empty in v1.

## Quickstart

The full bootstrap walkthrough — system prerequisites, build, run, log
locations, failure-path smoke test — lives in
[`specs/001-hiveclaw-hivegui/quickstart.md`](specs/001-hiveclaw-hivegui/quickstart.md)
(also mirrored at [`docs/quickstart.md`](docs/quickstart.md)).

```bash
cargo build --workspace      # builds both crates (requires gpui system deps for hivegui)
cargo run -p hiveclaw        # terminal A: starts the axum service on 127.0.0.1:8686
cargo run -p hivegui         # terminal B: opens the desktop window
```

## Layout

```text
Cargo.toml               workspace manifest
rust-toolchain.toml      pinned stable channel
crates/
  hiveclaw/              binary crate: axum HTTP service
  hivegui/               binary crate: gpui desktop client
specs/
  001-hiveclaw-hivegui/  feature spec, plan, contracts, tasks, quickstart
.specify/                constitution + spec-kit templates
.github/workflows/ci.yml fmt + clippy + test gates
```

## Constitution

[`.specify/memory/constitution.md`](.specify/memory/constitution.md) is
authoritative. Highlights: test-first development, p95 < 200ms API
budget, structured logging, ≤3 deployable units, Rust-only stack
(axum / gpui / Tokio / SQLite / sled).

## Status

v1: HiveClaw is a placeholder; HiveGUI is a real client whose two
helper-tool series ship empty. Real Hive task development / debugging
behaviour and concrete tools land in later features.
