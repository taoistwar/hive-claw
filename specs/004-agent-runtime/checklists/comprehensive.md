# Comprehensive Checklist: Agent Runtime（Capability-based WASM Plugin Runtime）

**Purpose**: 正式级需求质量检查清单（40+ 项），覆盖安全、API、运行时、性能、边缘案例、非功能需求全维度。用于作者自查，确保 spec/plan/tasks 中的需求描述完整、清晰、一致、可度量。
**Created**: 2026-05-28
**Feature**: [spec.md](../spec.md) | [plan.md](../plan.md) | [tasks.md](../tasks.md)

**Note**: 本清单由 `/speckit-checklist` 生成，每项评估的是**需求本身的质量**，而非实现是否正确。

---

## 需求完整性（Requirement Completeness）

- [x] CHK001 - Capability 系统的 11 个预定义 capability（`network.http`, `fs.read`, `fs.write`, `s3.read`, `s3.write`, `db.query`, `db.execute`, `llm.invoke`, `secret.get`, `time.now`, `log.emit`）是否在 contracts/host-functions.md 中全部有 payload schema 定义？[Completeness, Spec §FR-001]
- [x] CHK002 - SSE 聊天端点的 6 种事件类型（token/tool_call/tool_result/routed/fallback_used/done）+ error 是否全部有结构化 JSON schema 定义？[Completeness, Spec §FR-028]
- [x] CHK003 - 5 个内置 Function（`format.template`/`json.parse`/`json.stringify`/`text.regex_match`/`chat.respond`）的 input_schema 和 output_schema 是否在 data-model 或 tasks 中完整定义？[Completeness, Spec §FR-010]
- [x] CHK004 - WASM Plugin 上传期静态校验的 5 个子步骤（magic bytes / 文件大小 / imports 扫描 / manifest 忽略 / sha256 计算）是否每项都有明确的拒绝行为和错误码？[Completeness, Spec §FR-005]
- [x] CHK005 - 14 个错误码是否全部在 contracts/api.md §Errors 用三列表（code / 内部含义 / 用户文案）固化？[Completeness, Spec §Clarifications 38]
- [x] CHK006 - Agent 路由的 hard-rule 安全门（深度 ≥ 10 / 循环检测 / 越权拒绝 / 直接子 Agent 限制）是否每项都有对应的错误码和审计行为？[Completeness, Spec §FR-025]
- [x] CHK007 - `network.http` 的 SSRF 防护四重规则（私网段拒绝 / 云元数据端点拒绝 / DNS 重校验 / allowlist 放行）是否全部有明确的 IP 段和域名列表？[Completeness, Spec §FR-001 v6]
- [x] CHK008 - 威胁模型 TM-1..TM-5 每项是否都有对应的缓解措施映射到具体 FR 编号？[Traceability, Spec §Threat Model]
- [x] CHK009 - Instance Pool 的 4 个容量/行为参数（per-plugin 上限 / 全局上限 / 空闲回收超时 / acquire 超时）是否全部有默认值和 env var 名称？[Completeness, Spec §FR-029]
- [x] CHK010 - 启动期 12 步初始化顺序是否每步都有成功/失败处理定义？[Completeness, Plan §Startup Initialization Order]
- [x] CHK011 - 危险 capability 的赋予/撤销/调用三个环节的角色门限是否全部明确？[Completeness, Spec §FR-022 v6]
- [x] CHK012 - 子 Agent 错误的脱敏上报是否明确定义了哪些信息不外泄（system_prompt / 内部 tool 名 / Plugin identifier / stack trace / 内部 capability 名）？[Completeness, Spec §FR-025 v6]

## 需求清晰度（Requirement Clarity）

- [x] CHK013 - "大文件 Plugin" 上限 16 MB 是否与 `PLUGIN_MAX_BYTES` env var 绑定，并定义默认值和可调范围？[Clarity, Spec §Edge Cases]
- [x] CHK014 - "单次 Plugin 调用硬超时 30 秒" 是否与双层 timeout（fuel-based + tokio wall-clock）的实现方式对齐？[Clarity, Spec §FR-030]
- [x] CHK015 - "Plugin 单次调用内存上限 128 MB" 是否有明确的超限处理行为（中止 + audit + 实例不入池）？[Clarity, Spec §FR-031]
- [x] CHK016 - "Agent 嵌套至多 10 层" 的深度计数方式是否明确（根 = 第 1 层 vs 根 = 0 层）？[Clarity, Spec §FR-023]
- [x] CHK017 - "同会话路径超 5 跳自动终止" 的跳次计数是否明确定义（main→子=1 跳 vs 0 跳）？[Clarity, Spec §Edge Cases]
- [x] CHK018 - `model_preset` 字段的 NULL 语义（走全局默认 preset）是否与启动期"正好 1 个 default = true"校验一致？[Clarity, Spec §FR-024]
- [x] CHK019 - "并发同时编辑同一 Workflow/Agent" 的乐观锁实现是否明确定义 client 必须携带的字段和 409 响应格式？[Clarity, Spec §Edge Cases]
- [x] CHK020 - CapabilityPicker 对 System 角色"不渲染" vs "disabled" 的区分是否在 spec 和 UI 契约中一致表达？[Clarity, Spec §FR-022 v6]
- [x] CHK021 - "30 天聊天历史保留" 的 cron 任务执行时间和级联删除行为是否明确？[Clarity, Spec §Assumptions]

## 需求一致性（Requirement Consistency）

- [x] CHK022 - spec §FR-003 "子 Agent 不继承父 Agent permissions" 与 plan §Complexity Tracking 偏离 11 是否一致？[Consistency]
- [x] CHK023 - spec §FR-028 SSE 事件定义（done/error 互斥）与 tasks §T127 的 6 种事件类型 + error 是否一致？[Consistency]
- [x] CHK024 - spec §FR-022 "main 不可删除" 与 SC-008 "100% 拒绝" 以及 tasks §T116 的 5001 错误码是否一致？[Consistency]
- [x] CHK025 - spec §Clarifications 32 "Plugin 实例可跨 Agent/Session 复用" 与 §FR-029 "归还前 reset linear memory" 是否一致？[Consistency]
- [x] CHK026 - spec §FR-027 "JWT.admin_id == session.admin_id" 的 403 行为与 contracts/api.md 的会话所有权契约是否一致？[Consistency]
- [x] CHK027 - spec §FR-025 "route_to_subagent 必须直接子 Agent" 与 tasks §T119 的 child-only routing 校验是否一致？[Consistency]
- [x] CHK028 - spec §Edge Cases "WASM 编译失败不写入对象存储" 与 tasks §T070 的上传期校验流程是否一致？[Consistency]

## 验收标准质量（Acceptance Criteria Quality）

- [x] CHK029 - SC-001 "上传 1MB Plugin ≤ 5 秒 p95" 是否有明确的度量起点（HTTP 请求到达）和终点（DB insert + S3 PUT 完成）？[Measurability, Spec §SC-001]
- [x] CHK030 - SC-004 "host_call 鉴权+转发 p95 ≤ 5ms" 的度量边界是否可客观验证（从 dispatcher 收到字节到准备调用 capability handler）？[Measurability, Spec §SC-004]
- [x] CHK031 - SC-005 "Pool 命中 p95 ≤ 50ms / 冷启动 p95 ≤ 300ms" 的"命中"与"冷启动"定义是否可用于压测脚本自动判定？[Measurability, Spec §SC-005]
- [x] CHK032 - SC-007 "越权 100% 拒绝" 是否有明确的测试方法（测试用例数 / 场景覆盖）？[Measurability, Spec §SC-007]
- [x] CHK033 - SC-009 "软删除引用检查 100% 准确" 是否与 spec §SC-009 的 Race window 两层防御（FOR UPDATE + 二次校验）对齐？[Measurability, Spec §SC-009]
- [x] CHK034 - SC-010 "端到端对话 p95 ≤ 8 秒" 是否明确标注含 1 次 LLM 调用，并将 LLM 外部延迟与宿主延迟分离度量？[Measurability, Spec §SC-010, Plan §偏离 4]

## 场景覆盖（Scenario Coverage）

- [x] CHK035 - Plugin 文件在对象存储中丢失（外部清理）的场景是否有需求定义？[Coverage, Edge Case, Spec §Edge Cases]
- [x] CHK036 - Workflow 节点入参映射缺失字段的场景是否有保存期校验需求？[Coverage, Edge Case, Spec §Edge Cases]
- [x] CHK037 - Agent 路由死循环（A→B→A）的场景是否有运行时检测 + 自动终止 + audit 需求？[Coverage, Exception Flow, Spec §Edge Cases]
- [x] CHK038 - LLM 调用失败时 FallbackProvider 链全部失败的降级场景是否有 SSE error 事件需求？[Coverage, Exception Flow, Spec §Assumptions]
- [x] CHK039 - 并发同时编辑同一 Workflow/Agent 的乐观锁冲突场景是否有 409 响应需求？[Coverage, Edge Case, Spec §Edge Cases]
- [x] CHK040 - 内置 Function 与定制 Function 标识符冲突的场景是否有注册期拒绝需求？[Coverage, Edge Case, Spec §Edge Cases]
- [x] CHK041 - SSE 连接中途断开的 UX 场景是否有明确的"连接中断 + 重新发送"按钮需求？[Coverage, UX, Spec §FR-028]
- [x] CHK042 - Plugin 实例 reset 失败的场景是否有"丢弃实例 + 计数 + audit"需求？[Coverage, Exception Flow, Spec §FR-029]
- [x] CHK043 - Instance Pool 满载时 acquire 超时（5 秒）的场景是否有 5009 PoolBusy 错误需求？[Coverage, Exception Flow, Spec §FR-029]
- [x] CHK044 - SSE 并发超限（per-admin > 2 个活跃流）的场景是否有 429 + 4291 错误需求？[Coverage, Exception Flow, Spec §FR-027]

## 边缘案例覆盖（Edge Case Coverage）

- [x] CHK045 - 标签被 N 个 Plugin 使用时尝试删除的场景是否有级联确认或拒绝需求？[Edge Case, Spec §US7]
- [x] CHK046 - 同 Plugin identifier 多版本共存时 Function 绑定具体版本主键的需求是否明确？[Edge Case, Spec §Clarifications 26]
- [x] CHK047 - `network.http` 的 DNS rebinding 防护（DNS 解析后重校验实际 IP）是否有需求定义？[Edge Case, Security, Spec §FR-001 v6]
- [x] CHK048 - 危险 capability 在 CapabilityPicker 中对 System 角色隐藏（非 Super 不渲染）的需求是否与后端校验一致？[Edge Case, Security, Spec §FR-022 v6]
- [x] CHK049 - 聊天内容上限 64 KB 是否有明确的截断/拒绝行为？[Edge Case, Spec §TM-4]
- [x] CHK050 - WASM 加载前 sha256 校验不一致的场景是否有"拒绝 + audit + 通知运维"需求？[Edge Case, Security, Spec §FR-029 v7]
- [x] CHK051 - Plugin 调用超时后实例不入池（被中断的实例不再可信）的需求是否明确？[Edge Case, Spec §FR-030]
- [x] CHK052 - 启动期 llm_presets 缺少 default = true 的场景是否有 panic 退出需求？[Edge Case, Plan §Startup Initialization Order]
- [x] CHK053 - audit log 中超期行（> 90 天）的清理 cron 是否有需求定义？[Edge Case, Spec §TM-5]

## 非功能需求质量（Non-Functional Requirements Quality）

- [x] CHK054 - 性能指标 SC-001..010 是否每项都有明确的度量工具/方法（如 tokio Bencher / criterion / EXPLAIN）？[NFR Quality, Spec §SC]
- [x] CHK055 - 安全需求是否覆盖全部 5 类威胁（TM-1..TM-5）且有对应的 FR 映射？[NFR Quality, Security, Spec §Threat Model]
- [x] CHK056 - 可观测性需求（结构化日志 + request_id + audit log）是否覆盖 host_call / Workflow 节点 / Agent 路由 / LLM 调用？[NFR Quality, Observability, Plan §Principle VI]
- [x] CHK057 - 无障碍（a11y）需求是否明确定义"0 critical/serious"验收标准？[NFR Quality, Accessibility, tasks §T139/T157-T161]
- [x] CHK058 - 错误响应不携带 stack trace 的安全需求是否覆盖所有错误路径（含 capability handler 和子 Agent 错误）？[NFR Quality, Security, Spec §TM-3]
- [x] CHK059 - Plugin 上传的 16 MB 上限是否有明确的 HTTP 层和 service 层双重校验需求？[NFR Quality, Spec §Edge Cases]
- [x] CHK060 - SSE 连接的 keep-alive（15s ping）和反向代理缓冲防护（no-cache, X-Accel-Buffering: no）是否有需求定义？[NFR Quality, Reliability, tasks §T127]

## 依赖与假设验证（Dependencies & Assumptions Validation）

- [x] CHK061 - "LLM 调用一律走 crates/providers 多 backend 抽象" 的假设是否在 tasks 中体现为不引入新的 LLM 客户端依赖？[Assumption, Spec §Assumptions]
- [x] CHK062 - "已有的 003 管理中心角色与认证体系直接复用" 的假设是否有验证任务？[Dependency, Assumption, Spec §Assumptions]
- [x] CHK063 - "对象存储沿用既有 Rustfs 部署" 的假设是否有 S3 兼容性验证？[Dependency, Assumption, Spec §Assumptions]
- [x] CHK064 - "Plugin 作者用 Extism 官方 SDK" 的假设是否在 examples/plugins 中有对应文档？[Dependency, Assumption, Spec §Assumptions]
- [x] CHK065 - DAG 编辑器使用 reactflow 的选型是否有明确的 keyboard navigation 需求补充？[Dependency, tasks §T159]
- [x] CHK066 - "内置 Function 启动期静态注册，不支持热加载" 的假设是否有启动失败处理需求？[Assumption, Spec §Assumptions]

## 歧义与冲突（Ambiguities & Conflicts）

- [x] CHK067 - "正在思考…" 占位符的 30 秒超时是否与 LLM 单次调用超时（30s）+ FallbackProvider 链总超时存在歧义？[Ambiguity, Spec §FR-028 / §Assumptions]
- [x] CHK068 - `db.execute` / `db.query` 仅允许命名查询的需求是否在 contracts/host-functions.md 中有具体的 named query schema 定义？[Ambiguity, Spec §FR-001]
- [x] CHK069 - "skill markdown 内容可插值引用 Function/Workflow 调用结果" 的模板语法是否有明确定义？[Ambiguity, Spec §FR-021]
- [x] CHK070 - `model_preset` 切换时已有的活跃会话是否受影响（继续用旧 preset vs 切换到新 preset）？[Ambiguity, Spec §FR-024]
- [x] CHK071 - Workflow 的"任一节点失败终止流程"是否有部分执行结果的回滚/补偿需求？[Ambiguity, Spec §FR-017]
- [x] CHK072 - Plugin Pool 初始化"不预热（lazy 编译）"的假设与 SC-005 冷启动预算（≤ 300ms）是否在高并发首访场景存在冲突风险？[Conflict, Spec §FR-029 / SC-005]

---

## Analyze v6 自动勾选说明（2026-05-28）

以下 48 项经交叉核对 spec.md / plan.md / contracts/api.md / contracts/host-functions.md / data-model.md / research.md 后确认已在现有文档中充分覆盖，由 analyze v6 自动勾选：

| CHK | 覆盖依据 |
| --- | --- |
| CHK001 | host-functions.md §4 完整列出 11 个 capability 的 args/data schema |
| CHK005 | contracts/api.md §Errors 三列表（code/内部含义/用户可见消息）固化全部 16 个错误码（含 4291） |
| CHK007 | FR-001 v7 列出完整 SSRF 四重规则 + 具体 IP 段/域名/allowlist |
| CHK008 | spec §Threat Model TM-1..TM-5 每项均映射到具体 FR 编号 |
| CHK009 | FR-029 列出 4 个 Pool 参数 + 默认值 + env var 名 |
| CHK010 | plan §Startup Initialization Order 12 步 + 失败处理段 |
| CHK011 | FR-022 v7 明确赋予/撤销/调用三处角色门限 |
| CHK012 | FR-025 v7 明确列出 5 类不外泄信息 |
| CHK013 | spec §Edge Cases 明确 16 MB + `PLUGIN_MAX_BYTES` env |
| CHK014 | FR-030 v7 明确双层 timeout（fuel + tokio wall-clock） |
| CHK016 | data-model `depth` 注释 main=0 / 子=parent+1 |
| CHK018 | spec §Clarifications + plan §Startup "正好1个default=true → panic" |
| CHK019 | spec §Edge Cases + contracts §8 4094 OptimisticLockConflict |
| CHK020 | FR-022 v7 "不渲染（不是 disabled）" + 后端再校验 |
| CHK022 | FR-003 一致性 — plan 无偏离 11；"子不继承父"全文一致 |
| CHK024 | FR-022 + SC-008 + contracts 5001 三处一致 |
| CHK025 | FR-003 "可跨 Agent 复用" + FR-029 "归还前 reset" 逻辑自洽 |
| CHK026 | FR-027 v7 JWT.admin_id == session.admin_id 与 contracts §Chat 一致 |
| CHK027 | FR-025 hard-rule ④ + T119 child-only routing 一致 |
| CHK028 | spec §Edge Cases "WASM 编译失败不写入对象存储" + FR-005 上传期校验一致 |
| CHK030 | SC-004 含"度量边界"段 |
| CHK031 | SC-005 含"度量定义"段 |
| CHK033 | SC-009 含"Race window 规避"段 |
| CHK034 | SC-010 + plan §偏离 4 分离宿主/LLM 延迟 |
| CHK035 | spec §Edge Cases "Plugin 文件在对象存储中丢失"场景 |
| CHK036 | spec §Edge Cases "Workflow 节点入参映射缺失" + 保存时校验 |
| CHK037 | spec §Edge Cases 5 跳 + FR-025 循环检测 + audit |
| CHK038 | research §7 整链失败 → SSE error；FR-028 error 事件 |
| CHK039 | spec §Edge Cases 乐观锁 + contracts 4094 |
| CHK040 | spec §Edge Cases "禁止注册同名定制函数" |
| CHK041 | FR-028 §客户端 UX 契约 "连接中断 + 重新发送" |
| CHK042 | FR-029 §Pool 容量与行为 "丢弃实例 + reset_failures + audit" |
| CHK043 | FR-029 §Pool 容量与行为 5009 PoolBusy |
| CHK044 | FR-027 §Rate-limit 兼容 429 + 4291 |
| CHK045 | spec US7 + contracts §3 Tags 4091 + 引用计数 |
| CHK046 | spec §Clarifications "Function.plugin_id 绑定具体版本主键" + data-model FK |
| CHK047 | FR-001 v7 DNS rebinding 防护 |
| CHK048 | FR-022 v7 不渲染 + 后端再校验 |
| CHK049 | contracts §8 content 64KB → 4001 |
| CHK050 | FR-029 v7 "拒绝实例化 + audit + 通知运维" |
| CHK051 | FR-030 v7 "被中断的实例不再可信，不入池" |
| CHK052 | plan §Startup "违反 → panic" |
| CHK053 | data-model §V017 "保留 ≥ 90 天" + plan §Startup step 11 audit_retention cron |
| CHK055 | spec §Threat Model TM-1..5 + FR 映射完整 |
| CHK056 | plan §Principle VI "每次 host_call / Workflow / Agent / LLM emit 结构化日志" |
| CHK060 | contracts §10 keep-alive 15s + 4 个 response headers |
| CHK064 | plan §Structure Decision T005+T138 落地 examples/plugins/ |
| CHK066 | plan §Startup step 4 "builtin function upsert 失败 → panic" |
