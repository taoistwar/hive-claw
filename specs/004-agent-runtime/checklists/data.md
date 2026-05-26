# Data Model Requirements Quality Checklist: Agent Runtime

**Purpose**: 验证 `specs/004-agent-runtime/data-model.md` 中数据模型需求的**完整性 / 清晰度 / 一致性 / 可测量性**——表结构、外键策略、约束、不变量、索引、迁移顺序是否充分。
**Created**: 2026-05-26
**Feature**: [specs/004-agent-runtime/data-model.md](file:///home/developer/agent/hive-claw/specs/004-agent-runtime/data-model.md) + [spec.md](file:///home/developer/agent/hive-claw/specs/004-agent-runtime/spec.md) + [contracts/api.md](file:///home/developer/agent/hive-claw/specs/004-agent-runtime/contracts/api.md)
**Scope**: 18 张表（V008..V018）+ FK 策略 + 不变量 + 索引 + Capability 静态注册表
**Audience / Depth**: 作者自查（轻量）—— 仅列高影响项；阻塞实施的 hard requirement 以"⚠"标注

---

## 实体完整性

- [ ] CHK123 是否每个 spec 中的 Key Entity（Capability/Category/Tag/Plugin/Function/Workflow/WorkflowNode/WorkflowEdge/Tool/Skill/Agent/ChatSession/ChatMessage/AuditLog）都有对应的 DB 实体或代码注册表？[Completeness, data-model §1]
- [x] CHK124 V006 `audit_logs`（003 admin 审计）与 V017 `runtime_audit_logs`（004 runtime 审计）的职责边界是否在 data-model 显式区分？[Clarity, Gap] — ✅ data-model §V017 加 4 行注释明示职责分工（admin 配置写入 vs runtime 资源访问）
- [ ] CHK125 `agent_tools` / `agent_skills` / `agent_permissions` 三个多对多表的关系是否在 spec / data-model 完整描述（不只是 DDL 出现）？[Completeness, data-model §V015]
- [ ] CHK126 Capability 静态注册表（代码侧）与 V008 `capabilities` 表（DB 元数据）的同步契约是否定义（启动期 upsert / 删除策略）？[Clarity, data-model §V008 + §6]

## 字段与类型一致性

- [ ] CHK127 `plugins.sha256 CHAR(64)` 与 spec FR-005 上传期 / FR-029 加载期校验的 sha256 长度（64 hex）是否一致？[Consistency, FR-005 v7 + FR-029 v7 + data-model §V011]
- [ ] CHK128 `agents.identifier VARCHAR(64)` 是否够长容纳"main" + 多层专家命名（如 `coding/rust-async`）？[Clarity, Spec §FR-022]
- [ ] CHK129 `agents.model_preset VARCHAR(64)` 与 `llm_presets.toml` 中 preset name 的字符集 / 长度约束是否在 spec / research 明示？[Clarity, FR-024 v4]
- [ ] CHK130 `runtime_audit_logs.payload_summary JSON` 的字段黑名单 / 截断长度是否定义（避免大 payload 撑爆表）？[Gap, FR-004 / TM-3]
- [ ] CHK131 `chat_messages.content TEXT` 的最大长度（spec 限 64 KB）与 MySQL TEXT 类型上限（65,535 bytes）的关系是否明确（边界冲突）？[Consistency, contracts/api.md §8 vs MySQL TEXT 限制]
- [ ] CHK132 `workflow_edges.mapping JSON` 的 schema（"dst.input.foo": "src.output.bar"）是否在 data-model / contracts 中显式定义？[Clarity, data-model §V013]

## 唯一性约束

- [ ] CHK133 `plugins (identifier, version)` UNIQUE 是否能覆盖"同 identifier 多版本共存 + Function 绑定具体版本"的需求？[Coverage, Clarify v1.4]
- [ ] CHK134 `functions.identifier` 全局 UNIQUE 是否够（不需要按 plugin / category 区分）？建议是否在 spec 明示？[Clarity, data-model §V012]
- [ ] CHK135 `agents.identifier UNIQUE` 是否在 main + 子 Agent 嵌套场景中合理（子 Agent 与父 Agent 共用 identifier 空间）？[Clarity, Spec §FR-022]
- [ ] CHK136 `categories (parent_id, slug)` UNIQUE 是否允许跨 parent 同名 slug（如 "Coding/Rust" 与 "DataScience/Rust"）？[Clarity, data-model §V009]
- [ ] CHK137 `chat_messages (session_id, seq)` UNIQUE 单调性约束是否在并发写场景下安全（多个 SSE event 并发写）？[Coverage, Edge Case]

## 外键策略 (ON DELETE) 一致性

- [ ] CHK138 是否每个 FK 的 ON DELETE 选择（CASCADE / SET NULL / RESTRICT）都在 spec / data-model 给出明确**理由**而非只是 DDL 文字？[Completeness, data-model §V014..V016]
- [ ] CHK139 `functions.plugin_id ON DELETE RESTRICT` 与 Plugin 软删除（FR-007）是否冲突？— 软删除是 deleted_at IS NOT NULL，不是物理删除；FK 不会拦。但 service 层规则是否在 spec 显式（"被未删除 Function 引用的 Plugin 不能软删"）？[Clarity, data-model §V012 + FR-007]
- [ ] CHK140 `agents.parent_agent_id ON DELETE RESTRICT` 与"删除有子 Agent 的 Agent → 4093"是否一致？data-model 不变量 #2 是否覆盖？[Consistency, data-model §V015 + contracts §9 DELETE]
- [ ] CHK141 `tools.function_id` / `tools.workflow_id` ON DELETE RESTRICT 与"被 Tool 引用的 Function/Workflow 不能删"的 service 层规则是否对齐？[Consistency, data-model §V014]
- [x] CHK142 `chat_sessions.admin_id ON DELETE SET NULL` 与 FR-027 "session 绑定 admin_id" 是否冲突？admin 删除后 session 还能被 Super 访问吗？[Clarity, Gap, FR-027 v7] — ✅ data-model §V016 加 snapshot 列 + 所有权语义注释（admin 删后 admin_id IS NULL → 仅 Super 可访问）
- [ ] CHK143 `runtime_audit_logs` 没有 FK 到 agent_id / plugin_id —— 这是 intentional 的（audit 不可级联删除）— 是否在 data-model 显式说明？[Clarity, data-model §V017]

## 不变量（应用层强制）

- [ ] CHK144 data-model §4 列出的 9 项不变量是否每条都标明"谁强制"（DB 约束 / service 层 / DB+service 双重）？[Completeness, data-model §4]
- [ ] CHK145 不变量 #2 "main 不可删除" 是否在 service + API 层双重校验（防绕过）？[Consistency, FR-022 + data-model §4]
- [ ] CHK146 不变量 #6 "DAG 无环" 的检测**时机**（保存时 vs 执行时）是否明确？[Clarity, data-model §4 + FR-016]
- [ ] CHK147 不变量 #8 "危险 capability 仅 Super 可授予" 是否覆盖**撤销**（CHK063 v7 决议）？[Consistency, data-model §4 vs FR-022 v7]
- [x] CHK148 是否缺少不变量 #11 "agents.model_preset 必须存在于 llm_presets" 与不变量 #12 "FK ON DELETE SET NULL 后 admin_id NULL 但 session 仍可被 Super 读" 等新引入约束？[Completeness, Gap] — ✅ data-model §4 不变量 #11 (Tool schema 等值) / #12 (chat_session admin NULL Super-only) / #13 (Function 不软删除 + 引用阻塞) 三条新增

## 索引

- [ ] CHK149 spec SC-002 (Plugin 列表 + 三维检索 p95 ≤ 1s) 在 500 条数据集下，`plugins (category_id)` + `taggings (entity_type, entity_id)` + `FULLTEXT(name, description, identifier)` 是否足够覆盖热查询？[Coverage, data-model §5]
- [ ] CHK150 仪表盘类查询 "最近 N 条审计" 需要 `runtime_audit_logs.idx_occurred_at DESC`（已有），但"按 agent_id 统计调用数"是否需要 `(agent_id, occurred_at)` 复合索引？[Gap]
- [ ] CHK151 `chat_messages (session_id, seq)` UNIQUE 是否同时承担"按 session 拉历史"的索引职责（不需要额外 idx_session_seq）？[Clarity, data-model §V016]
- [ ] CHK152 `taggings` 表的 PRIMARY KEY `(tag_id, entity_type, entity_id)` 是否覆盖"某 entity 的所有 tag"反向查询（需要额外索引）？— 已有 `idx_taggings_entity`，是否在 data-model §5 显式标注？[Coverage, data-model §V010]
- [ ] CHK153 长期增长表（`runtime_audit_logs` / `chat_messages`）的分区 / 归档策略是否在 spec / data-model 提及？[Gap, scale]

## 迁移顺序与依赖

- [ ] CHK154 V008..V018 的拓扑顺序是否在 data-model 明确（不可乱序）？[Completeness, data-model §2]
- [ ] CHK155 V018 seed main agent 假定 V015 agents 表已创建 + `chk_agents_depth` CHECK 已生效 — 这一依赖是否在 data-model 标注？[Clarity, data-model §V018]
- [ ] CHK156 启动期 V008 `capabilities` 表的 upsert（与代码注册表同步）是否在 data-model 提及，避免 "新增 capability 但 DB 元数据缺失"？[Coverage, data-model §V008]
- [ ] CHK157 V006 `audit_logs`（来自 003）与 V017 `runtime_audit_logs`（004 新建）共存策略：tasks.md T015 创建 V017 是否假定 003 V006 已 applied？[Clarity, Coverage]
- [x] CHK158 5 个 builtin function 的 seed 是 V018 一次性写入还是启动期代码 upsert？两者都做时如何避免 race？[Clarity, FR-010 v5 + data-model §V018] — ✅ data-model §V018 明示：capabilities 元数据由 V018 seed；builtin function 由启动期代码 upsert（理由：schema 跟随代码版本演进；UNIQUE 兜底并发）
- [ ] CHK159 是否有 down-migration（回滚）策略？还是 forward-only（沿用 003）？[Gap, 沿用 003 forward-only 假设需明示]

## Capability 元数据 (V008)

- [ ] CHK160 V008 `capabilities` 表 vs 代码 const 列表的真值源（source of truth）哪个优先？[Clarity, Gap]
- [ ] CHK161 启动期发现代码列表 vs DB 不一致时的处理（drop DB 多余 / 写 warn / 拒绝启动）是否定义？[Coverage, Edge Case]
- [ ] CHK162 `is_dangerous` 字段从 false → true（升级危险等级）时，已授予该 capability 的 Agent 是否自动收回？或需手动 audit？[Edge Case, Gap]

## Skill 持久化（Q3 v3 markdown 模式）

- [ ] CHK163 `skills.content MEDIUMTEXT` 64 KB 软上限是否在 contracts 明示 + DB 字段类型(MEDIUMTEXT 上限 16 MB) 选择是否合理？[Consistency, contracts §8 vs data-model §V014]
- [ ] CHK164 `skills.frontmatter JSON` 是否在 spec 定义"允许的 key 子集"（无约束 JSON 会让 SkillsLoader 行为不可预测）？[Clarity, Gap, FR-021 v3]
- [ ] CHK165 内置 Skill（`source = 'builtin'`）的 seed 是 V018 还是启动期 upsert？怎么区分用户上传同 identifier 的 Skill？[Clarity, Gap]
- [x] CHK166 Skill 是否有"被引用阻塞删除"语义？目前 `agent_skills` 是多对多 CASCADE — 删除 Skill 会悄无声息从 Agent 移除，是否符合 spec FR-009 类的"引用阻塞"原则？[Conflict 候选, FR-007 vs data-model §V014] — ✅ data-model §V014 Skill DDL 上方加 5 行注释：删除 Skill 前 service 必须查 `agent_skills` 引用计数；不依赖 CASCADE；与 Tool/Function/Plugin 一致的引用阻塞模型

## Plugin 元数据 (FR-005 v7)

- [ ] CHK167 FR-005 v7 要求"忽略 manifest 中的 allowed_hosts / allowed_paths"—— `plugins.manifest JSON` 字段是否仍存储但运行时忽略？data-model 是否明示？[Clarity, FR-005 v7 vs data-model §V011]
- [ ] CHK168 `plugins.s3_key` 命名约定（含 sha256？含 version？防覆盖）是否在 data-model / spec 明确？[Clarity, Gap, CHK108-related]
- [ ] CHK169 `plugins.deleted_at` 软删除后 sha256 / s3_key 是否保留供 audit 追溯？是否在 data-model 明示？[Clarity, FR-006 + audit retention]

## 跨实体一致性

- [x] CHK170 `tools.input_schema` / `tools.output_schema` 与其引用的 `functions.input_schema` / `output_schema` 在 kind=1 时**完全相等**的约束是否在 spec / data-model 中明示？[Consistency, FR-019 + data-model §V014] — ✅ data-model §4 不变量 #11 新增：tool kind=1 → schema 深度等值校验，不一致返 5002
- [ ] CHK171 Workflow-wrapped Tool（kind=2）的 input_schema 与 workflow 入口节点 function 的 input_schema 之间的约束（"必须可赋值给"）是否在 data-model 明示？[Clarity, FR-020]
- [ ] CHK172 `agent_permissions.capability` 必须存在于 `capabilities.name` —— FK 约束缺失（取舍：capability 是代码注册表，DB 元数据只是镜像）；service 层校验是否在 data-model §4 明示？[Consistency, data-model §4 #7]
- [ ] CHK173 `workflow_nodes.function_id ON DELETE RESTRICT` 与 Function 是否硬删除（functions 表无 deleted_at）— 这是有意的吗？data-model 是否明示 Function 不软删除？[Clarity, data-model §V012 vs §V013]

## 命名 / 术语一致性

- [ ] CHK174 `LlmPresetName`（research 术语）vs `agents.model_preset`（data-model 字段名）vs `model_preset` (contracts 字段)三处命名是否一致？[Consistency, research §7 + data-model §V015 + contracts §9]
- [ ] CHK175 "identifier" 含义在不同实体里是否一致（user-facing slug 而非 DB id）？[Consistency, all]
- [ ] CHK176 V004 `login_records` snapshot 列（admin_phone_snapshot / admin_nickname_snapshot）模式是否扩展到 004 的 audit / chat 场景？[Gap, 沿用 003 模式]

## 可测量性

- [ ] CHK177 spec SC-005 (Pool 命中 p95 ≤ 50ms / 冷启动 ≤ 300ms) 的"命中" 定义是否在 data-model / spec 明示（实例已在 `PluginPool::idle` 而非 created）？[Measurability, FR-029]
- [ ] CHK178 spec SC-007 (capability denial 100% 准确) 的"准确"可测量吗？是否定义 audit log 抽样验证方法？[Measurability, FR-003 / TM-1]
- [x] CHK179 SC-009 (Plugin 软删除引用检查 100% 准确) 的边界：如果检查时刻 Function 未删除但 Plugin 软删后并发创建了引用 Function —— 是否在 spec 定义 race window？[Edge Case, Gap] — ✅ spec SC-009 补 Race window 规避段：`SELECT ... FOR UPDATE` 锁定 + Function INSERT 时二次校验 deleted_at + rollback；两层防御

## Dependencies & Assumptions

- [ ] CHK180 MySQL 8.0+ 假设（FULLTEXT in InnoDB / DESC index / CHECK 约束 / JSON 列）是否在 spec 明示？[Assumption, 沿用 003]
- [ ] CHK181 是否假定 `runtime_audit_logs` 与 003 `audit_logs` 在同一数据库实例？跨实例时审计能否 join？[Assumption, Gap]
- [ ] CHK182 sqlx compile-time SQL 验证开启 = 所有查询必须 schema 可达；这对动态 query（如 list 三维检索）是否有限制？data-model 是否提及？[Assumption, Gap]

---

## Notes

- 标 ⚠ 的 hard requirement：无（data model 多为软约束，violation 不会立即生产故障但会让后续重构付出代价）。
- 重点关注 **FK 策略 + 不变量 + 命名一致性**：这三类问题在 implement 期发现的成本远高于 spec 期发现。
- 与 003-admin-center 已落地的 schema 模式（snapshot 列 / forward-only migration / FULLTEXT 索引）保持一致是最便宜的做法。
- 这是"作者自查清单"——任何一项 `[ ]` 留空都不阻塞 `/speckit-implement`，但 implement 期才发现的 schema 不一致会引发迁移返工。
