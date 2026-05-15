# Quickstart: HiveClaw Agent & HiveGUI Client (v1 scaffold)

**Audience**: a new engineer joining the project on a fresh Linux or
macOS machine.
**Goal**: from a clean checkout, run HiveClaw and HiveGUI locally,
exchange one message in the conversation surface, and see structured
logs land in the right place — **in under 15 minutes** (SC-001).

> v1 ships **two empty tool series** (Day+1 and Hour+1). The home screen
> shows them as navigable sections with an empty-state message. This is
> intentional, not a bug.

## 1. Prerequisites (one-off, ~5 min)

- A POSIX machine: Linux x86_64 (primary) or macOS arm64 (secondary).
  Windows is out of scope for v1.
- `git` ≥ 2.30.
- The Rust toolchain pinned by `rust-toolchain.toml` at the repo root.
  Install [`rustup`](https://rustup.rs/) once and let it auto-install
  the pinned channel the first time you run `cargo` inside the
  repository.
- Linux only: GUI dev libraries that `gpui` links against
  (`libxkbcommon`, `libwayland`, `libvulkan`, `pkg-config`,
  `build-essential`). Install them via your distro's package manager.
- macOS only: Xcode Command Line Tools (`xcode-select --install`).

## 2. Clone & build (~3 min on a warm cache, ~8 min cold)

```bash
git clone <repo-url> hive-claw
cd hive-claw
cargo build --workspace
```

The first build will resolve the workspace, download crates, and
produce two binaries under `target/debug/`:

- `target/debug/hiveclaw`
- `target/debug/hivegui`

No source-controlled file is edited as part of this step (SC-005).

## 3. Run HiveClaw (Terminal A)

```bash
# Optional: change the bind address (default 127.0.0.1:8686).
# export HIVECLAW_BIND_ADDR=127.0.0.1:8686

cargo run -p hiveclaw
```

You should see a structured log line on stderr like:

```json
{"timestamp":"2026-05-14T08:01:23.456Z","level":"INFO","fields":{"message":"HiveClaw listening","bind_addr":"127.0.0.1:8686","version":"0.1.0"},"target":"hiveclaw"}
```

Sanity-check the contract endpoint from a third terminal:

```bash
curl -sS -X POST http://127.0.0.1:8686/v1/responses \
  -H 'Content-Type: application/json' \
  -d '{"model":"openclaw:hiveclaw-placeholder-v1","input":"hello"}' | jq
```

You should get a JSON response shaped like the `Response — synchronous`
section of `contracts/openresponses-v1.md`.

## 4. Run HiveGUI (Terminal B)

```bash
# Optional: point HiveGUI at a non-default HiveClaw.
# export HIVECLAW_URL=http://127.0.0.1:8686

cargo run -p hivegui
```

A native window opens with:

- A conversation area on the main pane.
- Two navigable sections labelled **Day+1 工具** and **Hour+1 工具**,
  each rendering a zh-CN empty-state message ("暂无工具" or similar
  — see `crates/hivegui/src/ui/strings_zh.rs` for the canonical copy).

Type a message in the conversation editor (FR-007a) and press the
**发送** button — or press **Enter** to submit (Shift+Enter inserts a
literal newline). You should see:

1. Your message appear in the thread, marked as sent by you.
2. An in-progress indicator (FR-008).
3. HiveClaw's placeholder reply appear below it within ~3 seconds
   (SC-002 budget).
4. The send button re-enable only after the reply lands (FR-008a).

### Attaching files (FR-007b)

Click **添加文件** to attach one or more files to the next message.
Each file is base64-encoded inline into the OpenResponses request as
an `input_file` content item. v1 enforces three hard limits at the
input surface (before any network call):

- Per-file size: **1 MiB**. Larger files are rejected inline with
  `文件超过 1 MiB 限制：<filename>`.
- Total attachment size per turn: **4 MiB**. Exceeding the cap is
  rejected inline with `附件总大小超过 4 MiB 限制`.
- Maximum attachments per turn: **8**. Beyond the 8th, the 添加文件
  affordance is disabled.

A successful attach renders as a chip below the editor showing
`filename (size, mime)`; click 移除 on a chip to drop it before sending.

When the request lands, HiveClaw's placeholder reply is enriched with
the documented attachment metadata. Example:

```text
HiveClaw 占位回复：已收到你的请求。附件：query.hql (1.2 KiB, text/plain), schema.json (4.0 KiB, application/json)
```

v1 does **not** enforce a MIME allow/deny-list. The engineer is
trusted to attach what they need. A richer policy will be added when a
specific threat model emerges.

## 5. Where are the logs?

- **HiveClaw**: stderr only (Terminal A above).
- **HiveGUI**: stderr (Terminal B) **and** a rotating JSON-lines log
  file under the platform's per-user app-data directory:
  - Linux: `$XDG_DATA_HOME/hivegui/logs/` (default
    `~/.local/share/hivegui/logs/`).
  - macOS: `~/Library/Application Support/hivegui/logs/`.

  Files rotate daily. Each line is one JSON object containing
  `timestamp`, `level`, `target`, `conversation_id`, `request_id`,
  `operation`, `outcome`, `duration_ms` (per FR-012b and Constitution
  Principle VI).

## 6. Smoke-test the failure path

While HiveGUI is still running, stop HiveClaw (Ctrl-C in Terminal A).
In HiveGUI, send another message. You should see:

- A clear zh-CN error attached to that turn ("HiveClaw 不可达，请检查
  服务是否运行" or similar — final copy in `strings_zh.rs`).
- A visible **重试** affordance on that failed turn (spec Edge Cases
  / FR-008a).
- No auto-retry. The button does nothing until you click it.

Restart HiveClaw, click **重试** on the failed turn, and the placeholder
reply should land.

## 7. Run the tests

From the repo root:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three commands MUST pass on a fresh checkout (Constitution Workflow
Quality Gates). The contract tests under
`crates/hiveclaw/tests/contract_responses_*.rs` enforce the wire format
in `contracts/openresponses-v1.md`.

## 8. Common gotchas

- **`cargo run -p hivegui` fails to open a window on Linux**: usually
  a missing system library (`libxkbcommon-dev`, `libvulkan-dev`).
  Re-check section 1.
- **HiveGUI shows "HiveClaw 不可达" immediately**: HiveClaw isn't
  running, or `HIVECLAW_URL` points at the wrong address. Verify with
  the `curl` command in section 3.
- **Logs are empty in the app-data dir**: check
  `HIVEGUI_LOG_LEVEL=debug` and make sure the directory is writable
  by your user.

## What's NOT in v1

- No persisted conversation history (close HiveGUI = lose the
  conversation; this is by design — spec Assumption).
- No Day+1 or Hour+1 tools (both series are empty by design).
- No auth, no multi-user, no shared deployment (FR-015).
- No Windows build (out of scope for v1).

These are tracked as follow-up features and will land via their own
specs.
