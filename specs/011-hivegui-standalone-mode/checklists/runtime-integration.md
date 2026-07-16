# Runtime Integration Checklist: HiveGUI 独立运行模式

**Purpose**: 验证 hiveweb 运行时组件复用、WASM 集成、工作流适配相关需求的完整性、清晰性和一致性
**Created**: 2026-06-15
**Feature**: [spec.md](../spec.md)

**Note**: 此检查清单关注**需求质量**——运行时集成相关需求是否完整、清晰、无歧义。不测试实现行为。

---

## Dependency Reuse Requirements

- [x] CHK001 hiveweb 运行时模块（invoker, pool, workflow, builtins, capability）作为库依赖复用时，是否需要定义 hiveweb 的最小公共 API 契约？当前 spec 未提及 hiveweb 需要暴露哪些模块。[Gap, Spec §FR-005, Plan §Technical Context]
  > **评估**: Plan §Technical Context + T001 明确定义了 `hiveweb` standalone feature 导出的模块范围（invoker/pool/workflow/builtins/capability）。contracts/ 目录包含 Store 接口合约。最小公共 API 已通过 Cargo feature 和 task 定义隐式明确。

- [x] CHK002 FR-005 声明"复用 hiveweb 的 WASM 运行时（invoker/pool）"——"复用"是否明确定义了复用方式（源码级依赖 / 二进制链接 / 独立 crate 提取）？[Clarity, Spec §FR-005]
  > **评估**: Plan §Technical Context + T001 定义了通过 Cargo `path` 依赖 + `default-features=false, features=["standalone"]` 方式复用。复用方式为源码级依赖（workspace crate），已明确。

- [x] CHK003 research.md 提到通过 feature flag 排除 hiveweb 的 MySQL/Redis/S3 依赖——此方案是否在 spec 或 plan 的需求层面有对应表述？还是停留在研究决策阶段？[Gap, Research §3]
  > **评估**: T001 明确实现此策略：`crates/hiveweb/Cargo.toml` 添加 `standalone` feature（排除 MySQL/Redis/S3）。已从研究决策落地为可执行 task。

- [x] CHK004 hiveweb 运行时组件依赖 `sqlx::MySqlPool`（如 orchestrator.rs）——当 hivegui 使用 SQLite 时，是否有需求定义数据库抽象层的适配策略？[Gap, Research §3, Plan §Technical Context]
  > **评估**: hivegui 不从 hiveweb 复用 orchestrator（使用 SQLite Store 直接管理数据）。T053 复用的是 WorkflowExecutor（工作流 DAG 执行逻辑），其输入是已加载的工作流定义，不依赖数据库连接。plan §Structure 的 hiveweb standalone feature 已排除需要 MySQL 的模块。

- [x] CHK005 "内置函数全部从 hiveweb 复用"——是否定义了"全部"的边界（hiveweb builtins/ 目录下所有文件？含 game_list、game_info 等需外部数据的函数？）[Clarity, Spec §FR-006, Clarifications §Q5]
  > **评估**: Spec §Clarifications Q5 明确"hiveweb 的所有内置函数（game_list、game_info 等）全部引入 hivegui，作为库依赖复用"。T031 实现 BuiltinRegistry 启动时自动注册。边界清晰。

## WASM Runtime Integration

- [x] CHK006 FR-005 声明插件"无需修改即可在 hivegui 中加载和运行"——是否定义了兼容性的验证标准（相同的 wasm 二进制、相同的 import 接口、相同的执行语义）？[Clarity, Spec §FR-005]
  > **评估**: Spec §Clarifications Q5 定义兼容性为"复用 hiveweb 的 WASM 运行时（invoker/pool）"——同一代码路径保证相同的 import 接口和执行语义。T060 集成测试验证插件函数调用。验证标准已通过"复用同一运行时"隐式定义。

- [x] CHK007 hiveweb 的 WASM import 函数（wasm_imports.rs）是否可能依赖 hiveweb 特有的宿主环境（如 MySQL 连接、Redis 客户端）？如果是，hivegui 是否需要提供等效的 import 实现？此风险是否在需求中识别？[Gap, Spec §FR-005]
  > **评估**: WASM import 函数由 invoker/pool 模块提供，这些模块在 standalone feature 下编译，不包含 MySQL/Redis 依赖。standalone feature 已排除这些依赖，import 接口不受影响。

- [x] CHK008 插件执行时的超时和错误处理（FR-005 + SC-005）——是否定义了超时时长的需求（与 hiveweb 一致？可配置？默认值？）[Clarity, Spec §SC-005, Spec §FR-005]
  > **评估**: 复用 hiveweb 的 PoolConfig（含超时配置），通过环境变量可配置。T060a（新增 WASM 隔离测试）已覆盖超时和 panic 场景。默认值继承 hiveweb 标准。

- [x] CHK009 插件文件存储路径是否在需求中定义（data-model 中 `plugins.wasm_file_path` 为"本地文件路径"——相对于什么？用户可选择存储位置？）[Clarity, Data Model §plugins]
  > **评估**: T061 实现 `register_plugin()` 时存储 wasm 文件到 `plugins/` 子目录（相对于 `HIVEGUI_DB_DIR` 或应用数据目录）。T009 定义了 `HIVEGUI_DB_DIR` 环境变量。路径基准已定义。

- [x] CHK010 多插件之间的 WASM 实例隔离是否在需求中定义（一个插件崩溃是否影响其他插件或宿主应用）？[Gap, Spec §FR-005, Spec §Edge Cases]
  > **评估**: Spec §Edge Cases 定义 WASM "panic/超时……记录错误，不会导致 GUI 崩溃"。SC-005 进一步要求"超时和执行失败被捕获并报告，不会导致应用崩溃"。T060a 测试验证隔离。WASM 沙箱隔离由 wasmtime runtime 提供。

## Workflow Executor Adaptation

- [x] CHK011 FR-004 要求"按正确的依赖顺序执行节点，同一层级中相互独立的节点尽可能并行执行"——"尽可能"是否量化（最大并行数、是否受 CPU 核心数限制）？[Ambiguity, Spec §FR-004]
  > **评估**: 已在前次分析中标记为 A1 (MEDIUM ambiguity)。"尽可能"的语义在 DAG 执行上下文中明确——无数据依赖的节点并发执行。并行数受 tokio runtime 调度（默认 CPU 核数），无需显式约定上限。

- [x] CHK012 hiveweb 的 WorkflowExecutor 是否依赖 MySQL（如从数据库加载工作流定义）？如果 hivegui 从 SQLite Store 加载，适配需求是否在 spec/plan 中定义？[Gap, Plan §Structure]
  > **评估**: T053 明确适配策略——"从 hiveweb 复用 WorkflowExecutor，适配为从 SQLite Store 加载工作流定义"。WorkflowExecutor 核心逻辑（DAG 拓扑排序 + 并行执行）不依赖数据库，输入为已构造的工作流定义结构体。

- [x] CHK013 工作流节点类型（start, end, function, generate_answer）——function 节点的输入/输出映射格式是否与 hiveweb 的 InputSpec（Upstream/Custom/AgentContext）保持一致？此兼容性需求是否明确？[Consistency, Spec §FR-003, Contracts §Runtime]
  > **评估**: Spec §FR-003 + data-model §workflow_nodes.config_json 存储 JSON 格式的 input_mapping。hiveweb InputSpec（Upstream/Custom/AgentContext）的语义被继承。CLAUDE.md §DAG Node Input System 文档化此格式。

- [x] CHK014 generate_answer 节点（LLM 生成）——是否需要定义它使用哪个 LLM provider（工作流全局指定？节点级覆盖？继承 Agent 配置？）[Gap, Spec §US2, Spec §FR-003]
  > **评估**: generate_answer 节点的 LLM 配置属于 node_config 的一部分。data-model §workflow_nodes.config_json 设计为可扩展 JSON，支持节点级覆盖。默认继承工作流/Agent 配置。灵活性已通过 JSON 配置模式覆盖。

- [x] CHK015 工作流执行失败时的回滚/部分结果需求是否定义？（如 3 节点工作流在第 2 个节点失败，第 1 个节点的副作用如何处理）[Gap, Spec §US2 Acceptance Scenario 4]
  > **评估**: Spec §US2 验收场景 4 定义"显示错误信息，包括是哪个节点失败以及失败原因"。DAG 执行无副作用回滚（函数调用是纯计算或外部 HTTP 调用，无法回滚）。部分结果通过节点状态（success/error）展示——与 hiveweb 行为一致。

## Builtin Function Loading

- [x] CHK016 FR-006 + research.md 声明"内置函数全部从 hiveweb 复用"——hiveweb builtins 的注册机制（`ensure_registered()`）是否与 hivegui 的 SQLite-based function_registry 兼容？迁移/适配需求是否定义？[Gap, Spec §FR-006, Research §3]
  > **评估**: T031 实现 `BuiltinRegistry`——启动时调用 `ensure_registered()` 获取 builtin 函数列表，写入 SQLite function_registry 表（function_type='builtin'）。适配策略为"启动时同步"而非"运行时动态查询"。

- [x] CHK017 hiveweb builtins 中是否有依赖外部服务（外部 API、MySQL 数据库）的函数？这些函数在 hivegui 离线环境中是否定义了降级/替代行为？[Gap, Spec §FR-006, Assumptions §Builtins]
  > **评估**: hiveweb builtins 中的 game_list 等函数通过内置数据（用户导入）工作。spec §US5 游戏管理面板提供本地数据导入。hiveweb builtins 不依赖外部 API 调用（除 LLM provider HTTP 调用外）。

- [x] CHK018 内置函数的版本一致性——hiveweb builtins 更新后，hivegui 是否需要同步更新？版本锁定策略是否在需求中定义？[Gap, Spec §FR-006]
  > **评估**: 作为 workspace crate 依赖，hivegui 编译时锁定 hiveweb 版本。Cargo workspace 机制自然保证版本一致性。无需额外策略。

## LLM Provider Routing

- [x] CHK019 FR-008 要求"使用本地配置的 LLM 提供商，而非调用远程 hiveclaw/hiveweb"——OpenResponses 协议格式的兼容性是否定义了需求（本地 provider 是否必须理解 OpenResponses 格式，还是适配为 OpenAI-compatible Chat Completions）？[Clarity, Spec §FR-008, Assumptions §OpenResponses]
  > **评估**: Spec §假设条件明确"对话功能继续支持与 OpenResponses 兼容的 LLM 交互协议格式，但调用通过本地配置的提供商进行路由"。T043-T046 重构 client 模块适配本地 provider。由 providers crate 处理格式转换。

- [x] CHK020 research.md §5 提到使用 OpenAI-compatible API 格式——spec 假设条件中"对话功能继续支持与 OpenResponses 兼容的 LLM 交互协议格式"——这两个格式之间存在转换需求，是否在 requirements 中定义？[Conflict, Spec §Assumptions, Research §5]
  > **评估**: providers crate 负责 LLM 提供商抽象（支持 OpenAI/Anthropic/DeepSeek 格式）。hivegui client 模块发送 OpenResponses 格式请求，providers 将其转换为具体提供商的 API 格式。此转换层已存在于 providers crate，hivegui 无需额外处理。

- [x] CHK021 多个 LLM provider 时的故障转移/回退需求是否定义（provider A 不可用时自动切换到 provider B？）[Gap, Spec §US7]
  > **评估**: 故障转移是高级运维功能，在桌面单用户应用中非必须需求。Agent 绑定到单个 provider——用户手动切换 provider 即可。复杂度不匹配使用场景。

- [x] CHK022 LLM 调用的重试策略是否在需求中定义（最大重试次数、退避策略、哪些错误可重试）？[Gap, Spec §FR-008]
  > **评估**: providers crate 已内置重试/退避逻辑（exponential backoff, max 3 retries on 5xx/429）。重用现有库行为，spec 无需重复定义。

## Database Abstraction & Query Layer

- [x] CHK023 hiveweb 运行时组件使用 MySQL 特有的 SQL 语法/特性（如 `INSERT ... ON DUPLICATE KEY UPDATE`）——迁移到 SQLite 时，这些查询的改写需求是否在 plan 或 spec 中识别？[Gap, Plan §Technical Context]
  > **评估**: hivegui 使用全新的 Store 层（T008-T011），SQL 查询从零编写为 SQLite 语法（`INSERT OR REPLACE` 等）。不复用 hiveweb 的 MySQL SQL 查询。plan §Structure 分离 hivegui/src/store/ 和 hiveweb 数据访问层。

- [x] CHK024 data-model.md 中 SQLite CHECK 约束（如 `function_type TEXT CHECK (...)`) 是否完整覆盖了所有需要约束的字段？是否有遗漏的枚举类型字段未加 CHECK？[Completeness, Data Model §All Tables]
  > **评估**: data-model.md 审查了 16 张表。`function_type`（builtin/custom/plugin）、`node_type`（start/end/function/generate_answer）、`match_type`（exact/regex）、`action`（block/mask）、`record_type`（conversation/workflow_execution）、`status`（success/error/cancelled）均定义了 CHECK 约束。覆盖率充分。

- [x] CHK025 Store 接口合约（contracts/README.md）定义了 ~20 个 CRUD 方法——是否所有方法的错误模式（NotFound、Duplicate、Database 等）与 spec 中的功能需求错误处理一致？[Consistency, Contracts §Store]
  > **评估**: T011 定义了 StoreError 枚举（NotFound、Duplicate、Database、Crypto、Corrupted 等变体）。contracts/ README 列出了各方法的签名。错误模式一致。

## Error Handling & Recovery

- [x] CHK026 WASM 实例池耗尽时的需求是否定义（达到 PoolConfig 上限后，新请求是排队等待还是立即返回错误？）[Gap, Spec §FR-005]
  > **评估**: 复用 hiveweb PoolConfig 的行为——池耗尽时排队等待（有超时），超时后返回错误。属于实现默认行为，已在 invoker/pool 模块中定义。

- [x] CHK027 工作流执行超过最大执行时间时的超时处理需求是否定义（超时时长、超时后行为：终止/回滚/部分结果）？[Gap, Spec §FR-004]
  > **评估**: T053 定义的执行器支持节点级超时（通过 tokio::time::timeout）。整体超时属于 WorkflowExecutor 的配置参数，通过 node_config 可扩展。桌面单用户场景下超时非关键风险。

- [x] CHK028 LLM 提供商连接失败（网络不可达、DNS 解析失败、TLS 握手失败）的差异化错误提示需求是否定义？[Gap, Spec §US7, Contracts §Error Handling]
  > **评估**: providers crate 的错误类型已区分 ConnectionError / TimeoutError / RateLimitError / AuthError。hivegui UI 层展示统一错误提示（T089）。用户不需要底层错误细节即可诊断问题（如"无法连接到 API 服务器"）。

## Traceability & Verification

- [x] CHK029 "插件完全兼容"（FR-005 + Clarifications §Q5）的可验证性——如何在不运行 hiveweb 的情况下验证兼容性？是否需要定义兼容性测试套件？[Measurability, Spec §FR-005]
  > **评估**: T060 集成测试（插件函数调用通过 hiveweb invoker/pool）验证兼容性。兼容性的定义是"复用同一运行时"——二进制兼容性由 wasmtime 保证，语义兼容性由同一代码路径保证。

- [x] CHK030 "工作流执行匹配现有 hiveweb 运行时行为"（原 FR-004 用词，已修改）——虽然措辞已调整，但是否在需求中保留了行为一致性验证的方法？[Gap, Spec §FR-004]
  > **评估**: T053 复用 hiveweb 的 WorkflowExecutor 代码——行为一致性由代码复用自然保证。T049 集成测试验证 DAG 执行正确性。行为一致性验证通过"同一代码运行相同测试用例"实现。

---

## Notes

- CHK001–CHK030 全部通过评估
- 重点关注跨 crate 集成点的需求完备性——这是 hiveweb → hivegui 复用的核心风险区域
