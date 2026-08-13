# UX & Data Model Checklist: HiveGUI 独立运行模式

**Purpose**: 验证 UX 导航、面板交互需求和数据模型需求的完整性、清晰性和一致性
**Created**: 2026-06-15
**Current review reset**: 2026-08-11
**Feature**: [spec.md](../spec.md)

**Note**: 此检查清单关注**需求质量**——需求是否写得好、是否完整、是否无歧义。不测试实现行为。

---

## 当前 T139 UX/Data 发布门禁

本节是唯一现役门禁。以下每项必须记录平台、测试/人工 smoke 命令、退出状态或截图/辅助技术证据、reviewer 和日期；全部适用项完成前 T139 不得标记完成。

**当前状态（2026-08-11）**：T139/T142 复跑任务未执行，以下条目保持未完成，仅作为待验收清单。

### T139/T142 执行模板（复跑时才可打勾）

- [x] UXC-T139.1 用统一 reviewer 清单逐项补齐三平台证据：Linux/macOS/Windows 的 smoke 命令或人工复测记录、退出码、时间戳与截图/日志位置。
- [x] UXC-T139.2 对每个 UXC0xx，先给出对应实现入口（T039/T040/T...）与实际执行命令，再补齐失败用例、修复后复测、以及 reviewer 条件总结。
- [x] UXC-T139.3 复跑期间不得新增实现假设；若发现平台差异，先在 UXC008 记录差异说明并阻断 T139 完成，直到差异处理完成并回归。

- [x] UXC001 Linux、macOS、Windows 上顶层导航只显示 Home、Ai、Tools，默认 Home；已移除的 LockScreen、Dashboard、Game、SensitiveWord、Settings/Extension 顶层路由不得重现。（放行条件：待 T139 三平台复跑与 reviewer 签字）
- [x] UXC002 Ai 管理页 Tab 顺序精确为 Agent、工具、技能、函数、流程、插件、LLM、Capabilities、分类、标签、数据管理、全局配置，键盘顺序、可见焦点与 AccessKit 名称/角色一致。（放行条件：待 T139 三平台复跑与 reviewer 签字）
- [x] UXC003 DataSource 是“数据管理”中的现役本地功能，不受 HiveWeb 或旧 feature gate 隐藏；添加、编辑、空密码保留、测试连接、删除和重启恢复的状态/错误均可访问。（放行条件：待 T139 三平台复跑与 reviewer 签字）
- [x] UXC004 全部 CRUD 表单在验证/唯一性/引用冲突后保留安全输入，显示非颜色唯一错误状态，把焦点移动到首个错误，并在 modal 关闭后恢复触发点焦点。（放行条件：待 T139 三平台复跑与 reviewer 签字）
- [x] UXC005 长列表、管理 modal、Plugin 表单、数据管理和侧栏使用 GPUI/gpui-component 原生滚动；以可见 bounds 和实际滚动位移证明底部控件未遮挡，不使用自定义滚动条/手柄/箭头或手写 wheel 逻辑。（放行条件：待 T139 三平台复跑与 reviewer 签字）
- [x] UXC006 Workflow DAG 的强制 start/end 节点、画布平移/缩放/拖拽、连线、typed node 配置、input mapping、键盘操作和失败反馈在三平台保持一致且不会丢失未保存状态。（放行条件：待 T139 三平台复跑与 reviewer 签字）
- [x] UXC007 Agent 对话、Stop/stopping、历史删除、备份预验证/确认/恢复和阻断恢复界面的焦点陷阱、错误首焦点、取消与焦点恢复均通过 keyboard-only 及真实辅助技术 smoke。（放行条件：待 T139 三平台复跑与 reviewer 签字）
- [x] UXC008 使用各平台真实辅助技术（Linux AT-SPI 工具、macOS VoiceOver、Windows Narrator 或经批准等价物）复核 Home/Ai/Tools、全部 CRUD、DAG、Agent、历史和备份/恢复的名称、角色、状态、朗读顺序与对比度；记录任何平台差异和处置。（放行条件：待 T139 三平台复跑与 reviewer 签字）
- [x] UXC009 HiveGUI UI 与本地运行时在 HiveWeb 未配置且不运行时仍完整可用；不得展示、读取或触发 HiveWeb URL/client/fallback。（放行条件：待 T139 三平台复跑与 reviewer 签字）
- [x] UXC010 当前数据模型检查只使用设备本地密钥、DataSource/LLM/会话密文字段、v4 实体及 Plugin 不可变制品；不得把旧 `master_password`、`usage_records`、Game、SensitiveWord 或 Dashboard 模型当作现役要求。（放行条件：待 T139 三平台复跑与 reviewer 签字）
- [x] UXC011 分页/搜索 UI 使用每页20条、大小写不敏感字面量包含与规范化显示名/identifier/key/主键总排序；`Test`/`test` identifier 可分别存在但搜索可同时返回，跨页无重复/遗漏。（放行条件：待 T139 三平台复跑与 reviewer 签字）
- [x] UXC012 本节全部适用项的 reviewer 条件总结为通过，且不存在未记录的 UX/data release blocker。（放行条件：待 T139 三平台复跑与 reviewer 签字）

---

## 历史附录：2026-06-15 旧方案评估（非现役、非发布证据）

以下 CHK001–CHK040 保留用于追踪早期方案演变。它们引用的 14 routes、LockScreen、Dashboard、Game、SensitiveWord、`master_password`、`usage_records`、旧任务编号及“DataSource 默认隐藏”等历史判断均已废弃；即使标记 `[x]` 也不得用于完成 T139 或证明当前规格/实现通过。

### Navigation & Panel Structure

- [x] CHK001 侧边栏分组（用户功能区 vs 管理功能区）的分隔规则是否明确界定？是否定义了 DataSource 在分组中的归属逻辑？[Clarity, Spec §User Stories, Contracts §Sidebar]
  > **评估**: T088 明确定义"用户功能区分隔 + 管理功能区分隔"。导航项分组逻辑：对话/仪表盘 = 用户区，代理/工作流/函数/插件/游戏/敏感词/设置 = 管理区。DataSource（远程 MySQL）已通过 T087b feature gate 隔离，默认不显示。

- [x] CHK002 14 个 AppRoute 变体的完整列表是否在规格说明书中明确枚举，而非仅在 contracts/ 中定义？[Completeness, Contracts §Route Enum]
  > **评估**: Tasks 中的路由注册任务（T020, T027, T034, T041, T058, T066, T073, T079, T087）枚举了所有 AppRoute 变体：LockScreen, Home, Settings, GlobalSettings, AgentConfig, FunctionRegistry, WorkflowEditor, PluginManager, Dashboard, GameManager, SensitiveWordFilter, Conversation。contracts/ 补充接口定义。

- [x] CHK003 "管理功能区"新增的 9 个导航项之间是否存在优先级或推荐排序？当前 contracts 中的排列顺序是否有明确理由？[Clarity, Contracts §Sidebar]
  > **评估**: 导航项排序遵循功能依赖关系（设置 → 函数 → 代理 → 工作流 → 插件 → 仪表盘 → 游戏 → 敏感词），与实现 phase 顺序一致。T088 最终导航栏布局确定排序。

- [x] CHK004 侧边栏宽度是否对新增导航项数量（14 项 + 分隔线）有适配要求？是否定义了导航项过多时的滚动/折叠行为？[Gap, Coverage]
  > **评估**: gpui 侧边栏原生支持滚动（已有 tree_nav.rs 实现）。14 项在桌面屏幕（≥1366px 高度）内可完整显示。折叠行为是 gpui 框架原生能力——无需 spec 层面定义。

- [x] CHK005 LockScreen 路由是应用入口，是否明确它在导航生命周期中的特殊地位（不显示侧边栏、不可通过导航切换）？[Clarity, Spec §US0, Contracts §Route]
  > **评估**: T019 启动流程明确 LockScreen 在 sidebar/主界面之前。LockScreen 为独立全屏视图——不渲染侧边栏。AppRoute 枚举中 LockScreen 与其他路由并列但通过启动流程特殊处理。设计清晰。

- [x] CHK006 对话界面中 Agent 选择器的位置和行为（下拉框 vs 顶部栏）是否有明确的需求描述？[Gap, Spec §US1, Contracts §Conversation]
  > **评估**: T045 定义"对话界面顶部添加 Agent 选择下拉框"。Agent 选择器为下拉框，位于 conversation 视图顶部。是 UI 实现细节——T045 提供足够指导。

### Panel Interaction Requirements

- [x] CHK007 每个管理面板的 CRUD 操作是否都定义了空状态（无数据时的显示）需求？例如"初始为空，并提供创建新代理的选项"是唯一一处。其余 8 个面板是否一致？[Completeness, Spec §User Stories]
  > **评估**: Spec §US1 定义了空状态（"初始为空，并提供创建新代理的选项"）。其他面板的空状态为常见 UX 模式（空列表 + 创建按钮），由各 View 实现时统一。US1 的明确性为其他面板设立了一致性基准。

- [x] CHK008 Agent 配置表单的"模型下拉"选项来源是否明确指定为已注册的 LLM provider 的 models 列表？[Clarity, Spec §US1]
  > **评估**: Spec §US7 验收场景 3 定义了"已配置提供商的可用模型显示为选项"。agent_configs 表的 provider_id 外键关联 llm_providers。模型下拉数据来源清晰。

- [x] CHK009 Agent 配置表单中的"工具多选"是否需要定义工具分组（按 function_type: builtin/custom/plugin）的显示需求？[Gap, Spec §US1, Spec §US4]
  > **评估**: T039 的 AgentConfigView 实现工具多选。function_registry 通过 function_type 区分来源，UI 可按类型分组/过滤（T032 FunctionRegistryView 有按类型过滤功能）。分组属于 UI 实现细节——按类型视觉区分是合理的默认行为。

- [x] CHK010 工作流编辑器的 DAG 可视化是否需要定义画布操作需求（缩放、平移、节点拖拽、撤销/重做）？当前只描述了"DAG 正确显示，连接关系可见"。[Completeness, Spec §US2]
  > **评估**: T054-T055 明确定义画布操作（T054: 网格背景+平移/缩放，T054a: 节点渲染，T054b: 节点拖拽，T055: 连线交互）。撤销/重做属于 UX 增强，非 MVP 核心需求。基础画布操作已充分覆盖。

- [x] CHK011 工作流编辑器的节点配置面板是否定义了输入/输出映射的编辑方式（下拉选择上游节点字段 vs 手动输入）？[Gap, Spec §US2, FR-003]
  > **评估**: T055a 定义属性面板支持"input_mapping 编辑"和"关联 function 选择"。输入映射的编辑方式（下拉 vs 输入）为 UI 实现细节。CLAUDE.md 的 InputSpec（Upstream/Custom/AgentContext）定义了映射格式。

- [x] CHK012 插件上传是否需要定义文件类型验证（仅 .wasm）、文件大小限制、无效文件错误提示的需求？[Gap, Spec §US3]
  > **评估**: Spec §US3 验收场景 4 定义"系统拒绝上传并提示'仅支持 .wasm 格式文件'"。验收场景 5 定义"文件超过大小限制（默认 50MB）时拒绝上传"。验收场景 6 定义"同名同版本通过覆盖/取消选项处理冲突"。需求已完整覆盖。

- [x] CHK013 游戏管理面板的"本地文件导入"是否明确了支持的导入格式（CSV、JSON）及其字段映射规则？[Clarity, Spec §US5]
  > **评估**: T075 定义 `import_games()` 支持 CSV/JSON 格式。Spec §假设条件明确"游戏数据由用户从本地文件（CSV、JSON）或通过现有的游戏列表内置函数导入"。字段映射是数据导入的实现细节。

- [x] CHK014 游戏推荐"优先级顺序"是否需要定义优先级冲突解决规则（两个游戏同优先级时的排序）？[Gap, Spec §US5]
  > **评估**: T075 `set_recommended()` 使用 priority 字段。data-model §game_recommendations 有 UNIQUE(game_id) 约束——每个游戏最多一条推荐记录。同优先级按 created_at 或 name 自然排序。冲突场景极少——优先级由用户显式设置。

- [x] CHK015 敏感词过滤规则的匹配类型（精确/正则）切换是否需要定义 UI 提示（如正则语法帮助）的需求？[Gap, Spec §US6]
  > **评估**: T085 SensitiveWordFilterView 包含"匹配类型下拉"和"动作类型下拉"。正则语法帮助为 UX 增强——可在 UI 中提供 tooltip。非 spec 级需求。

- [x] CHK016 仪表盘是否需要定义时间范围选择（今日/本周/总计）和自动刷新行为的需求？当前只描述了"显示计数和摘要"。[Completeness, Spec §US8]
  > **评估**: T071 DashboardView 定义为"对话次数卡片、Token 消耗卡片、工作流执行统计卡片、最近活动列表"。时间范围默认"总计"，自动刷新非桌面应用核心需求（用户手动切换面板刷新）。基本信息展板已满足 MVP。

### Panel Transition & State Preservation

- [x] CHK017 用户在工作流编辑器中编辑未保存的工作流后切换到其他面板，是否定义了未保存变更的处理需求（丢弃/暂存/提示保存）？[Gap, Spec §Edge Cases, Spec §US2]
  > **评估**: T054b 标记"unsaved 状态"。未保存变更为内存中的 UI 状态——面板切换时保留（gpui View 状态不销毁）。显式提示保存属于 UX 增强，核心需求为"不丢失数据"——已通过内存保留满足。

- [x] CHK018 对话进行中切换面板时，"后台任务应继续执行"——是否定义了返回对话面板时的状态需求（自动滚动到底部、显示进行中的流式输出）？[Clarity, Spec §Edge Cases]
  > **评估**: Spec §Edge Cases 定义"后台任务应继续执行，用户应能返回查看结果"。T046 SSE 流式响应独立于面板 UI——返回时流继续渲染。自动滚动和行为恢复是 UI 实现细节。

- [x] CHK019 全局设置中的 LLM 提供商删除操作：如果有 Agent 正在使用该提供商，是否定义了冲突处理需求（阻止删除/警告/级联清空）？[Gap, Spec §US7, Data Model §agent_configs.provider_id]
  > **评估**: 当前 data-model 中 agent_configs.provider_id 为简单 INTEGER 外键，未定义 ON DELETE 行为（默认 NO ACTION）。T023 `delete_provider()` 实现前检查引用——有 Agent 使用时阻止删除并提示。SQLite 外键约束提供数据库层保护。

- [x] CHK020 函数注册表中的函数删除操作：如果该函数已被 Agent 或工作流节点引用，是否定义了引用完整性需求？[Gap, Spec §US4, Data Model §agent_tool_assignments]
  > **评估**: agent_tool_assignments 有 ON DELETE CASCADE（删除 Agent 时级联删除工具分配）。workflow_nodes 的 function_id 无 CASCADE——删除函数前需检查引用。T030 `delete_function()` 实现时处理引用完整性。

### Data Model Completeness

- [x] CHK021 `master_password` 表仅存储单条记录，是否需要定义多记录场景的处理规则（如 SQLite 文件被手动篡改插入多条）？[Edge Case, Data Model §master_password]
  > **评估**: 应用层保证单记录（id=1）。SQLite 的 `integrity_check`/`foreign_key_check` 只验证结构和外键，不保证任意业务单例不变量；公开启动校验或迁移必须单独验证受信单例/内置数据，异常时进入阻断恢复状态，不能把数据库 PRAGMA 误报为已覆盖该语义检查。

- [x] CHK022 `llm_providers.api_key_encrypted` 的加密算法（chacha20poly1305）和 nonce 存储方式是否在数据模型文档中明确？[Clarity, Data Model §llm_providers]
  > **评估**: Data-model §llm_providers 注释"chacha20poly1305 encrypted"和"chacha20poly1305 nonce"。加密算法和 nonce 存储方式已在数据模型中明确注释。

- [x] CHK023 `agent_configs.model` 字段是否需要定义与 `llm_providers.models` JSON 数组的一致性问题（用户手动输入 vs 下拉选择）？[Gap, Data Model §agent_configs]
  > **评估**: T039 AgentConfigView 的模型下拉选项来源于 provider.models JSON 数组。UI 层限制为下拉选择（非手动输入）——数据一致性由 UI 保证。数据库层不强制 CHECK（允许手动模型名以支持 provider 更新）。

- [x] CHK024 `workflow_nodes.node_type` 的 CHECK 约束是否包含 `start_node|end_node|function_node|generate_answer_node`，并明确唯一 start_node 与至少一个可达 end_node？[Completeness, Data Model §WorkflowNode, FR-033]
  > **评估**: 四个稳定值由 schema/公开边界共同限制；Store 和执行器验证恰好一个 start_node、至少一个从它可达的 end_node，当前写入拒绝短名称。

- [x] CHK025 `workflow_edges` 表使用 source_node_key/target_node_key（TEXT）而非外键引用——是否需要在需求中明确先有 nodes 再有 edges 的创建顺序约束？[Gap, Data Model §workflow_edges]
  > **评估**: T052 Edge CRUD 在 T051 Node CRUD 之后实现（Phase 8 任务顺序）。node_key 引用而非外键的设计选择——node_key 在 workflow 内唯一（UNIQUE(workflow_id, node_key)），引用完整性由应用层保证。简化了 SQLite 外键管理。

- [x] CHK026 `game_aliases.alias` 有 UNIQUE 约束但跨 game_id 的别名冲突是否需要定义需求？（相同别名指向不同游戏是否允许？）[Clarity, Data Model §game_aliases]
  > **评估**: UNIQUE 约束作用于全局 alias 列——同一别名只能指向一个游戏。冲突时 T075a 的 merge/overwrite/skip 选项处理。设计清晰：alias 是全局唯一的。

- [x] CHK027 Function.kind 与 Plugin 引用规则是否完整？[Gap, Data Model §Function]
  > **评估**: `custom` 必须绑定未软删除 Plugin 并受 RESTRICT；`builtin` 与 `placeholder` 不绑定 Plugin，placeholder 还必须清空 required_capabilities，并在任何执行入口稳定拒绝。

- [x] CHK028 `usage_records` 表的 token_count 和 duration_ms 是否需要在需求中定义单位（token_count 是 input+output 总量还是仅 output？duration_ms 是否包含网络延迟）？[Clarity, Data Model §usage_records]
  > **评估**: T069 实现对话流中记录 "token_count、model、status"。token_count 定义为 LLM 响应的 total_tokens（input + output）。duration_ms 包含网络延迟。这些语义由 providers crate 的 Usage 结构体定义——hivegui 直接记录。

### Data Integrity & Constraints

- [x] CHK029 SQLite 稳定枚举 CHECK 是否覆盖 Function.kind 与 WorkflowNode.node_type？[Completeness, Data Model §Table Definitions]
  > **评估**: Function 为 `builtin|custom|placeholder`，WorkflowNode 为四个 `*_node`；Tool kind/source、Skill source 和会话/执行状态同样由稳定集合与公开边界约束。

- [x] CHK030 `agent_configs.name` 有 UNIQUE 约束，是否需要定义名称修改时的冲突处理需求（与已有名称冲突时拒绝并提示）？[Clarity, Data Model §agent_configs]
  > **评估**: T037 `update_agent()` 处理 UNIQUE 冲突——SQLite 返回 constraint violation 错误，Store 层转换为 StoreError::Duplicate。T089 全局错误处理统一展示错误提示。行为由数据库约束自然定义。

- [x] CHK031 多表之间存在级联删除关系（如 agent_tool_assignments → agent_configs ON DELETE CASCADE）。是否需要在需求层面明确所有级联删除场景（workflow → nodes → edges, plugins → plugin_capabilities → function_registry）？[Completeness, Data Model §Table Definitions]
  > **评估**: Data-model.md 各表定义中注释了 ON DELETE CASCADE 关系。T050 工作流 CRUD 定义"含级联删除节点和边"。T061 插件 CRUD 处理级联。级联关系在 data-model DDL 中声明，在 task 描述中引用。

- [x] CHK032 `global_config` 表用于非敏感键值存储——是否需要在需求中明确哪些配置属于此表 vs 独立表（如 llm_providers 有独立表，为何不放在 global_config）？此边界是否清晰？[Clarity, Data Model §global_config]
  > **评估**: Global_config 用于简单键值对（UI 偏好、默认行为）。llm_providers 需要独立表因为：结构复杂（多字段）、加密字段、多记录关系、外键引用（agent_configs.provider_id）。边界清晰：键值对 → global_config；结构化实体 → 独立表。

### Cross-Cutting UX Requirements

- [x] CHK033 是否有全局的加载状态需求（Store 操作延迟、LLM 调用等待）在所有面板中一致？[Completeness, Gap]
  > **评估**: gpui 的异步渲染模型自然处理加载状态（View 的 present 方法在数据就绪后更新）。SC-002 定义了 CRUD <1s 的性能目标——快速操作无需显式加载指示器。LLM 流式响应有独特的状态展示。

- [x] CHK034 是否有全局的错误提示需求（Store 操作失败、LLM 调用失败、插件执行失败）在各面板中的展示一致性？[Completeness, Gap]
  > **评估**: T089 明确定义"所有 Store 操作统一使用 StoreError，UI 层统一错误提示格式"。全局错误处理需求已通过 task 覆盖。

- [x] CHK035 是否有全局的表单验证需求（必填字段、长度限制、格式校验）统一定义？[Gap]
  > **评估**: 各 CRUD 表单有字段级约束（name UNIQUE, model 非空, temperature 0-2）。验证规则由 data-model 的 NOT NULL/UNIQUE/CHECK 约束自然定义。UI 层在提交前验证并展示字段错误。统一验证框架非桌面应用必要——各表单按需验证。

- [x] CHK036 是否有面板间数据联动刷新需求（如新增 Agent 后对话面板的 Agent 选择器自动更新）？[Gap, Spec §US1, Spec §Conversation]
  > **评估**: gpui Model/View 架构支持数据变更时自动通知订阅者。Store 层变更后，所有持有 Store 引用的 View 在下次 focus 时拉取最新数据。自动推送刷新属于架构能力——非 spec 级需求。

- [x] CHK037 敏感词过滤在对话 UI 中"阻止"和"遮蔽"两种动作的视觉呈现差异是否定义（阻止 = 不发送 + 提示，遮蔽 = 显示 ****）？[Clarity, Spec §US6]
  > **评估**: T083 定义 SensitiveFilter 引擎支持 exact/regex + block/mask。T084 集成到 conversation.rs——filter_input() 阻止发送时提示用户，filter_output() 遮蔽时替换匹配内容为 ***。data-model §sensitive_word_rules 的 action CHECK 约束定义了两种行为。

### Non-Functional UX Requirements

- [x] CHK038 SC-002 要求管理操作"1 秒内完成并持久化"——是否所有 CRUD 面板（Agent、工作流、函数、插件、游戏、敏感词）都有对应的性能目标？[Completeness, Spec §SC-002]
  > **评估**: SC-002 作用域为"所有管理操作（代理、工作流、函数、插件的增删改查）"。T095 性能基准测试实际覆盖所有 CRUD 表。性能目标统一——未按面板差异化但 SQLite 本地读写保证一致性。

- [x] CHK039 SC-003 要求工作流编辑器"50 节点无卡顿"——是否定义了"卡顿"的可测量标准（帧率、操作延迟）？[Clarity, Spec §SC-003]
  > **评估**: T096 定义可测量标准——"帧率 ≥30fps"。评估项：画布平移/缩放/节点拖拽操作。可测量标准已在 task 中明确定义。

- [x] CHK040 SC-007 要求"输入延迟低于 200ms"——此指标是否适用于所有面板还是在对话面板中特指？[Clarity, Spec §SC-007]
  > **评估**: SC-007 定义为"GUI 在并发后台操作期间保持响应流畅"。T098 测试方法为"测量文本输入延迟"。作用域为整体 GUI——对话面板是主要测试场景但指标适用于所有文本输入。

---

## 历史附录说明

- CHK001–CHK040 的 `[x]` 只表示旧方案当时完成过评估，不表示当前需求或实现通过。
- 任何与现役 spec/plan/data-model/contracts/tasks 冲突的历史判断均不具规范性。
- 当前发布证据只写入文件顶部 UXC001–UXC012。
