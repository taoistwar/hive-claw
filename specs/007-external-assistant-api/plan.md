# Implementation Plan: 对外 Assistant API

**Branch**: `004-agent-runtime` | **Date**: 2026-06-02 | **Spec**: [specs/007-external-assistant-api/spec.md](spec.md)
**Input**: Feature specification from `/specs/007-external-assistant-api/spec.md`

## Summary

在 hiveweb 上新增对外公开的 `POST /api/assistant` 端点，为外部系统提供 AI 对话能力。请求采用 MD5 签名鉴权，流程：Content-Type → 签名校验 → 参数校验 → 外部 DB 用户/会员验证 → Redis 日限流 → 内部用户同步 → Main Agent (AgentRunner) 编排处理。

## Technical Context

**Language/Version**：Rust 1.85+  
**Primary Dependencies**：`axum`, `sqlx` (内外双 pool), `redis`, `crates/agent` (AgentRunner), `providers` (LLM), `bcrypt`, `md5`, `serde_json`, `chrono`  
**Storage**：内部 MySQL (hiveweb), 外部只读 MySQL (cloud_computer), Redis (日计数)  
**Testing**：cargo test — 16 unit tests passed, 6 integration tests pending (需完整基础设施)  
**Performance Goals**：SC-001 (10s timeout), SC-006 (100 QPS / 95%)  
**Scale/Scope**：单一端点, 无 session, 每次请求独立  

## Constitution Check

✅ I. Code Quality — 单模块 ~480 行含测试, 函数职责清晰  
⚠ II. Test-First — 实现先于集成测试 (Complexity Tracking 已登记)  
✅ III. UX Consistency — 统一 JSON 响应格式  
⚠ IV. Performance — LLM 延迟偏离 (同 003/004)  
✅ V. Simplicity — 复用 crates/agent  
✅ VI. Observability — tracing 结构化日志  
✅ Security — MD5 签名 + 参数白名单 + env 注入  
✅ Stack — 纯 Rust, 无新增技术栈, md5 为轻量 crate  

**Gate**: CONDITIONAL PASS

## Complexity Tracking

| Violation | Why | Alternative Rejected |
|-----------|-----|---------------------|
| TDD 顺序偏离 | pre-spec 原型 + clarify 增量修改 | 严格 TDD 无法支持快速探索 |
| LLM 延迟 | 外部 API 依赖, p95 不可控 | 同 003/004 偏离 |

## Project Structure

```
crates/hiveweb/src/api/assistant.rs   # 实现 + 16 单元测试
crates/hiveweb/src/api/mod.rs         # AppState.ext_pool + public route
crates/hiveweb/src/main.rs            # EXTERNAL_DB_URL pool
crates/hiveweb/Cargo.toml             # +md5
crates/hiveweb/.env.example           # 2 new env vars
crates/hiveweb/tests/common/mod.rs    # test_app() 签名更新
```

## Phase 0: Research (all resolved)

| 决策 | 结论 |
|------|------|
| 外部 DB | create_pool(), max_connections=5 |
| MD5 签名 | md5 crate, handler 内 |
| Agent | AgentRunner + Main Agent (id=1) |
| TTL | 到次日 0 点 |
| VIP | effective_end_time >= now |

## Phase 1: Data Model & Contracts

无新建表。外部只读 cloud_user + cc_user_membership。内部复用 users, global_configs, agents。合约见 `contracts/api.md`。

## Phase 2: Tasks — 29/35 完成

- ✅ T001-T023: Setup + Core + Limit + Clarify + Degrade
- ✅ T024-T025, T027-T028, T033: 16/16 unit tests passed
- ⏳ T026, T029-T032, T034-T035: 6 integration tests (need infra)
