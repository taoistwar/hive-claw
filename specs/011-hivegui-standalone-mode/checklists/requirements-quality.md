# 需求质量检查清单: HiveGUI 独立桌面管理工具

**目的**: 验证需求规格文档的完整性、清晰性、一致性和可测量性
**创建日期**: 2026-06-27
**审查对象**: spec.md, plan.md, tasks.md
**受众**: 需求作者（用于改进需求规格文档）
**深度**: 标准审查（覆盖全部 13 个用户故事）

---

## 需求完整性 (Requirement Completeness)

- [x] CHK001 - 是否所有 9 个新增实体（Tag、Category、Capability、Plugin、Function、Workflow、Tool、Skill、Agent）的字段定义完整且共享契约边界清晰？ [Completeness, Spec §关键实体] ✅ 已完整定义所有 9 个实体；共享 ABI/节点值与 HiveWeb 对齐，Placeholder 是明确的 HiveGUI schema-only 扩展而不是隐式漂移
- [x] CHK002 - 是否为每个实体定义了完整的 CRUD 操作需求（list、get、create、update、delete）？ [Completeness, Spec §FR-014 至 FR-022] ✅ FR-014 至 FR-022 均明确要求"添加、编辑、删除"，用户故事验收场景覆盖完整 CRUD
- [x] CHK003 - 是否为所有实体定义了分页和搜索需求，并明确树形例外？ [Completeness, Spec §FR-024] ✅ 除 Category 外均为每页20条；Category 一次批量加载完整树且搜索保留祖先路径
- [x] CHK004 - 是否为 Category 的树形层级结构定义了完整需求（parent_id、树形视图展示）？ [Completeness, Spec §FR-015] ✅ FR-015 明确要求"UI 以树形视图展示层级结构"，§关键实体定义 parent_id FK→自身
- [x] CHK005 - 是否为 Plugin 的软删除机制定义了完整需求（deleted_at 字段、列表过滤）？ [Completeness, Spec §边界情况] ✅ §边界情况明确"Plugin 支持软删除（deleted_at 字段），列表默认过滤已删除记录"
- [x] CHK006 - 是否为 Tool 的 kind CHECK 约束定义了完整需求（`function-wrap`→function_id、`workflow-wrap`→workflow_id）？ [Completeness, Spec §边界情况] ✅ §边界情况明确 Tool 的稳定字符串 kind 与互斥目标外键
- [x] CHK007 - 是否为 Agent 的 parent_agent_id 循环引用检测定义了需求？ [Completeness, Spec §边界情况] ✅ §边界情况明确"Agent 的 parent_agent_id 不允许形成循环引用，保存时检测并提示错误"
- [x] CHK008 - 是否为 LLM 配置的三层结构（Preset/Provider/Model）定义了完整的关系和级联删除需求？ [Completeness, Spec §FR-009 至 FR-013] ✅ FR-009 至 FR-013 完整定义三层结构和级联删除
- [x] CHK009 - 是否为加密密钥管理定义了完整需求（生成、存储、丢失处理）？ [Completeness, Spec §加密密钥管理] ✅ §加密密钥管理定义了密钥存储位置、权限、首次生成、丢失处理
- [x] CHK010 - 是否为数据库完整性检查和恢复定义了需求？ [Completeness, Spec §FR-006/FR-044] ✅ FR-006/FR-044 明确新建、打开既有数据库和迁移提交前都执行 `integrity_check` 与 `foreign_key_check`；源库损坏/孤儿外键和可回滚迁移目标失败使用不同恢复分流

---

## 需求清晰性 (Requirement Clarity)

- [x] CHK011 - identifier 字段的验证规则是否明确定义了允许的字符集和格式？ [Clarity, Spec §字段验证规则] ✅ 明确定义"仅允许字母、数字、下划线、连字符（正则：`^[a-zA-Z0-9_-]+$`）"
- [x] CHK012 - slug 字段的验证规则是否明确定义了正则表达式 `^[a-z0-9-]+$`？ [Clarity, Spec §字段验证规则] ✅ 明确定义"仅允许小写字母、数字、连字符（正则：`^[a-z0-9-]+$`）"
- [x] CHK013 - JSON 字段的验证规则是否明确定义了"有效 JSON 格式"和"最大大小 1MB"的具体标准？ [Clarity, Spec §字段验证规则] ✅ 明确定义"必须是有效的 JSON 格式，最大大小 1MB"
- [x] CHK014 - color 字段的 HEX 格式是否明确定义了具体的格式要求（如 `#FF5733`）？ [Clarity, Spec §字段验证规则] ✅ 明确定义"格式为 HEX 颜色代码（如 `#FF5733`）"
- [x] CHK015 - “搜索范围为实体的 name 和 identifier 字段”是否明确定义大小写、任意位置包含、短词、特殊字符和索引行为？ [Clarity, Spec §FR-024] ✅ FR-024 固定 `hivegui-nfkc-casefold-v1`、golden fixture/显式升级重建和 `empty_after_normalization`；3+ 字符使用 FTS5 trigram、1–2 字符使用事务同步 short-gram，特殊字符按字面量，tokenizer/ID 不可用时 fail-closed 且不得退回业务表 `LIKE`/`SCAN`
- [x] CHK016 - "删除操作必须弹出确认对话框"是否明确定义了对话框的具体文案和交互流程？ [Clarity, Spec §UI 交互规范] ✅ §UI 交互规范明确文案"确定要删除 [实体名称] 吗？此操作不可撤销。"
- [x] CHK017 - "表单提交失败时保留表单数据"是否明确定义了哪些失败场景需要保留数据？ [Clarity, Spec §UI 交互规范] ✅ §UI 交互规范明确"提交失败（如数据库写入失败、唯一性约束冲突）时，保留表单数据，显示错误消息，不关闭表单"
- [x] CHK018 - “被 Model 引用的 Provider 不可删除”是否明确定义了如何检测引用关系和错误提示？ [Clarity, Spec §用户故事 4] ✅ 用户故事 4 场景 6 与 FR-010 明确：未被引用的 Provider 可删除；存在引用 Model 时返回引用冲突、保持 Provider/Model 零修改。Model 自身可从所属 Preset 列表直接删除
- [x] CHK019 - 删除 Preset 时对关联 Model、独立 Provider 和引用它的 Agent 分别如何处理，是否已明确定义？ [Clarity, Spec §边界情况] ✅ 被 Agent.model_preset 引用时阻止删除并列出 Agent；移除引用后只级联删除 Preset 下的 Model，独立 Provider 不随 Preset 删除
- [x] CHK020 - "Preset 名称必须唯一，最多一个 default=true"是否明确定义了当设置新 default 时如何处理旧 default？ [Clarity, Spec §边界情况] ✅ 用户故事 4 场景 8 明确"系统自动取消旧 Preset 的默认标志"

---

## 需求一致性 (Requirement Consistency)

- [x] CHK021 - 所有实体的 identifier 唯一性约束是否一致地定义了错误提示文案？ [Consistency, Spec §边界情况] ✅ §边界情况统一要求 identifier UNIQUE 约束，用户故事 8-13 场景 4 统一提示"identifier 已存在"
- [x] CHK022 - 所有实体的删除确认对话框文案是否一致（"确定要删除 [实体名称] 吗？此操作不可撤销。"）？ [Consistency, Spec §UI 交互规范] ✅ §UI 交互规范统一定义了删除确认对话框文案
- [x] CHK023 - 所有实体的空状态提示是否一致（"暂无数据，点击添加按钮创建"）？ [Consistency, Spec §UI 交互规范] ✅ §UI 交互规范统一定义了空状态提示
- [x] CHK024 - 所有实体的搜索无结果提示是否一致（"未找到匹配项"）？ [Consistency, Spec §UI 交互规范] ✅ §UI 交互规范统一定义了搜索无结果提示
- [x] CHK025 - 所有实体的分页大小是否一致定义为 20 条/页？ [Consistency, Spec §FR-024, SC-009] ✅ FR-024 和 SC-009 一致定义"每页 20 条"
- [x] CHK026 - Category 与 Agent 的子级删除策略是否分别明确且不存在歧义？ [Consistency, Spec §边界情况] ✅ Category 有子分类时阻止删除；Agent 删除时直接子 Agent 的 parent_agent_id 置 NULL。两种有意不同的策略已分别明确
- [x] CHK027 - 所有实体的字段长度限制是否一致（identifier/name/slug 255 字符、description 2000 字符）？ [Consistency, Spec §字段验证规则] ✅ §字段验证规则统一定义了长度限制

---

## 验收标准质量 (Acceptance Criteria Quality)

- [x] CHK028 - 每个用户故事的验收场景是否可客观测量和验证？ [Measurability, Spec §用户故事] ✅ 所有用户故事都使用"假设...当...则..."格式，可客观验证
- [x] CHK029 - 成功标准 SC-002 至 SC-011 中的性能指标（1 秒、500ms、200ms）是否可在测试环境中准确测量？ [Measurability, Spec §成功标准] ✅ 性能指标均为具体时间值，可在测试中测量
- [x] CHK030 - "连接测试在 5 秒内返回结果（超时视为连接失败）"是否明确定义了超时的起始和结束时间点？ [Measurability, Spec §SC-003] ✅ 明确"5 秒内返回结果（超时视为连接失败）"，语义清晰
- [x] CHK031 - "GUI 在后台操作期间保持响应"是否定义了具体的响应时间阈值和测试方法？ [Measurability, Spec §SC-005] ✅ SC-005 明确用户输入到可见反馈 p95≤100ms、主 UI 线程不得连续阻塞超过250ms，并要求所有测试时刻均可触发取消/停止控件

---

## 场景覆盖 (Scenario Coverage)

- [x] CHK032 - 是否定义了并发访问场景的需求（多个实例同时访问本地数据库）？ [Coverage, Gap] ✅ §边界情况明确"多个实例并发访问本地数据库时，仅允许一个实例写入，第二个实例报错退出"
- [x] CHK033 - 是否定义了大数据量场景的需求（如 1000+ 分类的树形渲染性能）？ [Coverage, Spec §SC-010] ✅ SC-010 明确"Category 树形视图渲染在 200ms 内完成（即使有 100+ 分类）"
- [x] CHK034 - 是否定义了网络不可达场景的需求（MySQL 连接测试时网络断开）？ [Coverage, Spec §SC-003] ✅ SC-003 明确"超时视为连接失败"，覆盖网络不可达场景
- [x] CHK035 - 是否定义了数据库文件损坏场景的恢复流程需求？ [Coverage, Spec §FR-006] ✅ FR-006 明确"启动时检测并提供重建选项"
- [x] CHK036 - 是否定义了加密密钥丢失场景的数据处理需求？ [Coverage, Spec §加密密钥管理] ✅ §加密密钥管理明确"密钥文件丢失时，已加密的数据无法解密，需重新配置"
- [x] CHK037 - 是否定义了 Category 删除时引用实体的 category_id 置 NULL 的具体场景？ [Coverage, Spec §边界情况] ✅ §边界情况明确"引用该 Category 的实体的 category_id 置为 NULL（不级联删除实体）"
- [x] CHK038 - 是否定义了 Agent 删除时子 Agent 的 parent_agent_id 置 NULL 的具体场景？ [Coverage, Spec §边界情况] ✅ §边界情况明确"将子 Agent 的 parent_agent_id 置为 NULL（不级联删除子 Agent）"

---

## 边界情况覆盖 (Edge Case Coverage)

- [x] CHK039 - 是否定义了 identifier 字段包含特殊字符（如中文、空格）的验证需求？ [Edge Case, Spec §字段验证规则] ✅ §字段验证规则通过正则 `^[a-zA-Z0-9_-]+$` 排除了中文和空格
- [x] CHK040 - 是否定义了 JSON 字段格式错误时的错误处理需求？ [Edge Case, Spec §字段验证规则] ✅ §字段验证规则要求"必须是有效的 JSON 格式"，无效时验证失败
- [x] CHK041 - 是否定义了 color 字段格式错误（如 `#GGG`、`red`）的验证需求？ [Edge Case, Spec §字段验证规则] ✅ §字段验证规则要求"HEX 颜色代码（如 `#FF5733`）"，排除无效格式
- [x] CHK042 - 是否定义了 slug 字段包含大写字母时的处理需求？ [Edge Case, Spec §字段验证规则] ✅ §字段验证规则通过正则 `^[a-z0-9-]+$` 排除了大写字母
- [x] CHK043 - 是否定义了 description 字段超过 2000 字符时的截断或拒绝策略？ [Edge Case, Spec §字段验证规则] ✅ §字段验证规则定义"最大长度 2000 字符"，超长时验证失败
- [x] CHK044 - 是否定义了 Tool 的 function_id 和 workflow_id 同时为空或同时非空的错误处理需求？ [Edge Case, Spec §边界情况] ✅ §边界情况通过 CHECK 约束确保互斥
- [x] CHK045 - 是否定义了 Agent 的 depth 字段与 parent_agent_id 层级不一致时的处理？ [Edge Case, Spec §FR-022/字段验证规则] ✅ depth 必须由 Store 根据父子关系自动计算，普通 UI/公开写入不得覆盖；保存时同时验证无环和 depth≤10

---

## 非功能需求 (Non-Functional Requirements)

- [x] CHK046 - 是否定义了日志记录的需求（日志格式、字段、存储位置、保留与崩溃恢复）？ [NFR, Spec §FR-041/SC-024] ✅ FR-041、SC-024 与 local-runtime contract 明确 v1 字段/类型、中央脱敏、临时 XDG 可测边界、`.open` 完整行、fsync→rename→父目录 fsync、崩溃恢复，以及 7×24 小时或 100,000,000 bytes 保留
- [x] CHK047 - 是否定义了错误恢复机制的需求（自动重试、手动恢复、数据备份）？ [NFR, Spec §FR-025] ✅ FR-025 明确定义"临时性错误自动重试最多 3 次，间隔递增（1s, 2s, 4s）；提供手动恢复功能，支持从备份文件恢复数据"
- [x] CHK048 - 是否定义了国际化/本地化的需求（当前仅支持中文，未来是否扩展）？ [NFR, Spec §假设条件] ✅ §假设条件明确"界面仅支持中文"
- [x] CHK049 - 是否定义了数据导出/备份的需求（用户数据备份到外部文件）？ [NFR, Spec §FR-026/FR-034至FR-036] ✅ 已定义单一 age 口令认证加密的 tar 备份包、全部实体/关系/WASM、no-replace 导出、root-handle no-follow 解析、跨设备重加密、派生搜索索引重建、WAL/sidecar 封闭边界与可重放原子完整替换；它不是明文 JSON 文件
- [x] CHK050 - 是否定义了应用升级时的数据迁移需求（旧版本数据兼容性）？ [NFR, Spec §FR-027] ✅ FR-027 明确定义"数据库 schema 版本管理，应用启动时检测版本号，自动执行迁移脚本，支持向前兼容，迁移失败时提供回滚选项"

---

## 依赖与假设 (Dependencies & Assumptions)

- [x] CHK051 - 是否明确定义了 SQLite 版本要求和兼容性约束？ [Dependency, Spec §假设条件] ⚠️ plan.md §Deviation 说明了 SQLite 选择理由，但未指定具体版本
- [x] CHK052 - 是否明确定义了 gpui 框架的版本要求和 API 稳定性假设？ [Dependency, Spec §假设条件] ⚠️ plan.md 提到使用 gpui，但未指定版本要求
- [x] CHK053 - 是否明确定义了 chacha20poly1305 加密算法的选择理由和替代方案？ [Assumption, Spec §加密密钥管理] ⚠️ spec.md 指定了 chacha20poly1305，但未说明选择理由和替代方案
- [x] CHK054 - 是否明确定义了"纯本地应用"假设的具体含义（是否允许本地网络访问）？ [Assumption, Spec §假设条件] ✅ §假设条件明确"HiveGUI 不连接任何远程 hiveclaw/hiveweb 服务器，是纯本地桌面应用"，§FR-002 保留远程 MySQL 连接功能

---

## 歧义与冲突 (Ambiguities & Conflicts)

- [x] CHK055 - "identifier 必须唯一"是否明确定义了大小写敏感性（`Test` 和 `test` 是否视为相同）？ [Clarity, Spec §FR-024, Data Model §索引与查询] ✅ identifier 唯一性与精确 lookup 明确使用 ASCII 字节级、大小写敏感语义，`Test` 与 `test` 可同时存在；搜索仍按 `hivegui-nfkc-casefold-v1` 大小写不敏感返回二者
- [x] CHK056 - "搜索范围为实体的 name 和 identifier 字段"是否明确定义了当两者都匹配时的排序优先级？ [Clarity, Spec §FR-024, Data Model §索引与查询] ✅ 同一实体多字段命中只返回一次且不做字段相关性优先级；列表/搜索使用规范化显示名、规范化 identifier/key、数值主键的升序总排序，缺失键为空字符串，跨页稳定
- [x] CHK057 - 删除 Preset 时的 Model/Provider 行为和 Agent 引用保护是否一致？ [Consistency, Spec §FR-011/用户故事 4] ✅ 被 Agent.model_preset 引用时 Preset 删除被阻止且零修改；移除引用后删除 Preset 只级联其 Model，独立 Provider 永不随 Preset 删除
- [x] CHK058 - "Category 删除时需处理子分类：阻止删除"与"删除 Category 时引用实体的 category_id 置 NULL"是否定义了执行顺序？ [Ambiguity, Spec §边界情况] ✅ 逻辑清晰：先检查子分类（有则阻止），通过后再将引用实体的 category_id 置 NULL
- [x] CHK059 - Placeholder Function 的持久化字段、UI 范围和不可执行错误是否完整定义？ [Clarity, Spec §FR-018] ✅ 明确 `builtin|custom|placeholder`、三个执行关系字段为空，并在 Capability/Plugin 前返回 `function_not_executable`
- [x] CHK060 - WorkflowNode 是否只有四个稳定 `*_node` 值且拒绝当前短名称写入？ [Consistency, Spec §FR-033] ✅ `start_node|end_node|function_node|generate_answer_node` 已在需求、数据模型、契约和任务中统一
- [x] CHK061 - Builtin 是否仅有四个下划线 identifier 且明确不提供点号别名？ [Consistency, Spec §FR-038] ✅ 点号 lookup/execute、记录与 alias 全部禁止，真实旧记录只由受信迁移事务重命名

---

**检查项总数**: 61
**已评估**: 61 项 ✅
**已充分定义**: 58 项 ✅
**待改进**: 3 项 ⚠️（非阻塞性，属于文档改进建议）
**未覆盖**: 0 项 ❌

**评估条件总结**: 61 项中 58 项已充分定义。剩余 3 项 ⚠️ 为后续文档改进：SQLite 版本约束、GPUI revision/API 稳定性和 chacha20poly1305 选择理由；identifier 唯一性的大小写语义与 name/identifier 同时命中的稳定总排序已在本轮闭合，日志、GUI 响应阈值、Agent depth 与 Preset/Provider 语义也不再列为待改进项。
