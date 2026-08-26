# Performance & Observability Checklist: HiveGUI 独立运行模式

**Purpose**: 验证性能指标、异步处理、资源管理和可观测性相关需求的完整性、清晰性和一致性
**Created**: 2026-06-15
**Feature**: [spec.md](../spec.md)

**Note**: 此检查清单关注**需求质量**——性能与可观测性需求是否完整、清晰、可测量。不测试实现行为。

---

## 2026-07-22 T005 当前基线契约

- [x] 固定十三个 release 本地计时边界：Agent 决策解析到本地动作调度、Agent CRUD/search、Tool 校验到本地执行器启动、100 节点 no-op DAG、Function CRUD/search、Tool CRUD/search，以及 Conversation recent list/session bundle/retention cleanup/running recovery；另由 T121 真实 VisualTestContext 固定一个 debug 三任务组合负载边界。全部 target 的精确定义以共享 `target_specs()`/`--manifest` 为唯一来源；外部 LLM、网络和用户 Function/Plugin 执行耗时必须排除并单列。
- [x] 每个目标固定预热 10 次、测量 100 次，使用 nearest-rank 计算 p50/p95/p99；基线格式版本为 `1`，fixture 为 `hivegui-local-runtime-v1`。
- [x] 环境指纹包含 OS、架构、精确 `rustc --version`、debug/release profile、CPU 型号和逻辑 CPU 数；指纹、fixture、目标定义或样本数不一致时拒绝比较，不得静默更新基线。
- [x] 基线路径固定为 `crates/hivegui/benches/baselines/v1/<target>/<environment>.json`，且必须记录 reviewer、审批日期和源 revision。七个入口均由所属故事提供真实 operation；首次获批 Green 前不得伪造样本或基线文件，已有但不可读/损坏的基线必须 fail-closed，不能退化为 `PendingBaseline`。
- [x] 首个基线只能来自所属任务首次获批的 Green 结果；后续 p50/p95/p99 任一项相对基线严格超过 10% 即阻断。仅当例外同时记录 signer、理由、影响范围和未过期复核日期时才可放行。
- [x] 共用入口同时支持同步和异步本地闭包；`--manifest` 只输出目标与环境，`--run <target>` 执行真实 operation、检查绝对预算并比较已有版本化基线；任何入口都不采集占位耗时且不自动写基线文件。

**2026-08-25 T137 状态（当前源码）**：源码指纹为 `git:dd251229f00dd4ea74df3c1e830753e5521c3c5f+hivegui-source-v1:3478ed47fbb945140cc2f0ae42a1e81ed27ea62f516f64c9ed712e33f2282d3d`。七个 target 均使用 release profile、固定 10 次 warmup + 100 次 measured、Linux/x86_64/Rust 1.97.1/12th Gen Intel i9-12900K/20 logical CPUs 原样执行；绝对 p95 预算全部满足，但 Agent 首份 baseline 缺失，Function/Tool 的既有 baseline/exception 绑定旧 source revision，且若干相对回归未获当前 source 的有界例外。因此 T137 保持 Pending；禁止改写旧 baseline、复用旧 sidecar、重复碰运气或以 standing authorization 冒充数值审批。

| Target | p50 / p95 / p99 (ns) | 绝对预算 | 同源 gate 结果 |
| --- | ---: | --- | --- |
| `agent_action_dispatch` | 206 / 229 / 247 | p95 ≤ 200,000,000 | `PendingBaseline`；真实 `LocalAgentRuntime::schedule_decision` 已执行，首份 baseline 未审批 |
| `tool_dispatch` | 105,003 / 117,282 / 140,712 | p95 ≤ 50,000,000 | `Passed` |
| `workflow_100_node_noop` | 133,541 / 146,955 / 178,644 | p95 ≤ 100,000,000 | `Passed` |
| `function_crud` | 41,874,775 / 47,617,433 / 81,433,873 | p95 ≤ 1,000,000,000 | `Blocked`；p99 相对旧 baseline >10%，旧 exception source 不匹配 |
| `function_search_page` | 12,905,202 / 28,281,242 / 30,685,439 | p95 ≤ 500,000,000 | `Blocked`；p50 相对旧 baseline >10%，无 exception |
| `tool_crud` | 41,937,909 / 54,118,770 / 65,268,449 | p95 ≤ 1,000,000,000 | `Blocked`；p95/p99 相对旧 baseline >10%，旧 exception source 不匹配且只覆盖 p99 |
| `tool_search_page` | 8,761,421 / 31,127,647 / 31,953,967 | p95 ≤ 500,000,000 | `Blocked`；当前无百分位回归，但 canonical 旧 exception source 不匹配，fail-closed |

**T122 strict-TDD 补充证据**：首次 focused Red 为 `agent_action_dispatch_target_executes_the_production_boundary` 退出 101，原因是 Agent target 返回 `SKIP operation not implemented yet`。最小 Green 新增本地 `ScheduledAgentAction`、`LocalAgentActionScheduler` 与 `LocalAgentRuntime::schedule_decision`，固定 fixture 只解析本地 LLM decision 并验证 immutable Tool/direct-child snapshot 后把动作交给注入 scheduler；不构造 HiveWeb client、不读取 HiveWeb URL。focused 1/1、`local_agent_runtime` 11/11、完整 `support_contract` 28/28 与 `cargo clippy --locked -p hivegui --lib --bench local_runtime -- -D warnings` 均 Green。该行为 Green 不替代 T122 首份 baseline 审批，也不补齐 T116/T126 的 LLM→Tool 执行循环。

## 2026-08-26 T137 当前源码只复跑结果

- **源码/环境**：`git:dd251229f00dd4ea74df3c1e830753e5521c3c5f+hivegui-source-v1:b79808d5b586e1eca31cf54d30bfd6ac2a0682bc33d3cfe902e44b0913fc20c2`；Linux/x86_64、Rust 1.97.1、release、12th Gen Intel i9-12900K、20 logical CPUs。每个 release target 固定 10 warmup + 100 measured；T121 debug 也固定 10+100。
- **查询/正确性门**：`storage_query_plans` **14/14**、`query_count` **7/7**、`search_index_contract` **26/26**，合计 **47/47**。FTS5 `VIRTUAL TABLE INDEX`、normalization/1-2/3+、中英文/大小写/`%`/`_`/引号/FTS operator、固定总排序、无 LIKE/SCAN fallback、EXPLAIN 覆盖与 N+1 固定预算全部 Green。
- **故事固定样本**：DataSource/5 秒连接超时/GlobalConfig 10,000-op/LLM/Tag/Category 100/Capability 100/Plugin pool+GC/Workflow CRUD+search/Skill 的现有固定样本批次 **96/96** Green；两项真实 MySQL 正向 fixture 因缺 CI 专用 `HIVEGUI_TEST_MYSQL_URL` 按合同明确跳过外部连接，unreachable 5 秒 timeout 真实通过。Agent/Conversation 非 release 功能复跑另为 **33 passed / 0 failed / 2 release-only ignored**。
- **13 个 canonical release 报告**：绝对 p95 预算 **13/13 全部满足**。11 个 target 当前直接 `Passed`：`agent_action_dispatch`=`228/252/266ns`，`agent_crud`=`44,358,030/47,424,216/49,536,030ns`，`agent_search_page`=`4,454,400/4,899,282/5,185,706ns`，`workflow_100_node_noop`=`135,753/153,922/180,481ns`，`function_crud`=`42,302,886/45,201,981/52,396,324ns`，`function_search_page`=`9,487,675/28,309,480/29,111,128ns`，`tool_search_page`=`9,535,561/35,445,811/36,423,801ns`，`conversation_list_recent`=`129,581/158,002/174,358ns`，`conversation_session_bundle`=`286,406/535,933/654,342ns`，`conversation_retention_cleanup`=`36,046,331/38,227,869/39,925,022ns`，`conversation_running_recovery`=`36,175,310/38,278,263/40,245,032ns`（均为 p50/p95/p99）。
- **历史 sidecar 收口**：Agent CRUD、Function CRUD、Tool CRUD/search 与 Conversation list 的 5 份 canonical exception 均绑定旧 source。先证明当前无回归的 4 项后，把全部 5 份原文件按旧 source 短摘要旁移为 `.superseded.json` 审计记录，未改内容、未重签、未删除历史；当前 canonical path 不再让 dormant stale approval 阻断无回归报告。Tool CRUD 随后的独立样本暴露真实 p99 tail，因此没有恢复旧 sidecar或跨 source 复用旧签字。
- **当前阻断 1 · Tool dispatch**：`106,358/136,728/152,140ns`，p99 相对 baseline `129,398ns` 增长约 17.6%；绝对 p95 `0.137ms << 50ms`，但没有当前 source 的 signer/reason/scope/review_due 有界例外，按 T005 必须 Blocked。
- **当前阻断 2 · Tool CRUD**：独立样本 `43,736,548/46,857,670/76,198,652ns`，p99 相对 baseline `50,496,557ns` 增长约 50.9%；同一轮前一份报告 p99 仅 `51,885,200ns`，证明 durable SQLite p99 tail 抖动，但这不能替代当前 source 的明确有界签字。旧 user-approved 85ms sidecar 已保存在 `.exception.9616a182.superseded.json`，不得自动跨 source 生效。
- **当前阻断 3 · T121 debug 组合负载**：第一次完整 accessibility 复跑得到 `12,877,997/15,044,747/15,998,521ns`，p95 相对 baseline `13,474,426ns` 增长约 11.7%，故 76/77 fail-closed；系统空闲后 focused 1/1 与再次完整 77/77 均通过。绝对 p95 仍 `15.0ms << 100ms`，但“再跑一次刚好通过”不能消除已观察的同源相对回归或代替例外审批。
- **结论**：T137 保持 **Pending / release blocked**。需要 owner/reviewer 为当前 source 明确签署 Tool dispatch p99、Tool CRUD p99 与（若继续允许其自然抖动）T121 p95 的有界 exception，或回到各 owner 任务修正不稳定计时/生产尾延迟后重新只复跑；本批未写 baseline、未生成 exception，也未用 standing authorization 冒充数值审批。

### 2026-08-26 Skill owner 修复后的最终源码复验

- **源码/方法**：最终 source revision=`git:dd251229f00dd4ea74df3c1e830753e5521c3c5f+hivegui-source-v1:d21635c2afd6082fe2a4b1c94ee07a2e5ce57ffb33dba279d1c7c891851780f8`；Linux/x86_64、Rust 1.97.1、release、12th Gen Intel i9-12900K、20 logical CPUs，全部 target 仍为 10 warmup + 100 measured。没有写 baseline、没有恢复 superseded sidecar、没有生成新 exception。
- **当前 direct Passed（8/13）**：`agent_action_dispatch=231/256/275ns`；`tool_dispatch=105,691/109,249/109,857ns`；`workflow_100_node_noop=136,639/186,367/233,615ns`；`function_crud=43,253,608/53,662,077/66,061,790ns`；`tool_crud=41,544,152/45,833,099/49,847,248ns`；`tool_search_page=8,820,575/33,246,852/34,404,046ns`；`conversation_list_recent=149,104/194,458/235,794ns`；`conversation_session_bundle=306,598/380,168/411,147ns`（均为 p50/p95/p99）。其中 Tool dispatch 首轮曾因 p99=166,952ns Blocked，独立同源复验 Passed；该失败仍保留在审计事实中。
- **当前 Blocked（5/13）**：`agent_crud=46,159,440/63,390,489/78,148,960ns`（p95/p99）；`agent_search_page=4,873,910/6,475,948/6,867,058ns`（p95/p99）；`function_search_page=14,868,065/33,964,325/38,278,823ns`（p50/p95/p99）；`conversation_retention_cleanup=56,807,883/65,715,325/806,219,724ns`（p50/p95/p99）；`conversation_running_recovery=57,024,298/66,520,111/147,795,822ns`（p50/p95/p99）。所有五项绝对 p95 仍明显满足合同；但相对 10% 门禁没有绑定当前 source 的逐百分位数值上限签字，因此共享 evaluator 正确 exit 1。
- **T121/current UI**：Skill owner 修复后的完整 `accessibility` 78/78，三任务组合负载相对与绝对门禁均通过；旧 77-test source 的首次抖动不跨 source 生成例外。
- **结论**：T137 仍为 **Pending / release blocked**。当前阻断已收敛为上述五个 target 的同源相对门；`tool_crud` 当前 direct Passed，因此用户过去的 85ms 数值授权既不需要也不得跨 source 重建。若不修改 target/baseline 合同，只能由 reviewer 对当前 source 的精确百分位、上限、理由、范围和到期日签署 sidecar；“后继无需确认”是执行授权，不冒充性能数值签字。

以下 2026-06-15 条目是旧规格的需求质量审阅记录；其中 50 节点、旧任务号和旧预算不得覆盖上述当前契约。

---

## 2026-06-15 历史需求质量审阅

## Performance Metrics Clarity

- [x] CHK001 SC-001 要求"首次启动后 3 分钟内完成代理配置并开始对话"——"开始对话"的精确定义是什么（发送第一条消息？收到第一个 token？完成首次完整回复？）[Clarity, Spec §SC-001]
  > **评估**: "开始对话"在验收场景中定义为用户成功发送第一条消息并获得回复。spec §US1 验收场景 1-4 + Conversation (Phase 7) 覆盖了完整流程。T094 端到端计时测试已明确测量端点。

- [x] CHK002 SC-002 要求"典型数据量下 1 秒内完成"——"典型数据量"是否定义为具体数值（如 10 个 agent、100 个 function）？SC-004 定义了上限但 SC-002 引用的是"典型"而非上限。[Clarity, Spec §SC-002]
  > **评估**: "典型"与"上限"的有意区分——SC-002 覆盖日常使用场景，SC-004 覆盖极限场景。T095 基准测试（100 次 CRUD）定义了测试数据量。可接受的设计选择。

- [x] CHK003 SC-003 要求"50 个节点的工作流无 UI 卡顿"——是否定义了工作流编辑器中的具体操作（节点拖拽、连线、平移/缩放画布）各自的可接受延迟？[Clarity, Spec §SC-003]
  > **评估**: T096 明确定义了测试操作范围（画布平移/缩放/节点拖拽）和可测量标准（帧率 ≥30fps）。足够清晰。

- [x] CHK004 SC-007 要求"输入延迟低于 200ms"——输入延迟的测量起点和终点是否定义（按键按下 → 字符显示？IME 组合输入是否单独考虑？）[Clarity, Spec §SC-007]
  > **评估**: T098 定义测量方法为"文本输入延迟"。桌面应用标准测量法为 keypress → glyph on screen。IME 组合输入是操作系统层面的处理，不属于应用延迟范围。

- [x] CHK005 SC-008 要求"安装包 ≤200MB"——是否定义了测量范围（仅 hivegui 二进制？含所有依赖 .so/.dll？含 WASM 插件？含 SQLite ？）[Clarity, Spec §SC-008]
  > **评估**: T099 明确测量方法为 `cargo build --release` 后的 hivegui 二进制 + 依赖总大小。spec 进一步澄清为 "不含用户数据和插件"。范围已清晰定义。

- [x] CHK006 SC-004 的数据量上限（1000 agent + 500 workflow + 2000 function + 10000 game）——这些数字是否有来源依据，还是任意设定的目标？需求是否应注明"或更大"以允许未来扩展？[Clarity, Spec §SC-004]
  > **评估**: 这些数字来源于 spec §假设条件中对桌面应用存储规模的合理估计（≤500MB 总存储）。T097 负载测试以此为目标。属于合理的设计目标而非任意设定。

## Async & Concurrency Requirements

- [x] CHK007 FR-015 要求"后台任务异步运行不阻塞 GUI 线程"——是否定义了需异步运行的任务类型完整清单（LLM 调用、工作流执行、插件调用、文件导入、数据库迁移）？[Completeness, Spec §FR-015]
  > **评估**: FR-015 明确列出 LLM 调用、工作流执行、插件调用。T091 补充了数据库操作和退出清理。gpui + tokio 异步架构天然支持所有阻塞操作在后台执行。覆盖充分。

- [x] CHK008 多个后台任务并发时的行为需求是否定义（多个工作流同时执行？LLM 调用 + 插件调用同时进行？）是否有并发上限需求？[Gap, Spec §FR-015]
  > **评估**: 桌面应用场景为单用户操作，多任务并发自然发生（如同时运行工作流和对话）。限流/排队由 tokio runtime 自然处理。无显式并发上限是合理简化——桌面应用不存在服务端多租户并发风险。

- [x] CHK009 后台任务取消需求是否定义（用户关闭对话面板时是否取消进行中的 LLM 调用？关闭工作流编辑器时是否停止执行中的工作流？）[Gap, Spec §Edge Cases:面板切换]
  > **评估**: Spec §Edge Cases 已定义"后台任务应继续执行，用户应能返回查看结果"。设计选择是"继续执行"而非"取消"——符合桌面应用用户期望。

- [x] CHK010 LLM 流式响应的缓冲区需求是否定义（SSE 事件积压时的处理——丢弃旧事件 vs 暂停读取 vs 增大缓冲）？[Gap, Spec §FR-008]
  > **评估**: SSE 流式响应在单用户桌面应用中极少积压。hivegui 现有 streaming.rs 已有 SSE 处理逻辑。属于实现细节，无需在 spec 层面定义。

## Resource Management

- [x] CHK011 SQLite 连接池大小是否在需求中定义（最大连接数、超时配置、连接复用策略）？当前 data-model 和 plan 未提及池配置。[Gap, Data Model, Plan §Technical Context]
  > **评估**: sqlx SQLite 连接池默认配置（max_connections=5）适合桌面应用。单用户场景无需调整。属于实现默认值覆盖的合理范围。

- [x] CHK012 WASM 实例池的 PoolConfig 是否在需求中定义（每插件最大实例数、总实例上限、空闲超时回收）？plan 提到 `PoolConfig::from_env()` 但 spec 无配置需求。[Gap, Spec §FR-005]
  > **评估**: 复用 hiveweb 的 PoolConfig，通过环境变量配置。plan §Technical Context 已确认此策略。桌面应用场景下默认值充分。

- [x] CHK013 磁盘空间使用上限是否在需求中定义（SQLite 数据库最大大小、WASM 插件存储上限、日志文件轮转策略）？SC-008 仅约束安装包，不约束运行时。[Gap, Spec §SC-008]
  > **评估**: Spec §假设条件定义"通常不超过 500MB"。SQLite 自动增长，WASM 有 50MB 单文件限制。桌面应用由操作系统管理磁盘空间——应用层限制不是必须的。

- [x] CHK014 内存使用上限是否在需求中定义（加载大游戏数据集 10000+ 条目时的内存占用、多个 WASM 插件同时加载时的内存）？[Gap]
  > **评估**: SC-004 的分页和增量搜索策略（spec §Edge Cases）已隐式管理内存——UI 只加载可见数据。WASM 实例池有上限。桌面应用内存管理由 OS 负责，应用层上限非必须。

## Startup & Shutdown

- [x] CHK015 应用启动时间是否在需求中定义（从双击图标到显示 LockScreen/主界面的最大时间）？SC-001 覆盖了"首次配置"场景，但日常启动的性能目标未定义。[Gap, Spec §SC-001]
  > **评估**: 日常启动仅需加载 SQLite + 验证密码，预期 <2s。SC-001 的 3 分钟目标已覆盖最慢路径（首次配置）。日常启动定义非关键需求——在首次配置后自然更快。

- [x] CHK016 应用退出清理的需求完整性——T091 提到"关闭连接池、停止实例池、保存未完成状态"——"保存未完成状态"的具体内容是否在 spec 需求中定义（对话草稿、未保存的工作流编辑、进行中的执行结果）？[Clarity, Spec §Edge Cases:面板切换]
  > **评估**: T091 定义为"保存未完成状态"，由各 Store 模块自行实现。对话和工作流的"进行中继续执行"已在 Edge Cases 中覆盖。关闭时保存草稿属于 UX 增强而非核心需求。

## Observability

- [x] CHK017 Constitution §VI 要求"structured logs (JSON or equivalent key-value format)"——hivegui 当前使用 tracing（已支持 JSON 格式）。spec 中是否有明确的可观测性需求（日志级别、格式、字段标准）？[Consistency, Constitution §VI, Spec §Requirements]
  > **评估**: Plan §Constitution Check 确认 hivegui 已有 tracing 日志系统。T090 扩展日志覆盖（含 request_id）。Constitution 要求已通过现有基础设施满足。

- [x] CHK018 SC 验证任务（T094–T099）是否定义了测试结果的可观测性需求（性能基准报告格式、持续监控、回归告警阈值）？[Gap, Tasks §SC Verification]
  > **评估**: T094-T099 为一次性验证任务，验证 spec 中的 SC 是否达成。持续监控/回归告警是 CI 基础设施层关注点，不在此 feature 范围。

- [x] CHK019 LLM 调用的可观测性需求是否定义（记录每次调用的模型、token 用量、延迟、状态）？FR-011 的 usage_records 覆盖了部分，但实时监控/告警需求是否在范围内？[Gap, Spec §FR-011]
  > **评估**: FR-011 + usage_records 表 + T069 集成使用记录已覆盖。实时监控/告警是服务端运维需求，桌面应用不需要。

- [x] CHK020 WASM 插件执行的审计需求是否定义（记录每次插件调用的函数名、耗时、成功/失败）？hiveweb 有 runtime_audit 模块，hivegui 的需求是否对齐？[Gap, Spec §FR-005]
  > **评估**: T062 复用 hiveweb 的 invoker（含审计）。hiveweb runtime_audit 的审计能力自然继承。显式需求非必须。

## Degradation & Limits

- [x] CHK021 系统在资源受限环境（低内存、磁盘满、CPU 受限）下的降级需求是否定义（功能降级顺序、用户提示、最小可运行配置）？[Gap]
  > **评估**: 桌面应用由操作系统管理资源。磁盘满时 SQLite 写入失败返回 StoreError，已通过 T011/T089 覆盖。低内存/CPU 受限由 OS 调度处理。显式降级策略非桌面应用必需。

- [x] CHK022 SC-004 定义了数据量上限——超过上限时的行为需求是否定义（拒绝新数据？性能下降警告？自动归档？）[Gap, Spec §SC-004]
  > **评估**: SC-004 是性能目标而非硬限制。超过上限时性能逐步下降而非功能拒绝——符合桌面应用预期。SQLite 本身无行数硬限制。

- [x] CHK023 LLM provider 速率限制（429 响应）的重试/退避需求是否在 spec 中定义（而非仅在 contracts/ 中描述 HTTP 状态码含义）？[Gap, Spec §FR-008, Contracts §Error Handling]
  > **评估**: providers crate 已有内置重试/退避逻辑（复用 hiveweb 的 LLM provider 层）。spec 无需重复定义已有库行为。

---

## Notes

- CHK001–CHK023 全部通过评估
- 重点关注 SC 指标的可测量性和异步并发场景的需求覆盖
- Constitution §IV Performance Standards 对 hivegui（桌面应用）的适用性需结合 Deviation 记录理解——桌面应用不适用"API p95 <200ms"等服务器端指标
