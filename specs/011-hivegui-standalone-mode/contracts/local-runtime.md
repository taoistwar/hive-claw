# Local Agent Runtime Contract

## 1. 边界

`LocalAgentRuntime` 运行在 HiveGUI 进程内，读取本地 SQLite 与托管 Plugin 目录，直接调用用户配置的 LLM。它不得构造 HiveWeb client、读取 HiveWeb URL 配置、请求 HiveWeb，或在本地失败后回退到 HiveWeb。

## 2. 命令

```text
start_session(user_message) -> session_id, execution_id
continue_session(session_id, user_message) -> execution_id
stop_execution(execution_id) -> accepted | already_terminal | not_found
delete_session(session_id) -> deleted_count
clear_history(retention_filter) -> deleted_count
```

- `start_session` 始终解析唯一默认根 Agent；公开命令和正式 UI 不接受入口 Agent 覆盖。测试若需从特定快照启动，只能使用不导出的 test-only helper。
- 每次用户轮次生成新的 execution_id，并更新 ChatSession.execution_id。
- 命令返回表示已接受，不等待外部 LLM、Tool 或 Plugin 完成。
- 所有命令参数和运行时配置必须在公开命令边界校验，不得依赖 UI：session_id/execution_id 必须是 UUID，user_message 去除首尾空白后长度为1..=1MiB，retention_filter 必须使用受支持单位且数值大于0。普通失败返回 `invalid_input { field, reason }`，不得携带被拒绝值；唯一性冲突返回 `conflict { field, value }` 且 value 仅允许安全字段值；引用或状态冲突返回不含 value 的 `conflict { field, reason, references }`，references 仅包含安全的实体类型、ID、identifier 或 name。机密值只返回字段名和脱敏原因；失败不得创建会话、执行或部分写入状态。

所有公开分页/搜索入口的 `invalid_input.reason` 词汇固定为：`page=0` 返回 `out_of_range`，`page_size!=20` 返回 `fixed_value_required`，`search` 长度大于 255 返回 `too_long`，`search` 含 NUL 或其他控制字符返回 `control_character`，原始非空 `search` 经 `hivegui-nfkc-casefold-v1` 规范化后为空返回 `empty_after_normalization`。

## 3. 事件

每个事件必须携带 `execution_id`, `session_id`, `sequence`, `occurred_at`：

| kind | 必需字段 | 语义 |
| --- | --- | --- |
| status | `phase` | queued/running/stopping/terminal |
| token | `text`, `agent_id` | LLM 增量文本 |
| routed | `from_agent_id`, `to_agent_id`, `reason` | 仅直接子 Agent |
| tool_started | `tool_call_id`, `tool_identifier` | Tool 已通过 schema/Capability 校验 |
| tool_finished | `tool_call_id`, `outcome`, `summary` | 不含完整敏感参数或结果 |
| workflow_node | `workflow_id`, `node_key`, `status` | pending/running/completed/failed/cancelled/skipped |
| fallback_used | `from`, `to`, `reason` | LLM fallback 切换 |
| completed | `final_agent_id`, `message_id`, `elapsed_ms` | 成功终点 |
| failed | `error.kind`, `error.message`, `elapsed_ms` | 失败终点 |
| cancelled | `completed_steps`, `interrupted_steps`, `unstarted_steps`, `side_effect_notice` | 用户停止终点 |

对一个 execution_id，`completed|failed|cancelled` 恰好出现一个且只出现一次。

## 4. 资源解析

1. 当前 Agent 的 Tool = 显式 AgentTool ∪ `is_always=true`，按 Tool id 去重。
2. 当前 Agent 的 Skill = 显式 AgentSkill ∪ `is_always=true`，按 Skill id 去重并拼入 system prompt。
3. Capability 只读取当前 Agent 的 AgentCapability，不继承父 Agent。
4. Function、Workflow、Plugin 调用前，资源声明的 required_capabilities 必须是当前集合的子集。
5. `route_to_subagent` 是运行时控制命令，不是 Tool 表记录；目标必须是当前 Agent 的直接子 Agent，路径无循环且 depth≤10。
6. Placeholder Function 是 schema-only 记录；所有公开 Function、Tool 与 Workflow 执行入口必须在 Capability 检查、Plugin 查找或 guest 实例化前返回 `function_not_executable`。
7. Plugin 资源解析必须返回由托管根目录逐段 no-follow 打开、完成身份/大小/SHA-256 校验且仍保持打开的只读制品句柄；校验与 guest 实例化读取同一对象。运行时不得根据 `s3_key`、绝对路径或已规范化字符串重新打开制品，也不得在句柄关闭后执行，以免并发 rename、symlink、junction/reparse point 或 hardlink 替换形成 TOCTOU 窗口。

## 5. Workflow 语义

- 稳定 node_type 仅为 `start_node|end_node|function_node|generate_answer_node`；保存和执行前验证唯一 `start_node`、至少一个可达 `end_node`、端点存在、节点键唯一及无环，当前写入和运行时不接受短名称别名。
- 仅在所有前驱完成后调度节点，同层独立节点可并行。
- 任一节点失败或超时后不再调度新节点；已运行节点允许安全结束，但输出不再触发下游。
- Workflow 层自动重试次数为 0，且不回滚已完成外部副作用。

## 6. 取消

停止请求按顺序执行：

1. 原子标记 execution 为 stopping，并停止新的 Agent 路由、LLM、Tool 和 Workflow 节点调度。
2. 触发共享取消令牌；丢弃停止后到达的 LLM token 和调度结果。
3. 取消可取消的网络 future；等待正在运行的普通 Tool/Workflow 到达取消检查点。
4. Plugin 获得最多 2 秒宽限；仍未退出则调用 Extism `CancelHandle`，丢弃该实例。
5. 加密持久化最终状态 `cancelled`，保留已完成/中断/未开始步骤和外部副作用提示。

取消一个 execution 不得退出进程、关闭全局 Tokio runtime 或中断其他会话。

## 7. 稳定错误类别

`invalid_input`, `not_found`, `conflict`, `key_missing`, `key_corrupt`, `key_permissions`, `llm_unavailable`, `capability_denied`, `function_not_executable`, `route_rejected`, `workflow_invalid`, `workflow_failed`, `plugin_incompatible`, `plugin_missing`, `plugin_tampered`, `plugin_timeout`, `plugin_memory_limit`, `plugin_output_limit`, `plugin_cancelled`, `storage_busy`, `storage_corrupt`, `storage_incompatible`, `storage_recovery_blocked`, `backup_publish_uncertain`, `cancelled`, `internal`。

字段级 envelope 固定为三种互斥形态：普通输入错误是 `invalid_input { field, reason }`；唯一性冲突是 `conflict { field, value }`；引用或状态冲突是 `conflict { field, reason, references }`。后两者不得同时携带 value 与 references，references 只允许安全实体标识，任何形态都不得回显机密值或底层 SQL 错误。

SQLite 文件封闭或启动恢复无法安全继续时必须精确返回 `storage_recovery_blocked { reason, artifact }`。合法配对及跨迁移、快照、备份恢复和启动重放统一使用的总优先级如下；实现必须先完成全部候选异常分类，再按此表选择第一项，不得依赖目录枚举、连接池或 OS 返回顺序。同一 reason 有多个候选制品时，artifact 次序固定为 `wal > rollback_journal > shm`。

| 优先级 | reason | 合法 artifact |
| ---: | --- | --- |
| 1 | `checkpoint_failed` | `checkpoint` |
| 2 | `checkpoint_busy` | `checkpoint` |
| 3 | `connections_open` | `connection` |
| 4 | `sidecar_reappeared` | `wal\|rollback_journal\|shm` |
| 5 | `sidecar_hot` | `wal\|rollback_journal` |
| 6 | `sidecar_recoverable` | `wal\|rollback_journal` |
| 7 | `sidecar_unknown_owner` | `wal\|rollback_journal\|shm` |
| 8 | `sidecar_cleanup_failed` | `wal\|rollback_journal\|shm` |

`reason` 和 `artifact` 不得出现表外值或配对；特别是 SHM 不得单独报告为 hot/recoverable。非 busy 的 checkpoint 错误统一为 `checkpoint_failed`，不得伪装为 `checkpoint_busy`。该 envelope 不包含路径、文件内容或底层 SQLite/OS 文本；相同 fixture 必须得到同一 reason/artifact。

日志仅在处理边界记录一次已经过中央脱敏的内部 cause 摘要；原始 cause、凭据、prompt、模型响应和 Tool 输入输出不得在 adapter 间传播为可记录字段。事件和 UI 只暴露稳定类别、可操作中文摘要和 execution_id。

## 8. 运行日志耐久性

- 产品必须提供可由 contract test 直接调用的结构化日志边界；构造边界时注入 `Clock`，生产使用系统 UTC 时钟，测试使用确定性时钟。不得以 source-contract、全局 tracing subscriber 或 US13 后置 E2E 代替该公开边界。
- 持久化 `LogRecord` schema 固定为版本 1：`schema_version: 1`、`occurred_at: UTC RFC3339`、`execution_id: UUID`、`operation: stable string`、`entity_identifier: string|null`、`result: success|failure|cancelled`、`error_category: stable string|null`、`cause_summary: string|null`、`segments_ms: object<string,u64>`。`cause_summary` 只能由中央脱敏器从内部 cause 生成，UTF-8 编码后最大 512 bytes，超限时必须在 Unicode 标量边界安全截断；原始 cause、密码、API Key、备份口令、完整 prompt、模型响应和 Tool 输入输出不得进入任何持久化字段。同一错误跨 adapter 仍只允许在处理边界形成一条 `LogRecord`，压缩或恢复不得复制该记录。
- 当前可写日志段必须使用 `.open` 后缀；每条记录是单行 UTF-8 JSON，并且只有完整写入记录及其换行符后才成为可见记录。诊断读取不得返回半条或尚未完成的记录。
- 轮换只能发生在完整记录边界：先 flush/fsync 当前 `.open` 文件，再原子 rename 为不可变 `.jsonl` 段并 fsync 父目录；新 `.open` 文件必须排他创建，并在开始接受记录前持久化其父目录项。任何时间只允许一个活动写入段。
- 启动恢复只可丢弃活动 `.open` 文件末尾的单条不完整记录，已经换行结束的完整记录不得丢失或重复。轮换各持久化边界发生崩溃时，恢复结果必须是完整旧活动段或完整不可变段，不得把同一条记录同时暴露在两个段中。
- 每条记录的时间保留上限精确为 7×24 小时。有效当前时间取注入 `Clock` 与相邻日志目录中已持久化 retention high-watermark 的较大值；high-watermark 每次前进都必须通过同目录 staging 写入、flush/fsync、原子 replace 和父目录 fsync 持久化，时钟回拨不得降低它、复活已到期记录或改写既有 `occurred_at`。`occurred_at <= effective_now - 7×24h` 的记录即为到期，不能用段内最大时间近似整段年龄。
- 应用运行时必须为最早一条未到期记录安排时间轮转/清理；启动恢复后且在追加、诊断读取或导出任何记录前也必须先执行同一清理。活动 `.open` 含到期记录时必须先按本节轮转协议在完整记录边界耐久封段，再参与清理，不能因为活动段未达到容量阈值而延长记录寿命。应用停机期间到期的记录必须在下次启动对外暴露日志前清理。
- 全部记录到期的不可变段直接删除并 fsync 父目录。混合到期与未到期记录的不可变段必须逐行流式读取，把每条仍有效记录按原顺序且恰好一次写入同目录唯一、诊断读取不可见的 compact staging 文件；完成换行、flush/fsync 和校验后，以原子 replace 替换原段并 fsync 父目录。崩溃恢复只能观察原段或已压缩段；遗留 compact staging 在启动时验证后完成替换或删除，绝不能与原段同时作为可见日志，也不能丢失或复制未到期记录。
- 容量上限精确为活动 `.open` 与全部不可变 `.jsonl` 实际总计 100,000,000 bytes。追加下一条完整记录会超过容量时先耐久封段，再从最旧记录开始使用相同的整段删除或混合段原子压缩协议，直到新记录写入后仍满足上限；单条 v1 记录自身超过上限时必须在写入前拒绝。任何 high-watermark、删除、压缩、fsync 或 replace 失败都不得暴露部分文件或重复记录，诊断读取/导出必须 fail-closed，且不能把尚未物理移除的到期记录报告为满足保留策略。
