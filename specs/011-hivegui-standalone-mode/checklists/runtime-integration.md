# Runtime Integration Checklist: HiveGUI 独立运行模式

**Purpose**: 验证共享运行时库、产品独立 adapter、WASM 集成与工作流适配相关需求的完整性、清晰性和一致性
**Created**: 2026-06-15
**Feature**: [spec.md](../spec.md)

**Note**: 此检查清单关注**需求质量**——运行时集成相关需求是否完整、清晰、无歧义。不测试实现行为。

---

## Dependency Reuse Requirements

- [x] CHK001 共享运行时模块（ABI、capability、execution、plugin、wasm、workflow、persisted_tool）作为库依赖复用时，是否定义了最小公共 API 契约和禁止依赖边界？[Gap, Spec §FR-028/FR-039, Plan §Technical Context]
  > **评估**: Plan §Constitution Check/§Project Structure、Research §1/§2 与 T002/T015 明确把这些存储无关协议和纯算法放入 `hive-runtime-core`，并要求其依赖图不含 HTTP、SQLx 或任何产品 crate。T144 负责最终公开 API inventory。HiveGUI 和 HiveWeb 各自保留数据、文件、网络及宿主 adapter，不通过 `hiveweb` crate 复用运行时。

- [x] CHK002 Spec 声明共享运行时和 Plugin ABI——“共享”是否明确定义了复用方式（源码级依赖 / 二进制链接 / 独立 crate 提取）？[Clarity, Spec §FR-028/FR-039]
  > **评估**: Plan §Summary/§Project Structure 与 Research §1/§2 将方式固定为 workspace 源码级 library 复用：运行时复用边界仅为 `hive-runtime-core`、`agent`、`providers`、`hive-builtins` 和共享 ABI fixtures。HiveGUI 不依赖 `hiveweb`，两个产品也不在运行时通信。

- [x] CHK003 research.md 拒绝通过 `hiveweb` feature 排除 MySQL/Redis/S3 后供 HiveGUI 依赖——此结论是否已落实到 spec、plan 和可验证任务？[Consistency, Research §1, Spec §FR-028/FR-029]
  > **评估**: FR-028/FR-029、Plan §Summary/§Project Structure、T015/T026 和 T116/T140 一致要求 HiveGUI 不构造 HiveWeb client、不读取 HiveWeb URL、零 HiveWeb 请求且失败时零 HiveWeb fallback；不存在为 HiveGUI 增加 `hiveweb` `standalone` feature 的实施任务。

- [x] CHK004 HiveWeb runtime 绑定 MySQL/Redis/S3/Axum，而 HiveGUI 使用 SQLite 和本地托管文件——是否定义了存储与宿主适配策略？[Gap, Research §1/§5, Plan §Project Structure]
  > **评估**: `hive-runtime-core` 只提供存储无关的 `WorkflowGraph`、ABI、取消和 Plugin 生命周期；HiveGUI 通过 T026、T070、T079、T096 的本地 adapter 接入 SQLite、受控文件句柄和本地 Capability，HiveWeb 通过自己的 adapter 接入云端设施。T080 只让两个 adapter 接入共享 ABI/host_call 类型和 fixture，不复用彼此的连接或 executor。

- [x] CHK005 共享 Builtin 的边界和 identifier 是否完整定义？[Clarity, Spec §FR-038, Research §17]
  > **评估**: 两端编译期复用 `hive-builtins` 的四个下划线 identifier：`format_template/json_parse/json_stringify/text_regex_match`；T083-T089 验证注册、执行和只读语义。点号名称不作为记录或运行时别名，消息回复、结束对话和子 Agent 路由也不注册为 Tool。

## WASM Runtime Integration

- [x] CHK006 FR-039 声明同一 Plugin “无需转换”即可在两个产品运行——是否定义了兼容性的验证标准（相同 WASM/manifest、相同 ABI import/export、等价 envelope）？[Clarity, Spec §FR-039, Contract `plugin-abi.md` §7]
  > **评估**: 兼容性由 `hive-runtime-core` 的版本化 ABI 类型、manifest/host_call 校验器和同一 WASM fixture 定义，而非由复用 HiveWeb invoker/pool 隐式推导。T072、T080、T082 要求 HiveWeb host adapter 与 HiveGUI host adapter 对 success、denied、unknown、timeout、memory、output 和 WASI-denied 产生等价 envelope。

- [x] CHK007 HiveWeb 的 WASM host adapter 可能依赖云端设施——HiveGUI 是否提供独立 host adapter，并只暴露真实本地 Capability？[Gap, Spec §FR-039, Contract `plugin-abi.md` §3/§5]
  > **评估**: T070 要求 HiveGUI `desktop_host` 只注册真实本地 handler；T080 让 HiveGUI/HiveWeb 各自 adapter 接入共享 `host_call` 类型与 fixture。缺失 Capability 在导入/执行前稳定拒绝，HiveGUI 不加载或调用 HiveWeb 的 `wasm_imports` 实现。

- [x] CHK008 Plugin 执行的超时和错误处理是否定义默认值、用户覆盖范围、硬上限和稳定错误？[Clarity, Spec §FR-045, Contract `plugin-abi.md` §5]
  > **评估**: 默认值固定为 30s/128MiB/10MiB，用户可按 Plugin 调整且不得超过 120s/512MiB/50MiB；T074/T079 覆盖 timeout、fuel、memory pages、output、取消和错误类别。配置来自 HiveGUI 本地 Plugin 记录，不继承 HiveWeb `PoolConfig`。

- [x] CHK009 Plugin 文件存储路径是否在需求中定义（`s3_key` 兼容字段相对于什么？用户选择的源文件是否继续作为运行时依赖）？[Clarity, Data Model §Plugin, Spec §FR-017/FR-031]
  > **评估**: `s3_key` 被定义为 HiveGUI 私有托管 Plugin 目录内的安全相对制品键，不是 S3 地址；T073/T077/T079 要求 staging、原子复制、受控文件句柄、路径/大小/SHA-256 校验，并证明删除原始文件后仍可执行。

- [x] CHK010 多 Plugin 之间的 WASM 实例隔离是否在需求中定义（一个 Plugin 的 trap/超时/超限是否影响其他执行或宿主应用）？[Gap, Spec §FR-045/FR-047, Contract `plugin-abi.md` §4-§5]
  > **评估**: T074/T079 定义默认无 WASI、完整 policy cache key、有界 LRU 空闲池和失效规则；取消、timeout、trap、memory/output 超限或 host error 后实例必须淘汰，只有健康实例可回池。T118 进一步验证强停一个 Plugin 不影响其他会话或主 UI。

## Workflow Executor Adaptation

- [x] CHK011 Workflow 要求按依赖顺序执行、同一拓扑层并行——并行语义和验收边界是否明确？[Ambiguity, Research §5, Contract `local-runtime.md` §5]
  > **评估**: T011/T020 将语义固定为存储无关 `WorkflowGraph` 的拓扑分层；T091 验证并行层、fail-fast、零重试和取消，T093 以固定 100 节点 no-op DAG 验证本地调度预算。物理线程数不是跨产品契约，产品 adapter 只能在不改变依赖、取消和 fail-fast 语义的前提下调度。

- [x] CHK012 HiveWeb 的现有 Workflow 实现绑定产品存储时，HiveGUI 从 SQLite 加载的适配需求是否在 spec/plan 中定义？[Gap, Plan §Project Structure, Research §5]
  > **评估**: T020 把纯 DAG 校验、拓扑层调度、fail-fast 汇总和取消放入共享 `WorkflowGraph`；T095 由 HiveGUI 一次事务保存/一次批量加载 SQLite 图，T096 通过 HiveGUI 自己的 `workflow_executor` 和 `NodeExecutor` adapter 执行 Function/Plugin/LLM。HiveGUI 不直接复用 HiveWeb `WorkflowExecutor`。

- [x] CHK013 工作流节点类型（start_node、end_node、function_node、generate_answer_node）及节点/边 JSON 映射是否通过共享图契约保持一致？[Consistency, Spec §FR-033, Data Model §WorkflowNode/WorkflowEdge]
  > **评估**: T011/T013/T090/T091 验证四个稳定 `*_node` 值、`node_config`/`mapping` JSON、图结构与执行语义；共享层只接收规范化 `WorkflowGraph`，产品 adapter 负责把各自存储映射成该类型。兼容性不得通过继承 HiveWeb 存储 DTO 或 `InputSpec` 实现。

- [x] CHK014 generate_answer_node（LLM 生成）——是否明确由哪个产品边界解析 LLM，而不会隐式调用 HiveWeb？[Gap, Spec §US10/§FR-029, Contract `llm-provider.md`]
  > **评估**: T096 必须等待 T052 Green，并通过 HiveGUI 的 `provider_resolver`/`NodeExecutor` adapter 执行 generate_answer；本地配置转换为 `providers::ProviderBuildConfig`。共享 `WorkflowGraph` 不持有 HiveWeb provider 或 client，失败也不得回退到 HiveWeb。

- [x] CHK015 Workflow 执行失败时的回滚/部分结果需求是否定义？（如 3 节点 Workflow 在第 2 个节点失败，第 1 个节点的副作用如何处理）[Gap, Spec §FR-037/SC-020]
  > **评估**: fail-fast 后不再启动新节点，已运行节点可完成并记录；Workflow 层重试次数为 0，不回滚已完成外部副作用。T091/T098 要求结果区分失败、已完成和未执行节点并提示副作用，不以 HiveWeb 现有行为作为规范来源。

## Builtin Function Loading

- [x] CHK016 四个共享 Builtin 的注册机制是否与 HiveGUI SQLite Function 表兼容，且无点号 alias？[Gap, Spec §FR-038, Research §17]
  > **评估**: T083/T084/T087 要求启动时从共享 `hive-builtins` 下划线 registry 幂等同步四条 `kind='builtin'` 记录；点号 identifier 的注册、lookup 与 execute 都必须失败，真实旧记录仅由 T012/T022 的迁移事务改名。

- [x] CHK017 HiveGUI Builtin 是否全部为无需外部服务的本地纯函数？[Gap, Spec §FR-038]
  > **评估**: 四个共享函数均为本地纯函数；游戏、消息回复和子 Agent 路由不进入 Builtin/Tool CRUD，不需要 HiveWeb 降级路径。

- [x] CHK018 Builtin 版本一致性——HiveWeb/HiveGUI 更新后是否通过共享 crate 同步？[Gap, Spec §FR-038]
  > **评估**: 两端编译期依赖同一个 workspace `hive-builtins` registry，因此下划线 identifier、schema 和 executor 在同一提交中版本锁定；HiveGUI 不在启动或执行时向 HiveWeb 同步。

## LLM Provider Routing

- [x] CHK019 FR-029 要求直接使用本地配置的 LLM——vendor 请求格式与 tool-call 解析的所有权是否明确？[Clarity, Spec §FR-029, Contract `llm-provider.md` §2]
  > **评估**: T052 把 HiveGUI 本地配置映射为 workspace `providers::ProviderBuildConfig`，由 `providers` 构造 `LLMProvider` 并拥有 vendor 格式与 tool-call 解析；HiveGUI 不维护 HiveWeb client，也不复制第二套 vendor HTTP/JSON 协议层。

- [x] CHK020 research.md 拒绝 HiveGUI 手写 OpenAI-only client——该决定是否与本地 Provider 需求和共享边界一致？[Consistency, Research §4, Contract `llm-provider.md`]
  > **评估**: T048/T052 以本地 Provider/Preset/Model 数据驱动 `providers` crate；`agent` 消费统一 provider/tool-call 抽象。不存在 HiveGUI→HiveWeb 的协议转换路径，也不以 OpenResponses 或 HiveWeb 请求格式作为本功能的运行时边界。

- [x] CHK021 多个 LLM Provider 时的故障转移/回退需求是否定义（Provider A 不可用时是否切换到下一 Model）？[Gap, Spec §FR-013, Contract `llm-provider.md` §3]
  > **评估**: Preset 中 Model 按 priority 形成 fallback 链；429、5xx、网络/TLS 和节点超时可切换，参数/认证错误、Capability 拒绝和用户取消不得切换。T048/T052 验证排序、事件、分段计时与取消；任何链路都不得回退到 HiveWeb。

- [x] CHK022 LLM 调用的失败切换策略是否在需求中定义（哪些错误可切换、哪些错误必须立即终止）？[Gap, Contract `llm-provider.md` §3-§5]
  > **评估**: 产品契约定义的是有序 Model fallback，而不是假定存在“指数退避、最多 3 次”的 HiveWeb 重试。T048/T052 精确覆盖允许/禁止 fallback 的错误类别、每次 `fallback_used` 事件、外层 execution 预算和取消不 fallback。

## Database Abstraction & Query Layer

- [x] CHK023 HiveWeb 使用 MySQL 特有的 SQL，而 HiveGUI 使用 SQLite——是否明确禁止复用或机械改写 HiveWeb 查询，并为 HiveGUI 定义独立查询边界？[Gap, Plan §Technical Context/§Project Structure]
  > **评估**: `hive-runtime-core` 不含 SQL；T022/T024/T028 负责 HiveGUI schema v4、公开 Store 校验、SQLx checked SQLite 查询和查询计划，T038 的外部 MySQL DataSource 也使用独立 HiveGUI adapter。HiveWeb 的 MySQL 查询与连接始终留在 HiveWeb 产品边界。

- [x] CHK024 data-model.md 中稳定枚举是否完整覆盖 Function 与 WorkflowNode？[Completeness, Data Model §Function/WorkflowNode]
  > **评估**: Function.kind 固定为 `builtin|custom|placeholder`；WorkflowNode.node_type 固定为 `start_node|end_node|function_node|generate_answer_node`；Tool/Skill/执行状态也各有稳定集合，未知值由公开边界和 schema 拒绝。

- [x] CHK025 HiveGUI Store/运行时 adapter 的公开错误模式是否与 spec 和稳定 runtime envelope 一致？[Consistency, Contract `local-runtime.md` §7, Spec §字段验证规则/§UI 交互规范]
  > **评估**: T013/T024 定义统一公开字段目录以及 `invalid_input`、安全 `conflict` 和事务零修改；T027 定义 `function_not_executable` 等稳定错误映射与 exactly-once 记录。`contracts/README.md` 是边界索引，不再声称复用 HiveWeb Store CRUD 或错误类型。

## Error Handling & Recovery

- [x] CHK026 HiveGUI WASM 实例池达到容量时的行为是否定义（淘汰、排队或立即失败）？[Gap, Contract `plugin-abi.md` §5]
  > **评估**: T074/T079 将其定义为“健康空闲实例缓存”而非 HiveWeb 并发池：全局最多 8 个、每个完整 cache key 最多 1 个，满时按 LRU 淘汰；执行中的实例不占空闲缓存槽。请求的 timeout/取消由当前 Plugin 本地策略控制，不继承 HiveWeb `PoolConfig`。

- [x] CHK027 Workflow 执行超过节点或 Workflow 时间预算时的处理需求是否定义（终止、回滚、部分结果）？[Gap, Spec §FR-037/FR-047, Data Model §Workflow]
  > **评估**: Workflow `timeout_ms` 是公开受校验字段；T091/T096 验证超时触发 fail-fast、停止调度新节点、取消传播、零自动重试和不回滚已完成外部副作用，并保留完成/失败/未执行结果。

- [x] CHK028 LLM Provider 连接失败（网络不可达、TLS、限流、认证、超时）的分类、fallback 与用户可定位信息是否定义？[Gap, Contract `llm-provider.md` §3-§5, Contract `local-runtime.md` §7]
  > **评估**: T048/T052 按错误类别决定是否 fallback，并发出稳定流事件和 `llm_ms`；所有节点失败映射为本地 `llm_unavailable`，参数/认证/取消不得 fallback。T049/T053 负责本地配置 UI 的遮蔽凭据、字段错误和焦点，不显示 secret，也不请求 HiveWeb 获取诊断。

## Traceability & Verification

- [x] CHK029 “Plugin ABI 兼容”（FR-039/SC-022）的可验证性——如何在不让 HiveGUI 请求 HiveWeb 的情况下验证？是否定义共享兼容性测试套件？[Measurability, Spec §FR-039/SC-022]
  > **评估**: T072/T080/T082 使用同一 WASM/manifest fixture 分别直接测试 HiveWeb host adapter 和 HiveGUI host adapter，并比较等价 envelope；这是编译/测试期 fixture 复用，不是 HiveGUI 运行时调用 HiveWeb。T116/T140 另以网络捕获证明零 HiveWeb 请求和零失败回退。

- [x] CHK030 Workflow 跨产品共享的是哪些不变量，如何避免把 HiveWeb 产品行为误当成 HiveGUI 规范？[Gap, Research §5, Spec §FR-033/FR-037]
  > **评估**: T011/T020 以 `hive-runtime-core` 契约测试固定图校验、拓扑分层、fail-fast、确定性错误和取消；T091/T096/T100 再验证 HiveGUI SQLite/Function/Plugin/LLM adapter。HiveGUI 不复用 HiveWeb `WorkflowExecutor`，产品特有存储与副作用也不要求相同。

---

## Notes

- CHK001–CHK030 全部通过评估
- 重点关注共享纯 runtime 与两个产品 adapter 的依赖方向；“共享”只表示编译期 Rust library/契约/fixture 复用。
- HiveGUI 不依赖、不请求、不回退 HiveWeb；HiveWeb 未配置、不可达或未运行均不影响 HiveGUI 启动、管理和本地 Agent 执行。

## 已废弃历史架构（禁止实现）

- 禁止为 HiveGUI 在 `hiveweb` 增加或启用 `standalone` feature（包括 `default-features=false, features=["standalone"]`）。
- 禁止让 HiveGUI 直接依赖或复用 HiveWeb 的 invoker、pool、`WorkflowExecutor`、orchestrator、Store、client 或云端连接。
- 禁止以“同一 HiveWeb 代码路径”或“继承 HiveWeb `PoolConfig`”代替共享 ABI fixture 与两个独立 host adapter 的兼容性验证。
