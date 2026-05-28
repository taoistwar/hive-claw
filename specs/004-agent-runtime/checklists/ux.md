# UX Requirements Quality Checklist: Agent Runtime

**Purpose**: 验证 `specs/004-agent-runtime/spec.md` 中 UX 相关需求的**完整性 / 清晰度 / 一致性 / 可测量性**，作为 spec 作者在进入 `/speckit-implement` 前的自查清单。
**Created**: 2026-05-26
**Feature**: [specs/004-agent-runtime/spec.md](file:///home/developer/agent/hive-claw/specs/004-agent-runtime/spec.md)
**Scope**: Spec + 复用 `crates/agent` / `crates/skills` / `crates/providers` 三 crate 后的调整面
**Audience / Depth**: 作者自查（轻量）— 仅列高影响项，不追求"所有可能问题"

---

## Requirement Completeness — UX

- [x] CHK001 Plugin 上传交互的进度反馈（sha256 计算 / S3 PUT 阶段进度）是否在需求中定义？[Gap]
- [x] CHK002 大文件上传（接近 16 MB 上限）失败时的用户反馈（错误码 + 中文描述）是否指定？[Gap, Spec §Edge Cases]
- [x] CHK003 SSE chat 的"等待首字"加载态（提交后到第一个 `token` 事件之间）UX 是否定义？[Gap, Spec FR-028] — ✅ FR-028 重写：「正在思考…」占位符 + 30s 超时
- [x] CHK004 SSE 中途断开后客户端可见的视觉反馈是否定义？[Gap, Spec §Edge Cases 流式断开] — ✅ FR-028 重写：「连接中断」+ 重新发送按钮；assistant 中断内容不写库
- [x] CHK005 Agent `route_to_subagent` 路由发生时，用户可见的提示（"已切换到 rust-expert"）是否在需求中描述？[Completeness, Spec FR-025]
- [x] CHK006 `tool_call` 事件渲染（用户看到 Agent 正在调用哪个 Tool + 入参摘要）的展示规则是否定义？[Gap, Spec FR-028]
- [x] CHK007 Workflow DAG 编辑器的空状态（新建 / 无节点）UX 是否定义？[Gap, Spec FR-015]
- [x] CHK008 Workflow 节点级失败（spec FR-017）在 UI 上的可见信号（高亮失败节点 + 错误消息）是否描述？[Gap, Spec FR-017]
- [x] CHK009 Skill markdown 编辑器的 frontmatter 校验时机（保存时 / 输入时实时）是否指定？[Gap, Spec FR-021]
- [x] CHK010 ModelPreset 下拉为空（启动 `llm_presets.toml` 缺失或为空）时的回退 UX 是否定义？[Gap, Spec FR-024]

## Requirement Clarity — UX

- [x] CHK011 "管理员在 Web 上拖拽 Function" 的精确交互（drag handle 在节点哪里 / 拖入位置如何决定）是否定义？[Clarity, Spec §US3]
- [x] CHK012 DAG 边连线时的端口语义（output 字段 → input 字段映射）是否在 UX 层描述？[Clarity, Spec FR-014]
- [x] CHK013 "客户端环检测红框预警"（T114）的红框作用范围（全图变红 / 仅环上节点变红）是否定义？[Ambiguity, T114]
- [x] CHK014 SSE 事件 `done` 携带"完整 assistant 消息"的具体字段结构是否在 spec 中明示？[Clarity, Spec FR-028]
- [x] CHK015 危险 capability 在 CapabilityPicker 中的"hidden = 不渲染"对 System 角色是 hard requirement 还是默认行为可被覆盖？[Clarity, Spec FR-022] — ✅ FR-022 补强：hard requirement；System 不渲染 dangerous 项；后端再校验
- [x] CHK016 Agent 编辑器中 `system_prompt` 的最大长度 + 行数 + 校验规则是否定义？[Gap]
- [x] CHK017 Skill markdown content 的 64 KB 上限超过时 UI 反馈（截断 / 拒绝保存 / 警告）是否定义？[Clarity, contracts/api.md §8]

## Requirement Consistency — UX

- [x] CHK018 Agent 编辑器的字段顺序（name / desc / tools / skills / permissions / model_preset / system_prompt）是否与 contracts/api.md §9 GET 返回字段顺序一致？[Consistency]
- [x] CHK019 "无权限 = 不渲染" 约定（沿用 003）是否在 004 涉及的 5 个新页面（Plugin/Function/Workflow/Agent/Chat）显式贯彻？[Consistency, 003-admin-center spec §US3 AS-1]
- [x] CHK020 错误码 5001/5006/5007/4030/4040/5004/4094/5008 的用户可见消息文案是否在 spec 中统一约定（避免每个 handler 自由发挥）？[Consistency, contracts/api.md §Errors] — ✅ contracts/api.md §Errors 升级为三列表（code / 内部含义 / 用户文案），全 14 项中文文案固化；前端集中字典 `web/src/utils/error_messages.ts`
- [x] CHK021 Plugin 列表的"已删除" 与 003 admin 列表的"已禁用" 的视觉处理（灰显 / icon / 标签）是否一致？[Consistency]
- [x] CHK022 SSE chat 与 003 已有的 SSE-like 消息组件（若存在）的视觉/交互是否统一？[Consistency, Gap]

## Acceptance Criteria Quality — UX

- [x] CHK023 SC-001 (Plugin 上传 ≤ 5s) 的"上传完成"时点是否定义（client 看到 200 / S3 写完 / DB 提交完）？[Measurability, Spec SC-001]
- [x] CHK024 SC-003 (Workflow save ≤ 1s) 的 50 节点 + cycle 检测的客户端感知是否包含 reactflow 渲染时间？[Measurability, Spec SC-003]
- [x] CHK025 SC-006 (Agent routing ≤ 1.5s) 包含 LLM 调用的可测量边界是否明确（不含 LLM 远程延迟 / 含 LLM）？[Measurability, Spec SC-006]
- [x] CHK026 SC-010 (端到端 ≤ 8s) 在用户可感知层面是"首字" 还是 "done"？[Measurability, Spec SC-010]
- [x] CHK027 SC-008 a11y 的"axe 0 critical/serious"是否在 spec 中明确覆盖 004 新增的 5 个页面 + DagEditor + ChatStream + ModelPresetSelect + CapabilityPicker？[Measurability, Spec SC-008]

## Scenario Coverage — UX

- [x] CHK028 Plugin 软删除后，Function 编辑器中的"plugin_id 下拉"是否过滤已删除项？UX 是否定义？[Gap, Spec FR-006]
- [x] CHK029 Workflow 中引用了已软删除 Plugin 的 Function 的节点，UI 上的视觉处理是否定义？[Coverage, Spec §US3 AS-3]
- [x] CHK030 Agent 编辑时勾选了未启动的 model_preset（启动后 toml 被改）的 UI 提示是否定义？[Coverage, Spec FR-024]
- [x] CHK031 聊天会话历史超过浏览器单页可视范围时的滚动 / 分页 / 加载更多 UX 是否定义？[Coverage, Spec FR-027]
- [x] CHK032 Skill 在 Agent 编辑器中被勾选后，预览 system_prompt 拼接结果的 UX 是否定义？[Gap, Spec FR-021]

## Edge Case Coverage — UX

- [x] CHK033 上传同 identifier 不同 version 的 Plugin 时（多版本共存允许），UI 上如何区分版本（badge / 列）？[Coverage, Spec FR-005 / §Edge Cases]
- [x] CHK034 Agent 嵌套到第 10 层时新建子 Agent 按钮的状态（禁用 / 隐藏 / 显示但提交时报错）是否定义？[Edge Case, Spec FR-023]
- [x] CHK035 main Agent 在 Agent 树中的视觉锚定（不可拖动 / 不可删除按钮隐藏）是否定义？[Edge Case, Spec FR-022]
- [x] CHK036 SSE 中 `error` 事件的 4030/4040/5004 等错误是否各自有特定的用户友好文案？[Coverage]
- [x] CHK037 路由跳次达到 `AGENT_MAX_HOPS=5` 上限时，用户在 Chat 界面看到的"终止"消息是否定义？[Edge Case, Spec FR-025]

## Non-Functional — UX

- [x] CHK038 a11y 需求覆盖到 DagEditor 的 keyboard navigation（如 Tab 顺序、Enter 创建节点、Delete 删除）是否在 spec 中显式声明？[Coverage, Spec FR-020]
- [x] CHK039 Monaco editor（用于 system_prompt / SchemaEditor / SkillMarkdownEditor）的 a11y 限制（jsdom 与浏览器差异）是否承认并描述？[Gap]
- [x] CHK040 reactflow 的 jsdom 测试限制（canvas API 不支持）是否在 a11y 测试需求中明示绕过策略？[Coverage, Gap]
- [x] CHK041 SSE 长连接的浏览器兼容性需求是否定义（最低支持 Chrome/Firefox/Safari 版本）？[Gap]
- [x] CHK042 i18n 需求是否定义（界面 / 错误消息 / SSE 事件 message 字段是否本地化）？[Gap, 沿用 003 默认中文]
- [x] CHK043 长 `chat_messages.content`（如 50 KB markdown）的渲染性能需求是否定义？[Gap]

## Dependencies & Assumptions — UX

- [x] CHK044 reactflow 11.x 的 API 稳定性假设是否在 spec/plan 中登记（升级风险）？[Assumption]
- [x] CHK045 Monaco editor 的 worker 加载失败时的 fallback 假设是否定义？[Gap, T036]
- [x] CHK046 假设管理员熟悉 markdown 和 JSON Schema 概念 — 这一假设是否在 spec 中明示？[Assumption]
- [x] CHK047 假设浏览器支持原生 EventSource（SSE）— 这一假设是否登记？[Assumption, Spec FR-028]

## Ambiguities & Conflicts — UX

- [x] CHK048 spec 中 `tool_call` 与 `routed` 事件在 SSE 流中的先后顺序与互斥关系是否明确？[Ambiguity, Spec FR-028] — ✅ FR-028 重写：事件类型 + 顺序 + 互斥规则完整列出（token/tool_call/tool_result/routed/done/error；done 与 error 互斥）
- [x] CHK049 `route_to_subagent` 是 Tool 还是特殊事件 — research §6 提"作为特殊 Tool"，spec FR-025 描述像"LLM 选择"，UX 层在哪个组件呈现？[Conflict 候选，research §6 vs Spec FR-025]
- [x] CHK050 Tool 与 Skill 在 Agent 编辑器多选框中是否区分视觉（不同 section / 不同 icon）？规范有无定义？[Gap, 复用 crates/agent 模式下的实现细节]

## 复用 crates 引发的 UX 调整面

- [x] CHK051 沿用 `crates/agent::SkillsLoader` 的 frontmatter 解析失败时的 UI 反馈是否定义？[Gap, Spec FR-021]
- [x] CHK052 `crates/agent::ToolRegistry` 启动期注册 5 个 builtin + DB 中 custom Tool — UI 上"内置/定制" 标签的视觉区分是否在需求中？[Gap, T080]
- [x] CHK053 `crates/providers::FallbackProvider` 链中途切到 fallback model 时是否在 UI 给用户提示（如"已切换备用模型"）？[Gap, research §7]
- [x] CHK054 多个 `LlmPresetName` 的命名约定（snake_case / kebab-case / 中文）是否在 spec 约束？UI 下拉如何排序？[Clarity, Spec FR-024]

---

## Notes

- 在 spec 上回答这些问题（→ 增 FR / 修 §Edge Cases / 加 §Clarifications），或在 plan/quickstart 落地具体 UI 规则。
- 这是"作者自查清单"—— 任何一项 `[ ]` 留空都不阻塞 `/speckit-implement`，但留空越多 = 实现期 UX 决策的随意性越大 = 上线后返工概率越高。
- 与 003-admin-center 已落地的 UX 约定（"无权限 = 不渲染" / 错误码格式 / a11y 自动检测）保持一致是最便宜的做法 — 任何偏差应在 §Clarifications 显式登记。
- 检测重点：**a11y / 错误反馈 / 边界状态（空 / 满 / 失败 / 断开 / 越限）**。这三类是 UX 需求最常被遗漏的地方。
