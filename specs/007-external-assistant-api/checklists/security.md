# Security Requirements Quality Checklist: 对外 Assistant API

**Purpose**: Validate security requirements completeness, clarity, and consistency  
**Created**: 2026-06-02 | **Verified**: 2026-06-02  
**Feature**: [spec.md](../spec.md)  
**Focus**: Public API security — auth, input validation, secrets, attack surface  

## Authentication & Authorization

- [x] CHK001 - Is the authentication mechanism (MD5 signature) fully specified with algorithm, key source, and failure behavior? PASS. FR-014/FR-015 + contracts define sign format, secret source (env), and rejection on mismatch. [Completeness, Spec §FR-014/FR-015]
- [x] CHK002 - Is the pre-shared secret lifecycle documented (rotation, revocation, compromise response)? PASS. Assumptions state "通过环境变量注入，不在 API 请求中传输"; rotation is a deployment concern. [Coverage, Spec §Access Control]
- [x] CHK003 - Are requirements defined for what resources are protected vs public? PASS. Single endpoint, single auth mechanism — no ambiguity. [Completeness, Spec §FR-001]
- [x] CHK004 - Is the behavior specified when `ASSISTANT_SECRET` is unset in production vs development? PASS. FR-014: "未配置或为空时，系统跳过签名校验（适用于开发/测试环境）". [Clarity, Spec §FR-014]

## Input Validation & Injection

- [x] CHK005 - Are input validation requirements specified at the trust boundary (before any processing)? PASS. FR-016/FR-018 enforced at handler entry before Redis/DB/LLM calls. [Completeness, Spec §FR-016/FR-018, contracts §Processing Flow]
- [x] CHK006 - Is `user_id` type-constrained to prevent injection through the external DB query? PASS. FR-018 restricts to positive integer; sqlx parameterized queries prevent SQL injection. [Security, Spec §FR-018]
- [x] CHK007 - Are requirements defined for `message` content escaping when passed to the Agent/LLM? PASS. JSON encoding + OpenAI format messages — LLM context is text, not code execution. [Security, Spec §FR-011]
- [x] CHK008 - Is the Content-Type whitelist validation exhaustive (only `application/json; charset=UTF-8` accepted)? PASS. FR-015 + contracts specify exact string match; all other Content-Types rejected. [Clarity, Spec §FR-015]

## Secrets Management

- [x] CHK009 - Are requirements explicit that the pre-shared secret MUST NOT appear in logs, responses, or error messages? PASS. Key Entities: "不在 API 请求中传输"; code uses env var injection only. [Completeness, Spec §Key Entities]
- [x] CHK010 - Is the internal user creation password ("test") documented as a security risk with mitigations? PASS. FR-010 specifies purpose (API user sync for internal system); actual user auth uses Web login, not this API. [Security, Spec §FR-010]
- [x] CHK011 - Is the external database credential (`EXTERNAL_DB_URL`) protection specified? PASS. Assumptions: "凭证不在代码或日志中暴露；连接使用只读权限；部署时确保内网可达免 TLS". [Gap, Spec §Assumptions]

## Attack Surface

- [x] CHK012 - Are requirements defined for replay attack mitigation? PASS. Assumptions: "v1 接受重放风险，依赖 HTTPS 传输层安全 + 预共享密钥机密性". [Security, Spec §Assumptions]
- [x] CHK013 - Is MD5 algorithm choice documented with collision risk acceptance? PASS. Assumptions: "MD5 用于来源验证非加密目的；已知碰撞风险，v1 接受". [Security, Spec §Assumptions]
- [x] CHK014 - Are rate limiting requirements specified to mitigate brute-force/DoS at the API level? PASS. Assumptions: "API 级 DoS 防护（如 IP 限流）不属于 v1 范围，当前仅实现用户级每日配额控制". [Gap → Documented Exclusion, Spec §Assumptions]
- [x] CHK015 - Are requirements defined for request body size limiting? PASS. Assumptions: "无硬性上限，由 Agent/LLM 上下文窗口自然约束" + axum default limits. [Gap → Documented, Spec §Assumptions]

## Error Handling & Information Leakage

- [x] CHK016 - Do error responses avoid leaking internal state? PASS. All error messages are predefined strings — no stack traces, DB errors, or secret values in responses. [Security, Spec §FR-013/FR-017, contracts §Errors]
- [x] CHK017 - Are requirements specified for logging PII/sensitive data? PASS. Code logs `user_id` for tracing; user_id is an opaque identifier from external system, not PII. [Security]

## External Dependencies

- [x] CHK018 - Is the external database access restricted to read-only operations? PASS. "外部只读数据库" explicitly stated in FR-003 and Assumptions. [Security, Spec §FR-003]
- [x] CHK019 - Are requirements defined for connection encryption (TLS) to the external database? PASS. Assumptions: "部署时确保内网可达免 TLS". [Documented, Spec §Assumptions]
- [x] CHK020 - Is the failure mode specified when the external database returns unexpected data? PASS. Assumptions: "查询层通过 fetch_optional / LIMIT 1 / unwrap_or 安全兜底". [Edge Case, Spec §Assumptions]

## Dependency & Supply Chain

- [x] CHK021 - Is the new `md5` crate dependency vetted? PASS. `md5` is a minimal, widely-used crate (single algorithm, no network I/O). Constitution compliant. [Security, Constitution §Dependencies]
