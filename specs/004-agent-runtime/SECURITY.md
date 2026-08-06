# Security Model — Agent Runtime

> **范围更新（2026-07-23）：** 原管理端测试聊天、admin session/SSE 所有权模型及 `/api/admin-chat*`、`/api/chat/sessions*` 已由 `81a84fe` 移除并 superseded。第 6 节仅保留历史边界，不得据此恢复管理端 Chat。

**Status**: Current for Agent Runtime；admin Chat security model superseded
**Audience**: Operators / Reviewers / Plugin authors
**Last Updated**: 2026-07-24

## 1. Threat model summary

See `spec.md §Threat Model` for the full TM-1..TM-5 list. This document is the
operational counterpart — *how* the controls are enforced in code.

| Threat | Control | Where enforced |
|---|---|---|
| TM-1 Plugin tampering / ambient host access | sha256 verify before instantiate + Extism WASI disabled | `runtime/pool.rs::acquire` + `runtime/invoker.rs::build_extism_plugin` (FR-002/FR-029) |
| TM-2 Capability bypass | Zero-trust deny default | `runtime/capability.rs::dispatch` (FR-003) |
| TM-3 SSRF via network.http | Hostname allowlist + resolve→private-IP rejection | `runtime/capabilities/network_http.rs` |
| TM-4 SQL injection via db.execute | Named-query only — no free SQL | `runtime/capabilities/db.rs` |
| TM-5 Privilege escalation | Dangerous capability + main agent edit require Super | `services/agent.rs::check_dangerous_permissions` + `MAIN_AGENT_IDENTIFIER` guard |

## 2. Capability auth chain

Every Plugin → host_call flows through:

```
host_call(envelope_str)
  → capability::dispatch
    1. parse envelope JSON           → 4000 BadRequest on malformed
    2. lookup capability name         → 4045 unknown if not registered
    3. load Agent.permissions         → 4030 denied if not granted;
                                        5000 + one safe audit if DB lookup fails
    4. handler dispatch (per-cap)     → 4xx/5xx on handler-specific failure
    5. exactly one sanitized tracing event + best-effort runtime_audit_logs row
       (success | error | denied | timeout)
```

The Plugin Invoker creates its audit guard before the Plugin DB lookup. Its
drop/finalize paths therefore cover missing Plugin rows, pool acquire, S3 load,
sha256 mismatch, WASM compilation, cancellation/panic and normal invocation
without duplicating the event. The audit record contains static error
classification only—never Plugin input, URL/payload, connection details, raw
S3/Extism errors or digest values. `RuntimeExecutionContext` alone decides
whether the safe tracing event also gets a best-effort DB copy, so a nested Hook
remains tracing-only on these early failures.

Capability handler failures cross the dispatcher boundary as a private typed
classification. Only internal `InvalidArguments` and `Timeout` variants can
select 4001/4081; provider content and arbitrary downstream strings cannot
impersonate either class by containing words such as `timeout`. The reply,
top-level tracing `error_kind` and allowlisted audit payload reuse the same
static mapping, while private details are discarded before serialization.

Pool-slot accounting is cancellation-safe after checkout: caller cancellation
signals Extism and releases the per-Plugin/global permit exactly once. FIFO
waiters time out as 5009 and increment `wait_count`; no path may exceed either
capacity. The cache stores only `CompiledPlugin`; every invocation creates a
fresh Store/Instance, so memory, mutable globals and tables never cross calls.
Capacity counts only `in_use + reserved`; idle compiled entries consume no
per-Plugin or global permit. `cache_misses` increments only after a successful
real compilation, and `created_total` only after a fresh Store/Instance is
successfully created.
The idle reaper starts with the service, evicts only unheld expired compiled
entries and stops during graceful shutdown. SIGINT/SIGTERM first stop accept,
then drain active requests for at most 30 seconds; the service next stops the
reaper, drops the pool/cache, and closes the DB pool even when drain times out.

Extism is built with WASI disabled. A Plugin cannot import
`wasi_snapshot_preview1` to access ambient files, clocks, randomness or sockets;
it must use the registered capability bridge. Extism PDK environment imports
and `extism:host/user.host_call` remain available.

This applies uniformly whether the Plugin is invoked via:
- Function/Tool from Agent's tool-calling loop (`runtime/orchestrator.rs`)
- POST /api/functions/:id/invoke direct RPC
- Workflow node execution (`runtime/workflow.rs::execute_node`)
- generate_answer_node LLM call (node_config 中的 model_preset 解析)

## 3. Dangerous capability grant flow

```
┌─────────────────┐
│ Capability list │  is_dangerous = true:
│  (static)       │     network.http / db.execute / secret.get
└────────┬────────┘
         │
         │ system_admin (role=2) creates Agent with permissions=[...]
         ▼
┌────────────────────────────────────────────────────────┐
│ services/agent.rs::check_dangerous_permissions          │
│   for each cap in permissions:                          │
│     if registry.is_dangerous(cap) && actor_role != 3:   │
│       reject with InsufficientPermission (2001)         │
└─────────────┬──────────────────────────────────────────┘
              │
              ▼
       only Super (role=3) succeeds
              │
              ▼
┌────────────────────────────────────────────────────────┐
│ DB row written to agent_permissions                     │
│ + V004 admin_audit_logs row (003 admin audit — actor + entity) │
└────────────────────────────────────────────────────────┘
```

Frontend `CapabilityPicker` disables dangerous checkboxes for non-Super users
with a Tooltip explanation. Defense-in-depth: both UI + backend reject.

**Required Capabilities declaration**（实际 V009–V011）：Function（V009）、Workflow（V010）、Tool / Skill（V011）均可声明 `required_capabilities`。系统在创建/包装时校验：
- Function 的 required_capabilities 每项必须属于 capabilities 表（不存在 → 5002）
- Tool 包装 Function/Workflow 时，Tool 的 required_capabilities 必须为被包装实体的超集
- Agent 绑定 Tool/Skill 时不校验（运行时由 capability dispatcher 按 Agent.permissions 鉴权）

## 4. Main agent protections

`agents.identifier = 'main'` (id=1) is seeded by V014 and is:
- **Not deletable**: `services/agent::delete` rejects with `CannotDeleteMainAgent` (5001)
- **Edit-restricted**: non-Super updates of system_prompt / permissions return 2001
- **Always present**: V014 seeds it with the system entry configuration

`route_to_subagent` only routes to **direct children** of the current agent
(not arbitrary agents in the tree) — enforced in
`runtime/orchestrator.rs::handle_route_tool`.

## 5. Plugin upload pipeline (FR-005)

```
POST /api/plugins (multipart)
  1. Bearer JWT check (role ≥ System)
  2. multipart size limit (axum::DefaultBodyLimit, default 16 MB)
  3. magic-byte check '\0asm'           → 4000 BadRequest on miss
  4. PLUGIN_MAX_BYTES check (env)        → 4000 if exceeded
  5. sha256 over uploaded bytes
  6. uniqueness check (identifier, version) → 4090 Conflict on dup
  7. S3 put (plugins/<identifier>/<version>.wasm)
  8. DB INSERT with sha256, s3_key, size_bytes
  9. On DB failure: spawn fire-and-forget S3 delete to avoid orphan blobs
```

## 6. Legacy admin SSE chat ownership（已 Superseded）

原 `services/chat::check_ownership` 的 admin/Super ownership bypass、per-admin SSE counter 和错误码 4291 都随管理端 Chat 删除，不是现行安全控制。不得恢复以下任一行为：

- `/api/admin-chat*` 或 `/api/chat/sessions*`
- Super 读取其他管理员测试会话
- `CHAT_SSE_MAX_CONCURRENT_PER_ADMIN` 驱动的 admin SSE 流限制

现行普通用户聊天由 `/api/assistant`、`/api/newsession`、`/api/messages` 提供，会话/消息按 `user_id` 隔离并使用 `chat_sessions_user` / `chat_messages_user`；其签名、鉴权与限流由外部 Assistant API 特性负责。

## 7. Audit retention (FR-022)

`runtime_audit_logs` retained for `AUDIT_RETENTION_DAYS` (default 36500 days /
100 years, configurable).
Background cron: `cargo run --bin audit-retention` (deploy as systemd unit /
Kubernetes Deployment). Each pass deletes rows where `occurred_at <
DATE_SUB(UTC_TIMESTAMP(), INTERVAL N DAY)`.

The retention process requires `AUDIT_RETENTION_DATABASE_URL`; it never falls
back to HiveWeb's general `DATABASE_URL`. Provision three database identities:

```sql
-- Migration identity: owns DDL and is not used by either long-running process.

-- HiveWeb identity: runtime audit is append-only and readable by the Super API.
GRANT INSERT, SELECT ON hiveweb.runtime_audit_logs TO 'hiveweb_app'@'%';
REVOKE UPDATE, DELETE ON hiveweb.runtime_audit_logs FROM 'hiveweb_app'@'%';

-- Retention identity: can delete expired audit rows and nothing else.
CREATE USER 'hiveweb_audit_retention'@'%' IDENTIFIED BY RANDOM PASSWORD;
GRANT DELETE ON hiveweb.runtime_audit_logs TO 'hiveweb_audit_retention'@'%';
```

The HiveWeb identity's CRUD privileges for non-audit tables must be granted
explicitly. A database/global `UPDATE` or `DELETE` grant is additive in MySQL
and would defeat the table boundary; verify the final state with `SHOW GRANTS`.
Inject the retention URL only into the systemd unit/Kubernetes Deployment that
runs `audit-retention`, using a mode-0600 environment file or Secret volume.

The worker exits non-zero after emitting the static
`runtime_audit_retention_failed` error kind when its initial or later cleanup
fails. This makes a revoked/missing `DELETE` grant visible to the service
manager instead of leaving a permanently unhealthy loop running.

All non-Hook runtime audit events are emitted to tracing first and then offered
to one bounded, non-blocking persistence worker. A full queue or DB failure
drops only the persistent copy and emits a static error kind; raw DB errors are
not logged. Hook execution remains tracing-only. `GET /api/runtime/pool/stats`
exposes enqueue, persist, tracing-only, queue/writer/no-writer drop and
persistence-failure counters; the Dashboard raises an alert for any drop or
failure counter.

An ignored real-MySQL permission test is retained in
`src/bin/audit_retention.rs`. It requires disposable
`AUDIT_WRITER_TEST_DATABASE_URL` (`INSERT+SELECT`) and
`AUDIT_RETENTION_TEST_DATABASE_URL` (`DELETE` only) identities. Enabling these
CI secrets and turning that ignored test green remains pending infrastructure
work; the normal test suite stays offline.

现行 `chat_messages_user` 由父表 `chat_sessions_user` 级联删除；`chat-retention` 的具体保留期属于外部 Assistant API 运行配置，不继承已 superseded 的 admin Chat 所有权契约。

`CHAT_RETENTION_DAYS` 与 `CHAT_RETENTION_INTERVAL_SEC` 只接受严格正整数；
`0`、负数和畸形值在连接数据库前 fail-fast，避免紧循环或未来 cutoff。
数据库连接或 DELETE 失败时，进程记录静态 `chat_retention_failed` 并以非零
状态退出，由 service manager 告警/重启。

开发/维护 CLI 也遵循最小暴露面：

- `create-super-admin`、`seed` 和 `api-test --body-file` 在 Unix 上以
  `O_NOFOLLOW` 打开一次，再通过已打开 fd 校验 regular file、当前用户 owner、
  `0600` 或更严格权限以及单一 hard link；非 Unix 文件模式 fail closed，可改用
  stdin。
- `create-super-admin` 是 INSERT-only bootstrap：重复 phone 以静态错误非零退出，
  不更新 nickname/password，也不输出 success；已有账户只能走认证后的修改密码
  流程。它与 `seed` 均复用正式 `utils::password` 的 6–20 Unicode code point、ASCII
  字母+数字校验和 bcrypt 哈希，不维护 CLI 私有规则。
- `api-test` 推荐 `--body-stdin` / `--body-file`，旧 argv body 仅做有弃用警告的
  兼容；无论响应成功或失败，均不打印 request/response body、签名输入、签名或
  signed URL。
- `seed-bench` 独立受 `bench-tools` feature gate 保护；只有安全解析后的 MySQL
  database 名以 `_test` / `_bench` 结尾且 `--confirm-destructive` 精确重复该名称
  才可连接。生成账户使用每次运行随机且不输出的密码，不存在固定可登录口令。

## 8. Environment variable surface (security-relevant subset)

| Var | Default | Purpose |
|---|---|---|
| `NETWORK_HTTP_ALLOWLIST` | empty | Comma-separated exact hostnames; leading-dot aliases and wildcards are rejected; **empty = network.http disabled entirely** |
| `SECRET_ALLOWLIST` | empty | Comma-separated env var names readable via secret.get |
| `JWT_SECRET` | (placeholder) | **MUST** be rotated per deployment |
| `PLUGIN_MAX_BYTES` | 16777216 | Cap per-Plugin upload bytes |
| `PLUGIN_CALL_TIMEOUT_MS` | 30000 | Non-zero wall-clock limit enforced by Extism and an outer Tokio deadline |
| `PLUGIN_CALL_MAX_MEMORY_MB` | 128 | WASM linear memory cap（×16 换算为 64 KiB pages；超限→5000+tracing audit+丢弃 fresh Store/Instance） |
| `PLUGIN_CALL_FUEL` | 10000000000 | Non-zero Wasmtime instruction budget for CPU-bound loops |
| `LLM_PRESETS_PATH` | `./llm_presets.toml` | File must exist with exactly one valid default preset before Router/listener startup |
| `LLM_NODE_TIMEOUT_MS` | 25000 | Per-provider wall-clock ceiling; fallback starts only while chain budget remains |
| `LLM_CHAIN_TIMEOUT_MS` | 45000 | Ordinary full-chain ceiling; `llm.invoke` is independently capped at 25000 ms inside the 30 s Plugin budget |
| `AUDIT_RETENTION_DATABASE_URL` | required for retention worker | Dedicated MySQL account with only `DELETE` on `runtime_audit_logs`; no `DATABASE_URL` fallback |
| `AUDIT_RETENTION_DAYS` | 36500 | Runtime audit retention in days (100-year default, configurable) |
| `AUDIT_RETENTION_INTERVAL_SEC` | 86400 | Retention scan interval in seconds |
| `CHAT_RETENTION_DAYS` | 30 | Chat session retention days; must be a positive integer |
| `CHAT_RETENTION_INTERVAL_SEC` | 86400 | Chat retention scan interval; must be a positive integer |

> `CHAT_SSE_MAX_CONCURRENT_PER_ADMIN` 是已 superseded 的 admin SSE 配置，不属于 004 现役安全变量面。

LLM registry 加载失败只记录静态 `error_kind` 与文案，不输出配置路径、TOML
片段、provider 参数或凭据。缺失或无效 default 必须在监听端口前非零退出，
不得回退到空 registry；无效非 default preset 仅静态告警并跳过。只有
`model_preset=NULL` 使用 default；显式未知名称必须 fail closed，避免在未授权
情况下切换到另一组模型。fallback tracing/audit 只能记录稳定 provider identity、
`actual_model`、`fallback_used` 与静态 `reason`，不能记录 provider 原始错误。
本地文本兜底使用独立 `llm_local_fallback` 事件，不能冒充 provider fallback。

## 9. Physical migration chain and security controls (V001–V033)

物理迁移的唯一真值是 `crates/hiveweb/migrations/*.sql` 与
`src/bin/migrate.rs::MIGRATIONS`。当前链为 V001–V033；V013 是刻意保留的
空号（无文件、无注册项），V032 创建 runtime audit，V033 仅规范 Workflow
timeout 默认值。旧文档中的“V019–V038 Agent Runtime extensions”是已合并或
重映射的历史规划标签，不是可创建、重命名或补号的物理迁移。

| Version | Actual physical migration | Security / runtime role |
|---|---|---|
| V001 | `create_admins_table` | 管理员身份、角色、状态与密码摘要 |
| V002 | `create_login_records_table` | 登录成功/失败记录与来源审计 |
| V003 | `seed_super_admin` | 初始 Super 管理员种子 |
| V004 | `audit_logs` | 直接创建 `admin_audit_logs`；不存在 V028 rename |
| V005 | `categories` | Category 层级元数据 |
| V006 | `capabilities` | Capability 元数据、危险标记与 category 外键 |
| V007 | `tags` | Tags / polymorphic taggings |
| V008 | `plugins` | Plugin 元数据、S3 key、sha256 与软删除 |
| V009 | `functions` | Function schema、Plugin 外键与 `required_capabilities` |
| V010 | `workflows` | Workflow、四类节点、边映射、输入/输出描述与 `required_capabilities` |
| V011 | `tools_skills` | Tool / Skill 的 `source`、`is_always`、category 与 `required_capabilities` |
| V012 | `agents` | Agent 树、Tool/Skill 关联及 `agent_permissions` |
| V013 | **reserved gap** | 无 SQL 文件、无注册项；不得补号 |
| V014 | `seed` | main Agent 与 Capability 静态元数据种子 |
| V015 | `recommended_games` | 历史推荐游戏表 |
| V016 | `seed_capability_categories` | Capability category 种子与回填 |
| V017 | `users_table` | 普通用户身份表 |
| V018 | `split_chat_tables` | 创建普通用户聊天表；迁移明确未实现 admin Chat |
| V019 | `create_global_configs` | 全局配置表；不是 Tool `source` 迁移 |
| V020 | `create_games_table` | 游戏目录表；不是 Tool `is_always` 迁移 |
| V021 | `create_game_alias_entries_table` | 游戏别名表；不是 Skill `is_always` 迁移 |
| V022 | `create_agent_hooks_table` | Agent Hook 配置 |
| V023 | `create_hook_executions_table` | Hook execution 历史表；现行 Hook runtime 保持 tracing-only |
| V024 | `add_extensions_to_chat_messages_user` | 普通用户消息扩展字段 |
| V025 | `create_sensitive_words` | 敏感词表 |
| V026 | `seed_sensitive_words` | 敏感词种子 |
| V027 | `add_uid_nickname_to_users` | 普通用户 UID / nickname |
| V028 | `drop_phone_password_status_from_users` | 移除普通用户 phone/password/status；不是 audit rename |
| V029 | `game_category_json` | 推荐游戏 category JSON 演进；不是 required-capabilities 迁移 |
| V030 | `recommended_games_channel` | 推荐游戏 channel 演进 |
| V031 | `game_image_text` | 推荐游戏 image 字段演进 |
| V032 | `runtime_audit_logs` | 非 Hook runtime 的脱敏、best-effort 持久审计 |
| V033 | `normalize_workflow_timeout_default` | Workflow DB 默认 timeout 规范为 33000 ms |

### 9.1 `source = 'builtin'` immutability (V011)
- `tools.source = 'builtin'` 的 Tool 不可编辑，只能包装 builtin Function (kind=1)
- 后端 service 层在 PUT/POST tools 时校验 source，builtin → 拒绝修改
- 防止有编辑权限的用户篡改系统级 Tool

### 9.2 `is_always` implicit authorization (V011)
- `tools.is_always = 1` 的 Tool 对所有 Agent 自动可用，无需 `agent_tools` 关联
- `skills.is_always = 1` 的 Skill 对所有 Agent 自动加载，无需 `agent_skills` 关联
- **风险**：如果 is_always=1 的 Tool 包装了需要危险 capability 的 Function，所有 Agent 均可间接调用 — 需确保 is_always Tool 仅包装安全 Function
- 当前无审计记录 is_always 隐式授权链（见 §10 Known gaps）

### 9.3 Required capabilities enforcement (V009–V011)
- Function/Tool/Workflow/Skill 声明 `required_capabilities` 后，系统在创建/包装时校验：
  - Function 的 required_capabilities 每项必须属于 capabilities 表
  - Tool 的 required_capabilities 必须为被包装实体的超集
- 运行时由 dispatcher 按 Agent.permissions 鉴权，不额外拦截

### 9.4 Workflow node type security (V010)
- `generate_answer_node` 的 `node_config` 可包含 system_prompt — 可被有编辑权限的用户植入 prompt injection
- `start_node` / `end_node` 不引用 Function，function_id 可为 NULL — 无额外安全风险

### 9.5 Admin and runtime audit trails (V004 / V032)
- V004 直接创建 `admin_audit_logs`，与 V032 的 `runtime_audit_logs` 明确区分
- admin_audit_logs 记录管理员对配置数据的写操作
- runtime_audit_logs 记录运行时 capability 调用 / agent 路由 / workflow 执行

## 10. Known gaps (to address before production)

- [x] **Plugin 双层 timeout** — Extism manifest timeout + Wasmtime fuel
      interrupt CPU-bound Wasm，外层 `tokio::time::timeout` 到期时显式调用
      `CancelHandle::cancel()`、立即释放池占用计数并返回 5004；被中断实例不回池。
      Extism epoch cancellation 不能抢占正在执行的同步 Rust host handler，因此
      capability I/O handler 仍必须设置自己的操作级 timeout。
- [ ] **secret.get** is currently env-backed allowlist; a proper KMS / vault
      backend (e.g., HashiCorp Vault, AWS Secrets Manager) recommended for
      production credential rotation.
- [ ] **JWT rotation** — current impl uses static `JWT_SECRET`. Adding key ID
      claim + rotation schedule would close the long-lived-token risk.
- [x] **Runtime correlation forwarding** — HTTP entry points create one
      explicit `RuntimeExecutionContext`; request_id/session_id survive
      Assistant → Orchestrator → Hook/Workflow → Invoker → Capability.
      Entering Hook permanently switches that chain to tracing-only.
- [x] **Capability-only Plugin boundary** — Extism explicitly disables WASI;
      real-WASM regression coverage rejects WASI imports while exercising the
      registered PDK `host_call`.
- [x] **Early runtime audit coverage** — capability permission DB failures and
      Plugin lookup/pool/S3/SHA/compile early exits enter an exactly-once safe
      tracing audit guard before returning.
- [x] **Invoker cancellation/reset accounting** — caller abort cancels the guest,
      releases the checked-out slot without re-pooling or double release, and a
      failed reset converts only the safe Plugin audit to `outcome=error` while
      preserving an already successful Plugin output.
- [ ] **is_always Tool/Skill bypass audit** — is_always=1 的 Tool/Skill 对所有 Agent 自动可用，当前无审计日志记录其隐式授权链。建议在 capability dispatch 时记录 "隐式授予 via is_always"。
- [ ] **Workflow answer_node prompt injection** — `generate_answer_node` 的 node_config.system_prompt 可被有 Workflow 编辑权限的用户修改，应增加 content-security 审核或版本化追踪。

## 11. Reporting security issues

Contact: see top-level `SECURITY.md` (repo root) or
`security@` mailbox per project policy.
