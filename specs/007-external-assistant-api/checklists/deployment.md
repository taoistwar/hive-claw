# Deployment Requirements Quality Checklist: 对外 Assistant API

**Purpose**: Validate deployment/configuration requirements completeness, clarity, and consistency  
**Created**: 2026-06-02  
**Feature**: [spec.md](../spec.md)  
**Focus**: Environment config, startup, rollback, migration  

## Environment Configuration

- [x] CHK001 - Are all required environment variables explicitly listed with descriptions? PASS. `EXTERNAL_DB_URL` and `ASSISTANT_SECRET` both documented in plan.md + .env.example. [Completeness, plan.md, .env.example]
- [x] CHK002 - Is the default behavior specified for each optional env variable? PASS. FR-014: "未配置 ASSISTANT_SECRET 时跳过签名校验". External DB: "未配置时返回 Assistant service unavailable". [Clarity, Spec §FR-012/FR-014]
- [x] CHK003 - Are env variable naming conventions consistent with existing project patterns (UPPER_SNAKE_CASE)? PASS. `EXTERNAL_DB_URL`, `ASSISTANT_SECRET` follow hiveweb conventions. [Consistency]

## Startup & Initialization

- [x] CHK004 - Is the startup failure behavior specified when critical dependencies are unavailable? PASS. External DB connection failure logs warning but does NOT block startup (FR-012: "不影响主系统稳定性"). [Completeness, Spec §FR-012]
- [x] CHK005 - Are requirements defined for the migration that created the `global_configs` table (V044)? PASS. Migration creates table + seeds default config values (normal_ask_times=5, vip_ask_times=50). [Completeness, V044 migration]
- [x] CHK006 - Is the `md5` crate dependency addition documented with version and build impact? PASS. Cargo.toml: `md5 = "0.7"`; plan.md §Research notes it as "lightweight, no transitive deps." No build impact. [Completeness, Cargo.toml, plan.md §Research]

## Rollback & Compatibility

- [x] CHK007 - Is the API backwards-compatible with no existing clients to break? PASS. New endpoint — no backwards compatibility concerns. [Completeness]
- [x] CHK008 - Are requirements defined for rolling back the `AppState` change (ext_pool field)? PASS. `ext_pool: Option<MySqlPool>` is Optional — missing env var = None, graceful degradation. [Completeness, Spec §FR-012]
- [x] CHK009 - Is the `create_router()` signature change documented with impact on existing test code? PASS. `tests/common/mod.rs` updated to pass `None` as 4th argument. [Completeness, tests/common/mod.rs]

## Testing Readiness

- [x] CHK010 - Is the test environment configuration documented (which env vars needed for tests)? PASS. Unit tests (16) require no env; integration tests (6) marked `#[ignore]` with requirement comments. [Completeness, assistant.rs §tests]
- [x] CHK011 - Are the 6 pending integration tests documented with their required infrastructure? PASS. Each `#[ignore]` test has an explicit requirement string ("requires MySQL + Redis + external DB"). [Clarity, tasks.md §Phase 6]

## Documentation

- [x] CHK012 - Is the API usage documented with a complete curl example? PASS. contracts/api.md includes signature algorithm, example curl commands, request/response formats. [Completeness, contracts/api.md]
- [x] CHK013 - Are error codes and their meanings documented for client developers? PASS. contracts/api.md has Business Errors, Validation Errors, and System Errors tables with message strings. [Completeness, contracts/api.md]
