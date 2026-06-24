# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

HiveClaw is a multi-agent AI platform with a web admin center, plugin/WASM runtime, and desktop GUI. The backend is a Rust monorepo with a React/TypeScript admin frontend (`web-admin/`).

## Build & Test Commands

```bash
# Build the server (default-members focuses on CLI crate)
cargo build                     # builds crates/cli only
cargo build -p hiveweb          # build the admin center server
cargo build --workspace         # build everything (needs system libs for GUI)

# Run server
export DATABASE_URL=mysql://... REDIS_URL=redis://...
cargo run -p hiveweb            # starts on port from HIVEWEB_PORT or 3000

# Database management
cargo run -p hiveweb --bin migrate          # run DB migrations
cargo run -p hiveweb --bin seed             # seed test data
cargo run -p hiveweb --bin create-super-admin

# Tests
cargo test -p hiveweb                        # all hiveweb tests
cargo test -p hiveweb --test it_agent_hook   # single integration test
cargo test -p hiveweb --lib -- builtins      # test builtins module
cargo test -p agent --lib -- context         # test agent context module

# Lint & Format
cargo fmt --all
cargo clippy -- -D warnings

# Frontend (web-admin/)
cd web-admin && npm run dev        # dev server on port 5173
cd web-admin && npm run build      # production build

# Check without building
cargo check -p hiveweb
```

## Architecture

### Crate Map

| Crate | Purpose |
|---|---|
| `hiveweb` | **Admin center backend** — Axum HTTP server, REST API, orchestrator, runtime |
| `agent` | **Agent framework** — AgentLoop, AgentContext (state carrier), tools, hooks |
| `providers` | **LLM abstraction** — OpenAI/Anthropic/DeepSeek providers, streaming, retry |
| `hivegui` | Desktop GUI (gpui-based, needs system libraries) |
| `hiveclaw` | OpenResponses-compatible agent placeholder |
| `cli` | CLI entry point (default workspace member) |
| `nanobot` | Legacy Python-based agent (Docker, being phased out) |

### Server Crate (`hiveweb`) Structure

```
hiveweb/src/
├── api/          # Axum route handlers (one file per resource)
├── services/     # Business logic + DB queries (one file per resource)
├── models/       # DB row structs, request/response types
├── runtime/      # Agent execution engine (see below)
├── middleware/   # Auth, rate limiting
├── db/           # SQL migrations
├── storage/      # S3 client wrapper
├── cache/        # Redis client
└── utils/        # Error types, JWT, pagination helpers
```

**Pattern**: Each domain entity (workflow, agent, tool, skill, etc.) follows a three-layer pattern:
`api/<entity>.rs` → `services/<entity>.rs` → `models/<entity>.rs`. API handlers are thin — they parse requests, call services, and return responses.

### Runtime Engine (`hiveweb/src/runtime/`)

The runtime is a **host-side execution environment** for tools, workflows, and agent orchestration:

- **`orchestrator.rs`** — Multi-hop agent loop: LLM call → tool execution → route → repeat. Hooks fired at lifecycle points (`before_agent_start`, `before_tool_call`, `after_agent_end`, etc.). Uses AgentContext for unified state.
- **`workflow.rs`** — DAG executor: topological BFS with parallel layer execution. Nodes can be `function_node`, `generate_answer_node` (LLM-based), `start_node`, `end_node`.
- **`builtins/`** — Host-native function registry. Each builtin is one file (e.g., `game_list.rs`, `game_info.rs`) implementing `BuiltinDef` with handler + schemas. Registered at startup via `ensure_registered()`.
- **`hook.rs`** — Hook execution engine. Three action types: `call_function`, `call_workflow`, `http_webhook`. Uses `HookDeps` (contains AgentContext) + `HookContext` (agent metadata).
- **`invoker.rs`** — Plugin invocation: resolve capability → acquire WASM instance from pool → invoke → audit.
- **`pool.rs`** — Per-plugin WASM instance pool with configurable limits.
- **`input_source.rs`** — `InputSpec` for DAG nodes: fields resolved from `Upstream` (other nodes), `Custom` (literals), or `AgentContext` (runtime state).

### AgentContext (`crates/agent/src/context/`)

Unified in-memory state carrier for an agent execution lifecycle:
- Thread-safe RwLock per category (UserInput, ToolResults, Entities, etc.)
- Supports `add_extension()` for cards/UI content
- `_agent_context_updates` in function/workflow output syncs back via `apply_agent_context_updates()`
- `inject_agent_context_snapshot()` injects read-only `_agent_context` into function inputs

### DAG Node Input System

Workflow nodes resolve input via `input_mapping` (stored in `node_config.input_mapping`):
- `Upstream { node_key, field }` — from another node's output
- `Custom { value }` — literal JSON value
- `AgentContext { category, key, sub_key }` — from runtime state (e.g., `user_input.raw_text`)

`start` is a virtual node whose "output" = `external_input` (the test `body.input` or hook serialized data).

### Key Patterns

- **AgentContext sync**: Functions/workflows write `_agent_context_updates` in output → orchestrator/DAG executor applies via `apply_agent_context_updates()`. Critical that this is called consistently across all invocation paths (tool, hook, workflow-wrap).

- **Hook ↔ Tool AgentContext consistency**: All three workflow invocation paths (`invoke_workflow`, `handle_workspace_tool` kind=2, `execute_call_workflow`) must call both `inject_agent_context_snapshot()` before and `apply_agent_context_updates()` after execution.

- **`crates/` internal dependencies** follow workspace Cargo.toml paths. Internal crates depend on each other via `path = "../<name>"`.

## Git Workflow

- Branch naming: `NNN-feature-name` (e.g., `004-agent-runtime`)
- Feature specs live in `specs/<NNN-feature-name>/` with spec.md, plan.md, tasks.md
- Constitution at `.specify/memory/constitution.md` (v1.3.0) — authoritative governance
