# Performance & Observability Checklist: HiveGUI 独立运行模式

**Purpose**: 验证性能指标、异步处理、资源管理和可观测性相关需求的完整性、清晰性和一致性
**Created**: 2026-06-15
**Feature**: [spec.md](../spec.md)

**Note**: 此检查清单关注**需求质量**——性能与可观测性需求是否完整、清晰、可测量。不测试实现行为。

---

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
