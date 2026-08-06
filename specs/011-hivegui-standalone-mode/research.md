# Research: HiveGUI 独立本地 Agent

**Feature**: `011-hivegui-standalone-mode`
**Date**: 2026-07-23
**Status**: Phase 0 complete; no unresolved technical clarifications

## 0. Rust 工具链基线

**Decision**: 不再采用 Rust 1.85.0。根据 Rust 官方 2026-07-16 发布记录，2026-07-22 的最新稳定版是 1.97.1；workspace 的 `rust-toolchain.toml` 与 `rust-version` 均精确固定为 1.97.1，本地和 CI 必须输出相同版本。这里使用精确版本而不是浮动 `stable`，以保证 `Cargo.lock`、GPUI/Zed 和 Extism/Wasmtime 的构建可复现。

**Rationale**: 当前固定的 Zed/GPUI revision 及 Extism 1.30/Wasmtime 43 依赖图已经要求高于 1.85 的编译器；在保留现有 UI/runtime 代码的前提下，降级整套 GPUI、WGPU、Extism 和 Wasmtime 会带来大范围 API 回退和回归风险。1.97.1 可直接覆盖当前依赖的最高 MSRV，并包含对 1.97.0 LLVM 误编译的修复。

**Source**: [Rust 1.97.1 official release announcement](https://blog.rust-lang.org/2026/07/16/Rust-1.97.1/)

## 0.1 依赖安全前置批次（2026-07-23）

**Decision**: 采用用户批准的方案 A，在 Foundation 生产实现前用一个独立的 Red→review→Green 安全补救批次清除当前 7 个 advisory，不添加 `[advisories].ignore` 或其他临时例外：

- `mysql_async` 精确升级到 `=0.36.2`，关闭默认 feature，只启用 `minimal` 与 `native-tls-tls`。隔离探针在当前 HiveGUI 使用面上得到 0 个 API 编译错误。
- SQLx 与 CI `sqlx-cli` 统一为 `=0.9.0`。workspace 根不再聚合数据库或 TLS feature；HiveGUI 精确启用 `chrono|macros|runtime-tokio|sqlite`（外部 MySQL TLS 由 `mysql_async` 承担，SQLx 不启用 MySQL 或 TLS）。2026-07-23 的历史 Red 最初把 HiveWeb 目标记为 `chrono|derive|json|mysql|runtime-tokio|rust_decimal|tls-rustls-ring-webpki`；2026-07-28 批准的 checked-static-query 方案 A 将最终目标升级为 `chrono|json|macros|mysql|runtime-tokio|rust_decimal|tls-rustls-ring-webpki`。两端 `macros` 已包含 derive，均不重复声明 `derive`；`agent` 移除未使用的 SQLx 依赖，所有图中禁止 `mysql-rsa`。SQLx 0.9 隔离依赖图已不含 `rsa`，但全仓探针仍发现 HiveGUI 1 个、HiveWeb 40 个 0.9 API/SQL 安全类型迁移错误，因此这不是“直接改版本即可”的 Green 结论。
- HiveWeb MySQL 连接改为强制 `VERIFY_IDENTITY`：必须显式提供 CA 并校验目标主机名；`DISABLED`、`PREFERRED`、`REQUIRED`、`VERIFY_CA`、CA/hostname 缺失、TLS 初始化失败或握手失败均拒绝连接，不得回退明文。2026-07-28 批准的查询方案 A 进一步要求固定应用 schema SQL 只使用 SQLx `query!`/`query_as!`/`query_scalar!` 与 offline metadata，有限变体以封闭 enum/`match` 选择静态 checked query，生产 `QueryBuilder` 调用数为 0。`game_service` 的 `category_name` 改为 checked static SQL 中的 `JSON_OBJECT('name', ?)` bind；恶意输入回归已存在于 T017B，T017D 只使 T017G 审批后的新版契约 Green，不新增测试专用 QueryBuilder helper。HiveGUI 外部 MySQL 继续只使用 `mysql_async` prepared values，并让 metadata allowlist 后的 `MysqlIdentifier` 序列化动态标识符。启动期从 `named_queries.toml` 读取的完整 SQL 保留，但只允许一个 reviewed-config 中央边界在拒绝多语句、SQL 注释、placeholder/参数不匹配、重复参数和 select/execute kind 不匹配后构造 `AssertSqlSafe`；其它生产调用点不得构造该类型。
- `zbus_xml =5.2.1` 继续使用已修复版本；根 `[patch.crates-io]` 指向 `third_party/wayland-scanner`，该目录保留上游 `wayland-scanner =0.31.10`、精确小写地址 `https://github.com/smithay/wayland-rs` 与 MIT 来源证明，并只把 `quick-xml` 提升到 `0.41` 及将已更名的 `xml_content` 调整为 `xml10_content`。该最小 backport 与当前 wayland client/protocols、`zbus_xml =5.2.1` 已在隔离探针编译；直接跟随上游 git HEAD 会产生 21 个 API 编译错误，故不采用。
- `aws-sdk-s3` 精确固定为 `=1.133.0`，关闭默认 feature 和旧 `rustls` feature，显式保留 `sigv4a`、`http-1x`、`default-https-client`、`rt-tokio`；隔离 AWS crate 探针可编译，完整 workspace 仍由上述 SQLx 迁移错误阻断。

**Rationale**: 该组合是当前探针中同时消除已知 advisory、保留产品 TLS/数据库能力且迁移范围可审计的最小方案。测试先固定精确 manifest、lockfile、vendored provenance、TLS fail-closed 和 SQL 安全边界；用户于 2026-07-23 批准 Red 证据及上述实施前审计修正。Rust 1.97.1 的 T001 仍是更早的独立 PR/CI 阻断项：独立分支 `codex/rust-1.97.1-toolchain` 的提交 `bf3690d` 已推送，但 PR 创建认证与远端 CI 仍 Pending，不能因分支已存在就视为完成。

**Alternatives considered**:

- 为 7 个 advisory 增加 ignore/到期日：拒绝；用户明确批准零例外方案，且会削弱合并门禁。
- 继续 SQLx 0.8 并维护私有 RSA/TLS fork：拒绝；引入长期密码学维护面，且 0.9 已提供无 `rsa` 的受支持路径。
- 等待新的 wayland-scanner crates.io 发布或直接使用上游 git HEAD：拒绝；前者不能即时清除 advisory，后者在当前依赖组合产生 21 个编译错误；本地最小 backport 有固定来源、精确差异和明确退出条件。
- 保留 AWS SDK 默认 feature：拒绝；会继续激活旧 `rustls` 兼容边，而四个显式现代 client/runtime feature 已满足 S3 调用面。

## 0.2 现役 HiveWeb CI 质量基线（2026-07-23）

**Decision**: 采用用户批准的方案 A，在 T001 取得远端 Green 前先用独立分支 `codex/ci-quality-baseline-rust-1.97.1` 修复 `origin/main` 已存在的 fmt、strict Clippy 和测试基线；删除只覆盖已移除管理聊天 API 的陈旧测试，并让仍然现役的 HiveWeb contract/integration suite 在强制 CI job 中使用一次性 MySQL 8、Redis 7、MinIO、测试 bucket 和已迁移 schema。测试 harness 只读取 `TEST_DATABASE_URL`，数据库名必须含 `test`，且不得加载仓库 `.env`。

**Rationale**: 宪章要求 Rust integration/contract tests 是 CI 合并门禁；把 74 个现役基础设施测试全部 ignore 会直接违反该要求。另一方面，恢复已由历史提交删除的管理聊天 UI/API 会逆转当前产品边界。因此基线只清理已失效契约、离线化本来不需要服务的测试，并为剩余现役测试提供真实隔离基础设施。

**Scope boundary**: 该批次不迁移 SQLx 0.9、不实现 HiveWeb 严格 `VERIFY_IDENTITY`、不加入 secret/dependency scan 或 SQLx offline metadata，也不实现 HiveGUI 外部 MySQL DataSource。T017D/T017E/T018/T038 继续分别拥有这些后续 Green 门禁；T038 必须使用独立 HiveGUI CI job、专用 `HIVEGUI_TEST_MYSQL_URL` 和隔离 schema/account，不得复用 HiveWeb `TEST_DATABASE_URL`、migration、server、client 或运行状态。质量基线合并后，T001 分支必须重基并保留新的 HiveWeb integration job，不能用旧三文件提交覆盖它。

## 1. 产品与依赖边界

**Decision**: HiveGUI 保持独立桌面进程，不依赖 `hiveweb` crate，不请求 HiveWeb HTTP API。共享只通过编译期 Rust library 与兼容性 fixture 完成。

依赖方向：

```text
hivegui ─┬─> agent
         ├─> providers
         ├─> hive-builtins
         └─> hive-runtime-core (new shared library)
hiveweb ─┴─> the same shared libraries
```

HiveWeb 的业务持久化 MySQL、Redis、Rustfs 和 Axum/SSE 留在 HiveWeb adapter；HiveGUI 自身的 SQLite、本地 Plugin 目录、设备密钥、GPUI 事件以及用户显式配置的外部 MySQL DataSource client 留在 HiveGUI adapter。该外部数据源连接不调用 HiveWeb，也不构成 HiveWeb 依赖。

**Rationale**: `crates/hivegui/Cargo.toml` 当前不依赖 hiveweb，这是独立性的正确基础。HiveWeb runtime 的依赖结构直接包含 MySQLPool、S3Client、RedisClient 和 Axum SSE，作为桌面依赖会把云端基础设施带入本地进程。宪法允许在第二个具体调用方出现后新增共享 library。

**Alternatives considered**:

- HiveGUI 依赖 `hiveweb` 的空 `standalone` feature：拒绝；该 feature 无实现且依赖边界错误。
- 把 HiveWeb runtime 全量复制到 HiveGUI：拒绝；现有 ABI、Capability 和 Builtin 名称已经出现漂移。
- 仅共享文档：拒绝；不能在编译期和契约测试中阻止协议漂移。

## 2. 共享 runtime 的最小范围

**Decision**: 新增 `crates/hive-runtime-core`，只承载两个产品已经共同需要的纯运行时不变量：

```text
abi.rs              manifest v1, host_call envelope, stable errors
capability.rs       catalog, product support set, permission gate, handler trait
execution.rs        execution_id, cancellation context, runtime events/outcomes
plugin.rs           limits, Extism invoke/cancel/pool lifecycle
wasm.rs             imports/exports/hash/path validation
workflow.rs         graph validation, mapping, layered scheduler
persisted_tool.rs   persisted Function/Workflow -> agent::Tool adapter
```

产品端先从自己的存储一次性加载值对象，再调用共享执行器；不建立覆盖 MySQL、SQLite、S3 和本地文件的“大一统 repository”。

**Rationale**: ABI、Capability gate、Workflow 图算法、取消和 Extism 生命周期都是真实的第二调用方需求；存储和 UI 则仍然不同。这个边界既实现代码复用，也避免 HiveGUI 通过 shared crate 间接获得 HiveWeb 服务连接。

**Alternatives considered**:

- 只新增 `hive-runtime-contracts` 类型 crate：不足以消除 Workflow/Extism/取消算法复制。
- 把全部能力加入 `agent` crate：拒绝；Extism、持久化 Workflow 和产品 Capability 不属于通用 AgentRunner 的核心职责。
- 抽象全部存储：拒绝；接口过宽，违反 YAGNI。

## 3. Agent、Tool、Skill 与 LLM 复用

**Decision**: HiveGUI 复用 `agent::AgentRunner`、`agent::ToolRegistry`/`Tool`、providers 的 `LLMProvider`/`FallbackProvider` 与 `hive-builtins`。不复用完整 `AgentLoop`，也不复制 HiveWeb 手写 orchestrator。

每轮从 SQLite 构造不可变执行快照：当前 Agent、显式与 `is_always` Tool/Skill、显式 Capability、直接子 Agent和 LLM Preset。持久化 Custom Function/Workflow 临时包装为 `agent::Tool`；Placeholder Function 只提供 prompt/tool schema 调试信息，不进入执行后端，并在 Capability 或 Plugin 解析前返回 `function_not_executable`；Skill markdown 拼入 system prompt。回复、结束和直接子 Agent 路由是运行时控制结果，不创建 Tool 表记录。

**Rationale**: AgentRunner 已有 tool-calling 循环、schema 校验、上下文治理和 provider 抽象。AgentLoop 还绑定 message bus、workspace 工具和文件式 session，HiveGUI 已有不同的 SQLite/GPUI 生命周期。每次路由重新构造快照可确保子 Agent 不继承父权限或模型。

**Alternatives considered**:

- 复制 HiveWeb orchestrator：拒绝；绑定 MySQL/Redis/S3/SSE 且重复 Agent loop。
- 直接使用完整 AgentLoop：拒绝；附带与桌面产品无关的通道、workspace 和 memory 行为。
- 把 Skill 暴露为 LLM Tool：拒绝；违反 Skill 拼入 system prompt 的现有语义。

## 4. 本地 LLM Provider 解析

**Decision**: 把本地 LlmPreset 的 Model 按 priority 转换成 providers crate 的 provider 链。Model 引用独立 LlmProvider；Provider 规范字段沿用当前 HiveGUI Store 的 `name/category/base_url/token_encrypted/token_env`，legacy `kind/api_key_*` 仅在迁移中显式映射。token_env 非空时优先读取环境变量，否则解密 token_encrypted。认证/参数错误不 fallback，429、5xx、网络、TLS 和节点超时可 fallback。

Agent 继续以 `model_preset TEXT` 引用 Preset.name：Agent 保存时验证存在，Preset 重命名在同一事务中级联更新该应用层引用，删除被引用 Preset 时 RESTRICT；这避免本批次引入额外 schema 版本，同时阻止悬空运行时配置。

**Rationale**: `providers::ProviderBuildConfig`、backend registry 和 `FallbackProvider` 已实现多 vendor 适配；HiveGUI 不应再维护 OpenAI-only 请求结构。外部延迟单列 `llm_ms`，不计入本地编排预算。

**Alternatives considered**:

- 在 HiveGUI 用 reqwest 手写 OpenAI Chat Completions：拒绝；丢失现有 provider/fallback/tool-call 支持。
- HiveGUI 请求 HiveWeb 代调模型：拒绝；违反独立性。

## 5. Workflow DAG

**Decision**: 共享层接收完整 `WorkflowGraph`，其稳定 node_type 为 `start_node|end_node|function_node|generate_answer_node`，执行唯一 `start_node`/可达 `end_node`/悬空端点/重复键/环校验，按拓扑层并行执行。产品 adapter 一次加载全部节点与边，NodeExecutor 回调执行 Function/Plugin/LLM；当前写入和运行时不接受短名称别名。

Fail-fast 发生在层边界：当前层已启动节点全部汇总状态；如有失败或取消，稳定选择 node_key 最小的主错误并停止下一层，保留其它已完成结果与未执行列表。

**Rationale**: HiveWeb 已有可参考的拓扑分层与 mapping 解析，但存储加载和调度耦合。其当前错误路径可能提前返回而漏记同层后续结果；新共享实现应先汇总整层。HiveGUI 当前图保存也缺少整体事务。

**Alternatives considered**:

- 把 HiveWeb SQL 从 MySQL 改成 SQLite后复制：拒绝；调度仍与数据库耦合。
- 引入 Temporal/LangGraph：拒绝；本地 DAG 无分布式工作流需求。
- 全部串行：拒绝；不满足同层并行和 100 节点预算。

## 6. Plugin ABI 与 Capability

**Decision**: 共享 `PluginManifestV1`、Call/Reply envelope、稳定错误与 WASM imports/exports 校验器。当前 ABI 名为 `hive-extism/v1`，Plugin 只导入精确允许的 Extism PDK 基础符号和 `host_call`。

Dispatcher 固定顺序为：大小/JSON解析 → Capability 注册 → manifest 声明 → 当前 Agent 权限快照 → 当前产品真实 handler → 参数校验 → handler → 脱敏事件。数据库中的 Capability 元数据不等于产品已实现的 handler。

Plugin 制品永不原位替换。每次导入分配不可复用的唯一键并通过受控根句柄 no-replace 发布；字段/ABI/manifest/Capability 与内容预校验失败时用户表和 operation/GC ledger 均零修改。预校验成功后先持久化含由 operation_id 确定性派生的唯一 `staging_name` 的 `prepared`，再创建 staging；写入、flush/fsync 和身份复核后先记录 `staging_identity`/`staged`，之后才发布、fsync 父目录并记录 new identity/`published`。schema CHECK 要求 `prepared` 两项 identity 均空、`staged` 仅 staging identity 非空、`published|referenced` 两项 identity 均非空，且要求 `done` 满足 `new_identity IS NULL OR staging_identity IS NOT NULL`、`conflict` 不限制两项 identity。`plugin_artifact_operations.operation_kind` 精确区分 `create|replace`：create 的全部 expected-old 字段/revision 始终为空，`plugin_id` 在 `prepared|staged|published` 为空，只有插入用户可见 Plugin 行、回填 plugin_id 和把 operation 标为 `referenced` 的同一事务才能形成用户状态；replace 从 `prepared` 起必须包含 plugin_id、完整旧键/哈希/大小/文件身份和预期 `row_revision`，原始在线操作在发布后用旧 tuple+精确 revision 的 live CAS 事务切换引用并递增 revision。CAS 不命中返回并发冲突，禁止 last-writer-wins。普通元数据更新也递增 revision，因此它与文件更新的竞态不会丢失字段。

启动开放 Plugin Store 前按 operation kind 重放全部非终态操作，并用 `plugin_artifact_gc` 持久记录旧对象或本次发布但未被引用的新对象。`prepared` 无 staging 时以单一 SQLite 事务标记 operation `done`；staging 已出现但 identity 未耐久时把 operation 置 `conflict`、原样保留对象并阻断 Store。`staged` 同时检查 staging/final：仅 staging 匹配时受保护清理并完成 operation；仅 final 与 staging identity/size/hash 匹配时重做父目录 fsync、复验并耐久推进 `new_identity/published`；二者均无时完成 operation；双重存在、任一不匹配或所有权无法证明时置 `conflict` 并保留全部对象。`published` 只接受 staging 不存在且 final 精确匹配已持久化 new identity/size/hash，其它组合一律置 operation `conflict` 并阻断 Store。create 无精确引用新键的用户行时不得重建创建意图，只能以“登记 owned object GC + operation→done”同一事务收敛；replace 仍匹配旧 tuple/revision 时不得重做 live CAS，采用同一登记事务，只有当前已引用 new tuple 才确认提交。`referenced` 是历史提交事实；create 直接完成 operation，replace 以“登记旧对象 GC + operation→done”同事务完成，staging 重现也不回写历史状态，只登记 incident/GC。operation 永远只有六态，歧义固定为 `conflict`。实际 GC 独立使用 `pending|blocked`：worker 在启动、固定周期和引用/租约释放事件后按 artifact_key 稳定扫描；引用/租约等瞬态条件消失且 identity 仍匹配时重试，identity 重现/不匹配或所有权未知持续 blocked，禁止采用竞争 identity。只有零元数据引用、零运行时句柄/租约且身份、link count、大小和哈希全匹配时才删除并 fsync；unlink+fsync 后、ledger 事务前崩溃时重验缺失再删 ledger。操作/GC 状态是内部恢复元数据，不进入可移植备份。

**Rationale**: HiveGUI 当前 catalog 声明约 15 项但主要只实现 `network.http`，其余返回 5010；HiveWeb/HiveGUI 的 `chat_respond`/`chat.respond` 与 envelope 已漂移。共享类型和同一 WASM fixture 才能兑现跨产品兼容。

**Alternatives considered**:

- 每个 Capability 一个 host function：拒绝；新增能力会破坏 ABI。
- 两端独立 catalog：拒绝；已有漂移证据。
- 导入时请求 HiveWeb 检查：拒绝；违反独立性。

## 7. Extism 隔离、资源限制与实例池

**Decision**:

- 精确锁定 Extism `=1.30.0` 且关闭默认 feature；仅用 Wasmtime `=43.0.2` 的 `anyhow` feature 补齐 Extism 1.30.0 发布包遗漏的编译依赖，不启用 Extism HTTP/filesystem 或 `wasmtime-default-features`。Plugin 构造固定使用 `PluginBuilder.with_wasi(false)`。
- 默认 timeout=30s、memory=128MiB、output=10MiB；覆盖硬上限 120s/512MiB/50MiB。
- `memory_limit_mb` 为兼容既有 schema 保留字段名，逻辑单位固定为 MiB；`with_memory_max` 参数按 64KiB WASM page 换算，pages=`memory_limit_mb × 16`，因此128MiB=2048 pages、512MiB=8192 pages，而不是传 MiB 值或字节数。
- 同时启用 manifest timeout、fuel 与外层取消；输出在复制/累积过程中执行硬上限。
- 实例池 key 包含制品 SHA-256、ABI 和有效资源策略；timeout、取消、trap、output 超限或 reset 失败时丢弃实例。
- 冷启动只从 HiveGUI 托管目录读取，在实例化前校验路径、size 和 SHA-256。

**Rationale**: HiveGUI 当前 `.with_wasi(true)`，无 memory/output limit，且外层 timeout 不能停止 `spawn_blocking`。Extism 1.30 提供 `CancelHandle`、fuel、timeout 与 memory pages，足以实现需求，并通过 Wasmtime 43.0.2 移除旧 41.x 安全线的阻断 advisory。实例池避免重复编译，且可复用 HiveWeb 中“校验、reset 失败丢弃”的正确思想而不引入 S3。

**Alternatives considered**:

- 每次调用重新编译：拒绝；重复调用和 100 节点 DAG 开销不可接受。
- 使用 WASI 预开目录：拒绝；规格要求资源只能经 Capability。
- 复用被中断实例：拒绝；guest 状态不可信。

## 8. 协作取消

**Decision**: `ExecutionContext` 持有 execution_id、session/agent 标识、权限快照、事件 sink 与 `CancellationToken`。父 token 派生到 AgentRunner、子 Agent、LLM、Tool、Workflow、Plugin 和 Capability。

AgentRunner 在 LLM、Tool batch、路由和迭代边界检查 token；网络/LLM future 用 `tokio::select!` 响应取消。Plugin 先获得 2 秒协作期，之后通过 Extism CancelHandle 中断，并等待阻塞调用真正退出后丢弃实例。终态持久化为 cancelled，不回滚已完成副作用。

**Rationale**: AgentRunner 当前只有 timeout；SubagentManager 直接 abort；HiveGUI 的 spawn_blocking 在外层 timeout 返回后仍可继续执行。统一 cancellation token 能唤醒异步等待，而 CancelHandle 能真正中断 CPU-bound guest。

**Alternatives considered**:

- 只 abort 外层 Tokio task：拒绝；不能终止运行中的 spawn_blocking WASM。
- 只轮询 AtomicBool：拒绝；不能唤醒 LLM/网络 future。
- UI 停止、后台继续：拒绝；与澄清决议相反。

## 9. SQLite 与迁移

**Decision**: 从 `entity_store.rs` 拆出 `datasource/migrations.rs`、`conversation_store.rs` 和 `backup.rs`。SQLite pool 每个连接启用 foreign_keys、WAL 和 busy timeout；`fs2` 持有单实例写锁。目标 schema v4，支持 v2/v3 顺序迁移。新数据库建成后、每次打开已有数据库以及 staging migration transaction 提交前都调用唯一 SQLite 健康检查：`PRAGMA integrity_check` 必须精确返回单行 `ok`，`PRAGMA foreign_key_check` 必须返回零行。文件级快照或主文件发布前冻结新写入，以独占连接完成非 busy `wal_checkpoint(TRUNCATE)`、关闭连接、分类 sidecar，再 fsync 主文件及父目录。current 固定为数据根句柄下 `datasources.db`/`db_id=current`；migration/restore live instance 固定为 `.hivegui-db-staging-v1/{role}-{UUID}/datasources.db`。建库前通过 manifest staging/final 耐久发布含 schema、role、UUID、完整 db_id、database_name、`ownership_state=unarmed` 的六元组；应用前才 arm 并发布 owner final/staging。启动在 Store 前按 ASCII 字节序 no-follow 先重放 registry retirement/tombstone，再验证 live manifest/owner 与 cleanup。registry、live/tombstone、manifest/owner/retirement final/staging、cleanup journal/quarantine 都不进入 archive、本地安全备份或新树；live locator 只能经 retirement 整目录接管，绝不直接删除 manifest/owner。已提交未 checkpoint WAL 必须完整合并；hot/unknown/recoverable sidecar 在 canonical 名称按字节保留且 Store 不开放。已证明安全的残留只经数据库同目录、SQLite 外部 cleanup journal；其 UTF-8 db_id/长度前缀/domain-separated SHA-256 token、64 位小写 hex、三个 final/三个 `.staging` basename 和 `schema_version=1` 全部固定，损坏/重复 fail-closed。cleanup journal 使用与 instance UUID 分离的 `cleanup_operation_id`，sidecar 以 identity-bound no-replace 移入精确且不复用的 quarantine 名，并按 `prepared→quarantined→done` 五分支逐边界重放；journal 删除前不快照、不进入 `applying`、不开放 Store。禁止盲删，也禁止把含未 checkpoint 已提交帧的主文件单独复制、哈希或切换。

迁移前先验证 source current 健康并按封闭边界创建数据库/Plugin 快照；随后创建 unarmed migration instance，把 current 主文件按字节复制到 staging。v2→v3→v4 的唯一 SQLite transaction 只在 staging 内执行，commit 前/后完成健康、Plugin DDL、派生索引、checkpoint/sidecar/fsync 与重开验证；该 SQLite commit 不修改 current，也不是系统 commit point。staging 全验证后才 arm manifest、发布 owner prepared/applying并切换；新 current 完整验证后才 owner committed。prepared/applying 失败恢复 old，committed 后保 new，均通过 retirement journal + 整目录 outcome tombstone 收口。源库结构损坏/孤儿外键、新于 current、早于 current-2 均不得进入主 UI；迁移目标失败不得误导用户重建 source。LlmStore 迁移错误不得再被 `.ok()` 丢弃。

Staging manifest 是含 `ownership_state=unarmed|armed` 的六元组，owner final/staging 固定为 `.hivegui-db-recovery-v1.json`/`.hivegui-db-recovery-v1.json.staging`；两个 role 共用 `prepared|applying|committed`，且 committed 前已完成新 current 语义验证。只有 unarmed 且 owner final/staging 均无可派生 aborted；armed owner 缺失/被删/staging-only/损坏必须 fail-closed。aborted、terminal old/new 都由 registry-level retirement `prepared|renamed|done` journal 接管，identity-bound rename 整个 live instance 到包含 outcome 的 tombstone，再从 tombstone no-follow 逐叶清理；普通文件 link count=1，目录只做 identity-bound 逐层复核。

**Rationale**: 当前版本为 2，迁移步骤不在单事务中，`current_version >= current` 会错误接受新版数据库，完整性失败只记录日志继续。把迁移从约 3600 行 entity_store 中拆出可测试的顺序步骤能直接满足失败注入要求。

**Alternatives considered**:

- 继续在 Store::new 中散落 CREATE/ALTER：拒绝；无法证明原子 rollback 和兼容窗口。
- 迁移失败后继续运行：拒绝；可能产生混合 schema。

## 10. 会话加密与 100 年保留

**Decision**: ChatSession、ChatMessage、AgentExecution 独立表；正文、Tool 调用和执行状态使用现有设备密钥加密成 BLOB。设备密钥首次启动时使用系统安全随机源原子创建并设为 owner-only 权限；已有密文时密钥缺失、损坏或权限不安全必须阻断，不得静默替换。默认 expires_at 为 created_at 加 100 个日历年；用户修改保留规则只影响新会话，清理既有数据前展示数量并确认。

启动发现遗留 running execution 时标记 failed/interrupted，不自动重放。删除会话级联消息与执行，不影响 Agent 配置。会话可进入整体加密备份，但绝不进入日志或诊断包。

**Rationale**: 当前无会话/执行表；`prompt_debugger` 只是直连 LLM 的调试器，不能替代本地 Agent runtime。持久化最终状态而不重放是避免重复外部副作用的安全选择。

**Alternatives considered**:

- 仅内存会话：拒绝；不满足重启恢复。
- 明文 SQLite + 仅依赖磁盘权限：拒绝；不满足敏感数据要求。
- 自动恢复 running 并重放：拒绝；可能重复副作用。

## 11. 可移植备份

**Decision**: 使用 tar 作为内层容器，使用 `age` 的 passphrase 流式认证加密作为外层单文件格式。manifest 固化 format/schema version、实体文件和 Plugin 制品的 path/size/SHA-256。归档和托管制品路径从已打开的受控根目录句柄逐段 no-follow 解析，拒绝 symlink、hardlink、junction/reparse point、device/FIFO/socket、特殊条目和检查后替换；校验、哈希和实际读取绑定同一仍打开、link count 为 1 的普通文件句柄。Plugin 新导入使用同目录 no-replace 发布不可变唯一对象，更新发布新对象后再事务切换数据库引用，绝不原地覆盖或删除身份不匹配的并发对象。恢复可以处理已通过分块认证的内容，但敏感值只在有界内存短暂出现并立即以目标设备密钥写入隔离 staging 数据库；必须验证认证流结尾和全部预检、按当前 normalization ID 从实体重建搜索索引后才能请求用户最终确认。后段认证失败时保持 current 不变；unarmed/no-owner live instance 通过 outcome=`aborted_pre_switch` retirement 整目录移入 tombstone 后清理，任何 ownership/identity 歧义都保留并 fail-closed。

导出只接受尚不存在的最终目标，在同目录 staging 完成 flush/fsync 后以 no-replace rename 发布并 fsync 父目录。恢复预验证只展示影响且 manifest 保持 unarmed；用户确认后冻结写入，在同一周期完成 current/staging checkpoint、关闭、sidecar 分流和从封闭 current 生成的安全备份。随后 arm manifest、发布 owner prepared/applying并执行同文件系统切换；新 current 必须在写闸门关闭下通过全部 health/search/artifact/identity 验证后才 owner committed。prepared/applying 恢复并验证 old，committed 只维持已验证 new；两者分别以 retirement outcome old/new 接管整个 live instance。启动先重放 retirement/tombstone，再处理 live manifest/owner/cleanup。所有 registry/live/tombstone/manifest/owner/retirement/cleanup control state 不进入 archive、安全备份或新树。

预验证崩溃产生的 manifest-only、无数据库或 partial payload 只有在 unarmed/no-owner 且 cleanup 六槽收敛时，才可创建 aborted retirement journal并整目录移入 tombstone；live 名称下不逐项删除。owner/retirement 歧义、armed owner 缺失、symlink/hardlink/special、普通文件 link count≠1、目录/文件 identity 竞态或任何 fsync 无法证明时，恢复保持关闭并 fail-closed。

**Rationale**: 现有明文 JSON 备份缺 DAG、LLM、Agent 关系、会话和 WASM，也无法跨设备重加密。`age` 避免自创 Argon2id/XChaCha 分块协议，支持流式大文件与口令加密；tar 足够表达受控的文件集合。新增依赖必须通过维护状态、许可证和 CVE 审查。

**Alternatives considered**:

- 扩展当前明文 JSON：拒绝；无法安全包含敏感值和 WASM。
- 自定义 Argon2id + XChaCha20Poly1305 分块格式：拒绝；自造密码协议风险高。
- ZIP 自带密码：拒绝；常见 ZipCrypto 不满足现代认证加密要求。

## 12. 结构化日志与诊断

**Decision**: 共享 runtime 统一 tracing 字段：execution_id、operation、实体 identifier、outcome、stable error_kind、duration_ms。默认只记录长度、计数、SHA 或 allowlist 摘要；不记录完整 prompt、模型响应、Tool/Capability 输入输出。

HiveGUI 沿用非阻塞 JSONL writer，但通过可直接测试、可注入 UTC Clock 的边界写入固定 v1 schema；中央脱敏后的 `cause_summary` 为 `string|null` 且最多 512 UTF-8 bytes，原始 cause 不落盘，同一错误只在处理边界记录一次。活动段固定使用 `.open` 后缀并只发布以换行结束的完整单行 JSON。轮转先 flush/fsync 活动段，再原子 rename 为不可变 `.jsonl` 并 fsync 父目录；启动只允许丢弃活动段末尾的一条不完整记录。持久化 retention high-watermark 防止时钟回拨复活记录；活动段在最早记录到期前强制轮转，混合新旧完成段通过流式 staging、fsync、原子 replace 和父目录 fsync 逐记录压缩，保证每条记录自 `occurred_at` 起实际不超过 7×24 小时且全部可见文件实际总计不超过 100,000,000 bytes。诊断导出只打包同样脱敏的 execution_id 相关完整记录和非敏感环境信息。

**Rationale**: HiveGUI 已有按日 JSON 日志，但没有容量/天数清理和 execution_id span。字段黑名单不足以保护未知 Tool 数据，默认不记录 payload 更安全。

**Alternatives considered**:

- 建 runtime audit 数据库表：拒绝；tracing 已满足诊断需求并降低敏感数据复制。
- 记录全部参数再 mask：拒绝；未知字段仍可能泄漏。

## 13. GPUI 异步、键盘与可访问性

**Decision**: 长任务全部在 Tokio runtime；GPUI 主线程只消费有界/合并后的事件。顶层保持真实的 Home/Ai/Tools 三路由，AI 页复用现有 12 Tab。新增 conversation view 和 runtime-event bridge，统一 action button、modal focus trap、焦点恢复与 DAG keyboard actions。

自动测试使用 VisualTestContext 的 keystroke、focus、debug_bounds 和事件注入；主题色做对比度单测。AccessKit 完整树若测试平台不可见，则增加测试 hook 并保留各平台辅助技术 smoke test。

**Rationale**: 现有管理页已可复用，但多数点击行为仍是 mouse handler div。GPUI 已支持 AccessKit，VisualTestContext 已被现有滚动回归证明适合验证真实几何与键盘行为。

**Alternatives considered**:

- 新增第二套 desktop UI 框架：拒绝；违反宪法。
- 只检查源码存在 aria 字符串：拒绝；不能证明焦点、角色和实际动作。

## 14. 测试与性能证据

**Decision**: 遵循严格 TDD：每批先写/评审红灯 contract/integration/visual test，再写实现。Foundation 的补充门禁固定为 T016A-T016F→T017F，分别覆盖日志持久性、Capability/Persisted Tool 公开边界、SQLite/cleanup journal、Plugin v4 schema、跨故事原生滚动 inventory/harness 与敏感字段×真实落盘介质 inventory/harness；搜索 normalization/FTS/SQL source inventory 另由 T017G→T017H 审批。未来故事行只能由对应 Tests/Reviewer 激活并观察 Red，T138/T139/T142 只能汇总复跑。新增 mock LLM、本地 WASM fixture、v2/v3 schema fixture、format 1/2/3 备份 fixture和可注入故障点。

性能基准固定输入，采集样本并报告 p50/p95/p99：Agent action 调度、Tool 分派、100 节点 no-op DAG。每项保存版本化基线、fixture 版本和硬件/OS/toolchain/build-profile 环境指纹；已有路径在实现前采集基线，新入口以首个获批 Green 建立基线，任一跟踪百分位回归 >10% 时无明确签字、记录理由、影响范围和到期复核日期即阻断。外部 LLM/网络/用户代码单列。UI heartbeat 测试记录输入到可见反馈和主线程最大间隔。

**Rationale**: 当前可运行的 `desktop_host_call`、`runtime_capability_catalog`、`wasm_exports_test` 和 DAG 单元测试均通过；但迁移测试仍断言 schema=1 而实现已是 2，且 rollback 测试没有故障注入。HiveWeb benches 绑定服务端依赖，不能作为 HiveGUI 本地预算证据。

**Alternatives considered**:

- 只运行 `HIVEGUI_HEADLESS=1`：拒绝；该路径在 Store 初始化前退出，不能证明本地 runtime。
- 只用 Criterion 默认报告：不足；规格要求显式 p50/p95/p99，需要固定样本统计。

## 15. CI 安全与 SQLite 查询证据

**Decision**: 在现有 CI 的 fmt/clippy/test 之外增加固定版本的 secret scanner、`cargo-deny` 和 `cargo-sqlx`，并用 source-contract test 防止步骤被移除或改成非阻断；CI 精确执行 `SQLX_OFFLINE=true cargo sqlx prepare --workspace --check`，metadata 缺失/陈旧或安装/命令失败均阻断。每个含过滤或关联条件的 SQLite/MySQL 生产查询都必须由 EXPLAIN fixture 解析计划并断言所有过滤/关联列使用预期索引，FTS5 `VIRTUAL TABLE INDEX` 视为索引访问，非小型表未经批准的扫描使测试失败；小型固定表/metadata 例外记录大小、理由、审批者、到期日和复核结论。Agent 资源、Workflow 整图和 Category 树通过查询计数 hook 验证无 N+1。

普通管理列表保持大小写不敏感的纯文本任意位置包含语义：字段和输入使用 `hivegui-nfkc-casefold-v1`，其严格实现 [Unicode Standard 17.0.0 Default Case Algorithms R5](https://www.unicode.org/versions/Unicode17.0.0/core-spec/chapter-3/)——把每个 Unicode 标量按官方 [`DerivedNormalizationProps-17.0.0.txt`](https://www.unicode.org/Public/17.0.0/ucd/DerivedNormalizationProps.txt) 的 `NFKC_CF` 映射（缺失项为 identity）后拼接，再把结果规范化为 NFC。`NFKC_CF` 会同时处理 compatibility、case 和 default-ignorable，不能以简单 lowercase、simple fold 或 `casefold(NFKC(input))` 近似替换。实现使用精确固定的 `unicode-normalization =0.1.25` 提供 Unicode 17.0.0 NFC，并从上述官方数据确定性生成紧凑映射表；`PROVENANCE.md` 必须记录源 URL、下载文件 SHA-256、Unicode 许可/使用条款和生成命令，contract test 同时断言 crate `UNICODE_VERSION=(17,0,0)`、生成表校验值和全量/边界 golden fixture。数据库持久化 normalization ID；依赖、源文件校验值、Unicode 行为或 fixture 变化必须分配新 ID 并显式迁移重建。该新增直接依赖与生成数据仍须在 T017G Red 后由 T017H reviewer 审批，不能把本次文档方案批准冒充依赖 Green。

原始非空输入规范化为空时返回 `empty_after_normalization`。规范化后 3+ Unicode 标量值走 FTS5 trigram 参数化完整 phrase，1–2 值走事务同步 short-gram 索引；`%`、`_`、引号和 FTS 操作符仅按字面量匹配。SQLite 缺少 trigram tokenizer、normalization ID 不匹配或派生索引验证失败时启动/迁移 fail-closed，禁止回退业务表 `LIKE`/`SCAN`。固定应用 SQL 使用 checked macros，有限变体使用封闭 enum/`match`，生产 `QueryBuilder` 为零；HiveGUI 外部 MySQL 值使用 `mysql_async` prepared API，动态数据库/表/列名必须精确匹配服务器 metadata allowlist 后由单一 `MysqlIdentifier` 序列化。`game_service.category_name` 的既有恶意字符串回归验证 checked static SQL bind；完整 named-query 只有 reviewed-config 中央边界能在 fail-closed 验证后构造唯一 `AssertSqlSafe`。

**Rationale**: 一次性人工 secret scan 不能满足宪章的 CI MUST；声明索引或只保存 EXPLAIN 输出也不能证明运行时查询计划实际使用预期索引。source-contract、计划断言、offline metadata 检查和查询计数提供可重复的合并门禁。

**Alternatives considered**:

- 只在发布前手工扫描：拒绝；无法阻止后续提交引入凭据。
- 只检查 CREATE INDEX 源码：拒绝；SQLite planner 可能不使用该索引。
- 只审查 SQL 是否看似批量：拒绝；不能捕获 adapter 层循环查询。

## 16. 启动恢复状态分离

**Decision**: 数据库不存在、迁移失败和既有源库健康检查失败使用三条互斥启动路径。不存在时创建 v4 并在发布前通过完整性与外键双检查；迁移目标提交前双检查或 FTS5 trigram 可用性失败只允许 rollback 后重试/退出；既有源库 `integrity_check` 失败或 `foreign_key_check` 返回孤儿记录时，允许从备份恢复、先隔离损坏文件后重建或退出，重建需要二次确认且不触碰托管 Plugin。

**Rationale**: “重试迁移”不能修复物理损坏，而把迁移失败当作损坏重建会绕过安全快照和版本语义。分离状态让每条路径都可故障注入并避免误删数据。

**Alternatives considered**:

- 所有启动错误统一显示重试/退出：拒绝；损坏数据库没有可行恢复动作。
- 所有失败都允许重建：拒绝；会把可回滚迁移错误变成数据丢失。

## 17. 稳定枚举、Builtin、会话入口与 Category 树

**Decision**: Function/Tool kind 在 SQLite、Rust serde 和备份中统一使用稳定字符串：Function 为 `builtin|custom|placeholder`，Tool 为 `function-wrap|workflow-wrap`，旧 Function 整数 `1/2/3` 与 Tool 整数 `1/2` 只在迁移中映射。四个 Builtin 的唯一 identifier 为 `format_template|json_parse|json_stringify|text_regex_match`，共享 `hive-builtins` registry 不提供点号 alias；真实旧点号记录仅由迁移事务重命名，目标碰撞时整体回滚。公开 `start_session` 不接受 Agent 覆盖；Category 是普通分页的唯一例外，整树一次加载且搜索保留祖先。

**Rationale**: 字符串消除跨层魔法数字漂移；Placeholder 的 schema-only 状态使其意图和不可执行边界可测试；单一 Builtin 名称避免两个产品的 lookup/execute 再次漂移；唯一公开入口保证所有新会话满足默认根不变量；Category 的层级语义无法用独立平面分页稳定表达。

**Alternatives considered**:

- 同时接受整数和字符串：拒绝；长期扩大兼容面并使备份不稳定。
- 公开任意 Agent 会话入口：拒绝；直接违反唯一默认入口验收。
- 对 Category 树按平面20条分页：拒绝；会截断祖先/子树并破坏导航语义。

## 18. 已识别的实施风险

- HiveGUI 与 HiveWeb 当前均开启 WASI。
- HiveGUI 缺 memory/output limit；外层 timeout 不会停止 spawn_blocking。
- HiveWeb 现有 memory limit 把字节当作 WASM pages，抽取时必须修正。
- HiveGUI catalog 宣称支持尚无本地 handler 的服务端 Capability。
- 共享 `hive-builtins` 当前仍使用点号 identifier；实施必须改为四个下划线名称且不得保留运行时别名，旧持久化记录仅由受信迁移处理。
- 当前 AgentRunner、Tool、Workflow、LLM 没有统一 cancellation token。
- 当前迁移、备份与 Plugin 导入存在部分成功/混合状态风险。
- 当前日志缺贯穿全链的 execution_id，现有管理控件键盘语义不完整。
- 当前 SQLite 普通列表搜索没有 FTS5 trigram/short-gram 的统一索引路径，SQL 调用面仍包含运行时 `QueryBuilder`；补充 Red/审批闭合前不得实施对应 Green。
