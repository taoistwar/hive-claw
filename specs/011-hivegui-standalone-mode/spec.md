# 功能规格说明书: HiveGUI 独立桌面管理工具

**功能分支**: `260517-hivegui-standalone-mode`
**创建日期**: 2026-06-15
**状态**: 草案 (大幅简化)
**最后更新**: 2026-07-02
**输入**: 用户描述: "将 hivegui 变为独立运行的桌面数据源管理工具，不依赖远程 hiveclaw/hiveweb 服务器。"

## 澄清记录

### Session 2026-06-15

- Q: hiveweb 和 hivegui 分别使用什么数据库？ → A: hiveweb 提供 SaaS 服务，使用远程 MySQL；hivegui 提供私有独立服务，使用嵌入式 SQLite。HiveGUI 不与任何远程 MySQL 通信。
- Q: HiveGUI 桌面应用是否需要登录认证？ → A: 需要本地密码认证 —— 首次启动时用户设置主密码，后续每次启动需输入密码解锁应用。（**已废弃** — 当前版本无认证。）
- Q: hiveweb 到 hivegui 的数据迁移方式？ → A: 不提供自动迁移 —— 用户在 hivegui 中手动重新创建配置（Agent、工作流、函数等），保证应用简洁，避免导入兼容性问题。（**已废弃** — 不涉及 Agent/工作流迁移。）
- Q: hivegui 现有的远程 MySQL 数据源管理功能如何处理？ → A: 保留为可选高级功能 —— hivegui 自身存储使用 SQLite，但远程 MySQL 连接（用于浏览外部数据库的表数据）作为独立的高级特性保留，与核心功能隔离，默认隐藏。（**已变更为核心功能** — 数据源管理是当前唯一功能。）
- Q: WASM 插件在 hiveweb 和 hivegui 之间的兼容性？ → A: 完全兼容（**已废弃** — 插件管理已移除。）
- Q: hivegui 应包含哪些内置函数（Builtin）？ → A: 全部复用（**已废弃** — 内置函数已移除。）

### Session 2026-06-17

- Q: 用户故事 5（游戏数据与推荐游戏，P3）是否保留？ → A: 不要（**已废弃** — 已从代码中移除。）

### Session 2026-06-23

- Q: GameManager 在 spec 中已移除但代码中仍存在，是否删除？ → A: 已删除。
- Q: 仪表盘的访问方式？ → A: Home 即 Dashboard（**已废弃** — 仪表盘已移除。）
- Q: 是否需要数据导出/备份功能？ → A: 提供手动备份/恢复（**已废弃**。）
- Q: 从备份导入时数据冲突如何处理？ → A: updated_at 时间戳合并（**已废弃**。）
- Q: HiveGUI 是否需要支持多语言（i18n）？ → A: 仅中文。

### Session 2026-07-01

- Q: 当前 hivegui 的实际功能范围是什么？ → A: 仅保留数据源管理功能。Agent 配置、工作流编辑器、插件管理、函数注册表、敏感词过滤、仪表盘、Day+1/Hour+1 工具、对话 UI、认证锁屏 —— 全部移除。不再连接远程 hiveclaw/hiveweb 服务器。
- Q: 仿照 hiveweb 添加全局配置，schema 如何设计？ → A: 完整复制 hiveweb 的 global_configs 表结构（id, name, key, type, data, created_at, updated_at），支持分页和搜索。
- Q: LLM 配置中 Preset、Model、Provider 如何关联？ → A: Provider 是独立的后端配置，不属于 Preset；Model 属于一个 Preset，并引用一个 Provider。关系：Preset 1→N Model，Provider 1→N Model。

### Session 2026-07-02

- Q: HiveGUI 是否需要实现实体间的关系管理（如 Agent↔Tool、Agent↔Skill、Tag→任意实体）？ → A: 基础 CRUD —— 每个实体独立增删改查，不实现关联管理（如 Agent 不分配 Tool/Skill，不打标签）。
- Q: HiveGUI 的实体字段是否完整对齐 hiveweb？ → A: 完整对齐 —— 复制 hiveweb 所有字段（包括 manifest、schema JSON、s3_key 等），确保数据模型一致性。
- Q: Plugin、Function、Workflow、Tool、Skill、Agent 的 identifier 字段是否必须唯一？ → A: 必须唯一 —— identifier 作为程序化标识符，数据库层面加 UNIQUE 约束。
- Q: Workflow 管理是否包含 WorkflowNode 和 WorkflowEdge 子实体的 CRUD？ → A: 仅主表 —— 只管理 Workflow 元数据（identifier, name, description, timeout_ms 等），不包含节点/边的结构化 CRUD。
- Q: Category 是否支持 parent_id 层级结构？ → A: 支持层级 —— 保留 parent_id 字段，支持树形结构（父分类/子分类），UI 用树形视图展示。

### Session 2026-07-02 (补充)

- Q: HiveGUI 是否需要错误恢复机制（自动重试、手动恢复）？ → A: 需要 —— 对于临时性错误（数据库锁定、文件占用）自动重试最多 3 次；提供手动恢复功能（从备份恢复数据）。
- Q: HiveGUI 是否需要数据导出/备份功能？ → A: 需要 —— 支持手动导出所有数据到 JSON 文件，支持从备份文件恢复数据。备份文件包含所有实体数据。
- Q: 应用升级时如何处理数据迁移？ → A: 需要版本管理 —— 数据库 schema 使用版本号管理，应用启动时检测版本差异，自动执行迁移脚本。支持向前兼容（新版本能读取旧数据）。

## 用户场景与测试 *(必填)*

### 用户故事 1 - 首页导航 (优先级: P1)

用户启动 HiveGUI 后看到首页，侧边栏提供工具入口，数据源管理位于工具页面中。

**独立测试**: 启动应用 → 默认显示首页 → 点击工具导航 → 进入工具页面并显示数据源管理。

**验收场景**:

1. **假设** HiveGUI 启动，**当** 用户进入应用时，**则** 显示首页。
2. **假设** 用户在首页，**当** 点击侧边栏"工具"图标时，**则** 切换到工具页面并显示数据源管理。
3. **假设** 用户在工具页面，**当** 点击"首页"图标时，**则** 返回首页。

---

### 用户故事 2 - 数据源管理 (优先级: P1)

管理员希望在 HiveGUI 桌面应用中管理远程 MySQL 数据源连接。他们打开数据源管理面板，可以添加、编辑、删除数据源配置，并测试连接。数据源配置持久化在本地。

**独立测试**: 添加数据源 → 测试连接 → 编辑 → 删除 → 重启应用验证数据持久化。

**验收场景**:

1. **假设** 数据源管理面板已打开，**当** 用户点击"添加"时，**则** 显示数据源表单（名称、主机、端口、用户名、密码）。
2. **假设** 用户在表单中填写了有效的连接信息，**当** 点击"连接测试"时，**则** 系统尝试连接并显示成功或失败。
3. **假设** 用户填写完表单，**当** 点击"保存"时，**则** 数据源被存储到本地并出现在列表中。
4. **假设** 存在一个数据源，**当** 用户编辑其配置并保存时，**则** 更新后的配置被持久化。
5. **假设** 存在一个数据源，**当** 用户删除并确认后，**则** 该数据源从列表移除。

---

### 用户故事 3 - 全局配置管理 (优先级: P1)

管理员希望在 HiveGUI 中管理全局配置项。他们打开全局配置面板，可以添加、编辑、删除键值对配置，支持搜索和分页浏览。配置数据持久化在本地 SQLite。

**独立测试**: 添加配置项 → 编辑 → 删除 → 搜索 → 分页浏览 → 重启验证持久化。

**验收场景**:

1. **假设** 全局配置面板已打开，**当** 用户点击"添加"时，**则** 显示配置表单（名称、键、类型、数据/值）。
2. **假设** 用户在表单中填写完整信息，**当** 点击"保存"时，**则** 配置项被存储到本地并出现在列表中。
3. **假设** 列表中有配置项，**当** 用户输入搜索关键词时，**则** 列表按名称或键过滤匹配项。
4. **假设** 存在一个配置项，**当** 用户编辑并保存时，**则** 更新后的配置被持久化。
5. **假设** 存在一个配置项，**当** 用户删除并确认后，**则** 该配置项从列表移除。

### 边界情况

- 数据库文件损坏或丢失时，系统应在启动时检测并提供重建选项。
- 多个实例并发访问本地数据库时，仅允许一个实例写入，第二个实例报错退出。
- 大型数据集应使用分页保持 UI 响应流畅。
- 密码字段允许为空（仅修改时表示"不修改密码"）。
- 全局配置的 key 必须唯一。
- Preset 名称必须唯一，最多一个 default=true。
- 删除 Preset 时级联删除关联 Model，不删除独立 Provider。
- API Key 界面输入后加密存储，不显示明文。
- Plugin、Function、Workflow、Tool、Skill、Agent 的 identifier 必须唯一（UNIQUE 约束）。
- Category 删除时需处理子分类：若存在子分类，**阻止删除**并提示"该分类下有 N 个子分类，请先删除子分类"。
- 删除 Category 时，引用该 Category 的实体的 category_id 置为 NULL（不级联删除实体）。
- Plugin 支持软删除（deleted_at 字段），列表默认过滤已删除记录。
- Capability 的 name 为主键，不可重复。
- Tag 的 name 必须唯一。
- Category 的 slug 必须唯一。
- Agent 删除时需处理 parent_agent_id 引用：若存在子 Agent，将子 Agent 的 `parent_agent_id` 置为 NULL（不级联删除子 Agent）。
- Tool 的 function_id 和 workflow_id 互斥（CHECK 约束：kind=1 时 function_id 非空，kind=2 时 workflow_id 非空）。
- Agent 的 parent_agent_id 不允许形成循环引用（如 A→B→A），保存时检测并提示错误。

### 字段验证规则

- **identifier**: 最大长度 255 字符，仅允许字母、数字、下划线、连字符（正则：`^[a-zA-Z0-9_-]+$`）
- **name**: 最大长度 255 字符，不允许为空
- **slug**: 最大长度 255 字符，仅允许小写字母、数字、连字符（正则：`^[a-z0-9-]+$`），手动输入
- **description**: 可选，最大长度 2000 字符
- **JSON 字段**（input_schema, output_schema, manifest, frontmatter, required_capabilities）: 必须是有效的 JSON 格式，最大大小 1MB
- **color**（Tag）: 可选，格式为 HEX 颜色代码（如 `#FF5733`）

### UI 交互规范

- 列表为空时，显示友好的空状态提示（如"暂无数据，点击添加按钮创建"）
- 删除操作必须弹出确认对话框，显示"确定要删除 [实体名称] 吗？此操作不可撤销。"
- 表单提交失败（如数据库写入失败、唯一性约束冲突）时，保留表单数据，显示错误消息，不关闭表单
- 搜索无结果时，显示"未找到匹配项"提示

### 加密密钥管理

- 加密密钥（chacha20poly1305）存储在本地配置文件中（如 `~/.hivegui/key`），文件权限设置为 600（仅所有者可读写）
- 首次启动时自动生成随机密钥并保存
- 密钥文件丢失时，已加密的数据（密码、API Key）无法解密，需重新配置


---

### 用户故事 4 - LLM 配置管理 (优先级: P1)

管理员希望在 HiveGUI 中配置 LLM 预设（Preset）、提供商（Provider）和模型（Model）。Provider 独立维护连接与凭据；Preset 通过一组有优先级的 Model 形成 fallback 链，每个 Model 引用一个 Provider。

**独立测试**: 创建独立 Provider → 创建 Preset → 添加引用该 Provider 的 Model → 编辑 → 删除 → 重启验证持久化。

**验收场景**:

**Model 管理**:
1. **假设** Model 管理面板已打开，**当** 用户点击"添加"时，**则** 显示 Model 表单（名称、Preset、Provider、优先级），保存后出现在所选 Preset 的列表中。
2. **假设** 存在一个 Model，**当** 用户删除时，**则** 该 Model 从对应 Preset 的列表移除。
3. **假设** 存在多个 Preset，**当** 用户切换 Preset 时，**则** 只显示属于该 Preset 的 Model。

**Provider 管理**:
4. **假设** Provider 管理面板已打开，**当** 用户点击"添加 Provider"时，**则** 显示独立 Provider 表单（kind 下拉、base_url、API key），不显示 Preset 或 Model 选择。
5. **假设** 存在一个 Provider，**当** 用户编辑并保存时，**则** 更新后的配置被持久化。
6. **假设** 存在一个未被 Model 引用的 Provider，**当** 用户删除时，**则** 该 Provider 从独立列表中移除；被 Model 引用时阻止删除。

**Preset 管理**:
7. **假设** Preset 管理面板已打开，**当** 用户点击"添加"时，**则** 显示 Preset 表单（名称、描述、默认标志、max_tokens、temperature）。
8. **假设** 用户创建 Preset 并标记为默认，**当** 已存在另一个默认 Preset 时，**则** 系统自动取消旧 Preset 的默认标志。

### 用户故事 5 - 标签管理 (优先级: P1)

管理员希望在 HiveGUI 中管理标签（Tag）。他们打开标签管理面板，可以添加、编辑、删除标签。标签支持名称和颜色。

**独立测试**: 添加标签 → 编辑颜色 → 删除 → 重启验证持久化。

**验收场景**:

1. **假设** 标签管理面板已打开，**当** 用户点击"添加"时，**则** 显示标签表单（名称、颜色），保存后出现在列表中。
2. **假设** 存在一个标签，**当** 用户编辑其名称或颜色并保存时，**则** 更新后的数据被持久化。
3. **假设** 存在一个标签，**当** 用户删除并确认后，**则** 该标签从列表移除。

---

### 用户故事 6 - 分类管理 (优先级: P1)

管理员希望在 HiveGUI 中管理分类（Category）。分类支持树形层级结构（父分类/子分类），UI 用树形视图展示。

**独立测试**: 添加父分类 → 添加子分类 → 编辑 → 删除子分类 → 删除父分类 → 重启验证持久化。

**验收场景**:

1. **假设** 分类管理面板已打开，**当** 用户点击"添加"时，**则** 显示分类表单（名称、slug、描述、父分类选择），保存后出现在树形列表中。
2. **假设** 存在一个父分类，**当** 用户添加子分类并选择该父分类时，**则** 子分类在树形视图中嵌套显示。
3. **假设** 存在一个分类，**当** 用户编辑并保存时，**则** 更新后的数据被持久化。
4. **假设** 存在一个分类，**当** 用户删除并确认后，**则** 该分类从列表移除。

---

### 用户故事 7 - 能力管理 (优先级: P1)

管理员希望在 HiveGUI 中管理能力（Capability）。能力包含名称、描述、危险标志和可选分类。

**独立测试**: 添加能力 → 编辑 → 删除 → 重启验证持久化。

**验收场景**:

1. **假设** 能力管理面板已打开，**当** 用户点击"添加"时，**则** 显示能力表单（名称、描述、是否危险、分类选择），保存后出现在列表中。
2. **假设** 存在一个能力，**当** 用户编辑并保存时，**则** 更新后的数据被持久化。
3. **假设** 存在一个能力，**当** 用户删除并确认后，**则** 该能力从列表移除。

---

### 用户故事 8 - 插件管理 (优先级: P1)

管理员希望在 HiveGUI 中管理插件（Plugin）。插件包含标识符、名称、版本、运行时、描述等完整字段。

**独立测试**: 添加插件 → 编辑 → 删除 → 重启验证持久化。

**验收场景**:

1. **假设** 插件管理面板已打开，**当** 用户点击"添加"时，**则** 显示插件表单（identifier, name, version, runtime, description, manifest, author, repository_url, s3_key, sha256, size_bytes, category_id），保存后出现在列表中。
2. **假设** 存在一个插件，**当** 用户编辑并保存时，**则** 更新后的数据被持久化。
3. **假设** 存在一个插件，**当** 用户删除并确认后，**则** 该插件从列表移除。
4. **假设** 用户尝试创建 identifier 重复的插件，**当** 保存时，**则** 系统提示 identifier 已存在并阻止保存。

---

### 用户故事 9 - 函数管理 (优先级: P1)

管理员希望在 HiveGUI 中管理函数（Function）。函数包含标识符、名称、kind（builtin/custom）、input/output schema、可选 plugin 关联和分类。

**独立测试**: 添加函数 → 编辑 → 删除 → 重启验证持久化。

**验收场景**:

1. **假设** 函数管理面板已打开，**当** 用户点击"添加"时，**则** 显示函数表单（identifier, name, kind, description, input_schema, output_schema, plugin_id, plugin_export, category_id, required_capabilities），保存后出现在列表中。
2. **假设** 存在一个函数，**当** 用户编辑并保存时，**则** 更新后的数据被持久化。
3. **假设** 存在一个函数，**当** 用户删除并确认后，**则** 该函数从列表移除。
4. **假设** 用户尝试创建 identifier 重复的函数，**当** 保存时，**则** 系统提示 identifier 已存在并阻止保存。

---

### 用户故事 10 - 工作流管理 (优先级: P1)

管理员希望在 HiveGUI 中管理工作流（Workflow）。仅管理主表元数据，不包含节点/边的可视化编辑。

**独立测试**: 添加工作流 → 编辑 → 删除 → 重启验证持久化。

**验收场景**:

1. **假设** 工作流管理面板已打开，**当** 用户点击"添加"时，**则** 显示工作流表单（identifier, name, description, timeout_ms, category_id, input_schema, start_description, output_schema, required_capabilities），保存后出现在列表中。
2. **假设** 存在一个工作流，**当** 用户编辑并保存时，**则** 更新后的数据被持久化。
3. **假设** 存在一个工作流，**当** 用户删除并确认后，**则** 该工作流从列表移除。
4. **假设** 用户尝试创建 identifier 重复的工作流，**当** 保存时，**则** 系统提示 identifier 已存在并阻止保存。

---

### 用户故事 11 - 工具管理 (优先级: P1)

管理员希望在 HiveGUI 中管理工具（Tool）。工具包含标识符、名称、kind（function-wrap/workflow-wrap）、source、is_always 等字段。

**独立测试**: 添加工具 → 编辑 → 删除 → 重启验证持久化。

**验收场景**:

1. **假设** 工具管理面板已打开，**当** 用户点击"添加"时，**则** 显示工具表单（identifier, name, description, kind, source, is_always, function_id, workflow_id, input_schema, output_schema, category_id, required_capabilities），保存后出现在列表中。
2. **假设** 存在一个工具，**当** 用户编辑并保存时，**则** 更新后的数据被持久化。
3. **假设** 存在一个工具，**当** 用户删除并确认后，**则** 该工具从列表移除。
4. **假设** 用户尝试创建 identifier 重复的工具，**当** 保存时，**则** 系统提示 identifier 已存在并阻止保存。

---

### 用户故事 12 - 技能管理 (优先级: P1)

管理员希望在 HiveGUI 中管理技能（Skill）。技能包含标识符、名称、描述、frontmatter、content（markdown）、source、is_always 等字段。

**独立测试**: 添加技能 → 编辑 → 删除 → 重启验证持久化。

**验收场景**:

1. **假设** 技能管理面板已打开，**当** 用户点击"添加"时，**则** 显示技能表单（identifier, name, description, frontmatter, content, source, is_always, category_id, required_capabilities），保存后出现在列表中。
2. **假设** 存在一个技能，**当** 用户编辑并保存时，**则** 更新后的数据被持久化。
3. **假设** 存在一个技能，**当** 用户删除并确认后，**则** 该技能从列表移除。
4. **假设** 用户尝试创建 identifier 重复的技能，**当** 保存时，**则** 系统提示 identifier 已存在并阻止保存。

---

### 用户故事 13 - Agent 管理 (优先级: P1)

管理员希望在 HiveGUI 中管理 Agent。Agent 包含标识符、名称、描述、system_prompt、parent_agent_id、depth、model_preset 等字段。

**独立测试**: 添加 Agent → 编辑 → 删除 → 重启验证持久化。

**验收场景**:

1. **假设** Agent 管理面板已打开，**当** 用户点击"添加"时，**则** 显示 Agent 表单（identifier, name, description, system_prompt, parent_agent_id, depth, model_preset），保存后出现在列表中。
2. **假设** 存在一个 Agent，**当** 用户编辑并保存时，**则** 更新后的数据被持久化。
3. **假设** 存在一个 Agent，**当** 用户删除并确认后，**则** 该 Agent 从列表移除。
4. **假设** 用户尝试创建 identifier 重复的 Agent，**当** 保存时，**则** 系统提示 identifier 已存在并阻止保存。

---

## 需求 *(必填)*

### 功能需求

- **FR-001**: 系统必须提供首页视图，显示应用标题和导航入口。
- **FR-002**: 系统必须提供数据源管理界面，支持添加、编辑、删除远程 MySQL 数据源配置。
- **FR-003**: 系统必须支持测试数据源连接（MySQL），并显示连接测试结果。
- **FR-004**: 数据源配置（名称、主机、端口、用户名、加密密码）必须持久化到本地 SQLite 数据库。
- **FR-005**: 后台任务（连接测试、保存）异步执行，不阻塞 GUI 线程。
- **FR-006**: 系统必须在启动时执行数据库完整性检查，损坏时提供恢复选项。
- **FR-007**: 系统必须提供全局配置管理界面，支持添加、编辑、删除全局配置项（name, key, type, data），支持搜索和分页。
- **FR-008**: 全局配置数据必须持久化到本地 SQLite 数据库，key 唯一。
- **FR-009**: 系统必须提供 Model 管理界面，支持添加、编辑、删除 Model（name, preset_id, provider_id, priority）；Model 属于一个 Preset，并引用一个 Provider。
- **FR-010**: 系统必须提供独立 Provider 管理界面，支持添加、编辑、删除 Provider（kind, base_url, api_key），不按 Preset 分组；被 Model 引用的 Provider 不可删除。
- **FR-011**: 系统必须提供 Preset 管理界面，支持添加、编辑、删除 Preset（name, description, is_default, max_tokens, temperature），删除时级联删除关联 Model，不删除 Provider。
- **FR-012**: API Key 加密存储（chacha20poly1305），界面显示为遮蔽形式。
- **FR-013**: Preset 的 Model 列表按 priority 排序，作为 fallback 链。
- **FR-014**: 系统必须提供标签（Tag）管理界面，支持添加、编辑、删除标签（name, color）。
- **FR-015**: 系统必须提供分类（Category）管理界面，支持添加、编辑、删除分类（name, slug, description, parent_id），UI 以树形视图展示层级结构。
- **FR-016**: 系统必须提供能力（Capability）管理界面，支持添加、编辑、删除能力（name, description, is_dangerous, category_id）。
- **FR-017**: 系统必须提供插件（Plugin）管理界面，支持添加、编辑、删除插件，字段完整对齐 hiveweb（identifier, name, description, manifest, runtime, version, author, repository_url, s3_key, sha256, size_bytes, category_id）。identifier 必须唯一。
- **FR-018**: 系统必须提供函数（Function）管理界面，支持添加、编辑、删除函数，字段完整对齐 hiveweb（identifier, name, kind, description, input_schema, output_schema, plugin_id, plugin_export, category_id, required_capabilities）。identifier 必须唯一。
- **FR-019**: 系统必须提供工作流（Workflow）管理界面，支持添加、编辑、删除工作流主表（identifier, name, description, timeout_ms, category_id, input_schema, start_description, output_schema, required_capabilities）。不包含 WorkflowNode/WorkflowEdge 子实体的 CRUD。identifier 必须唯一。
- **FR-020**: 系统必须提供工具（Tool）管理界面，支持添加、编辑、删除工具，字段完整对齐 hiveweb（identifier, name, description, kind, source, is_always, function_id, workflow_id, input_schema, output_schema, category_id, required_capabilities）。identifier 必须唯一。
- **FR-021**: 系统必须提供技能（Skill）管理界面，支持添加、编辑、删除技能，字段完整对齐 hiveweb（identifier, name, description, frontmatter, content, source, is_always, category_id, required_capabilities）。identifier 必须唯一。
- **FR-022**: 系统必须提供 Agent 管理界面，支持添加、编辑、删除 Agent（identifier, name, description, system_prompt, parent_agent_id, depth, model_preset）。identifier 必须唯一。
- **FR-023**: 所有新增实体的 CRUD 操作仅做独立增删改查，不实现实体间的关联管理（如 Agent 不分配 Tool/Skill，不打标签）。
- **FR-024**: 所有新增实体的列表必须支持分页（每页 20 条）和搜索。搜索范围为实体的 `name` 和 `identifier` 字段（如适用），采用模糊匹配（LIKE '%keyword%'），不区分大小写。
- **FR-025**: 系统必须提供错误恢复机制。对于临时性错误（数据库锁定、文件占用），自动重试最多 3 次，每次间隔递增（1s, 2s, 4s）。提供手动恢复功能，支持从备份文件恢复数据。
- **FR-026**: 系统必须提供数据导出/备份功能。支持手动导出所有实体数据到 JSON 文件，支持从备份文件恢复数据。备份文件包含所有实体（Tag、Category、Capability、Plugin、Function、Workflow、Tool、Skill、Agent、DataSource、GlobalConfig、LLM 配置）的完整数据。
- **FR-027**: 系统必须支持数据库 schema 版本管理。应用启动时检测数据库版本号，若版本不匹配则自动执行迁移脚本。支持向前兼容（新版本能读取旧数据）。迁移失败时提供回滚选项。


### 关键实体

- **数据源配置（DataSource）**: 远程 MySQL 连接配置，包含名称、主机地址、端口号、用户名、加密密码。存储在本地 SQLite。
- **全局配置（GlobalConfig）**: 应用级配置项，包含名称（name）、键（key）、类型（type）、数据值（data）。存储在本地 SQLite。key 全局唯一，支持分页和搜索。
- **Model**: Preset 内的模型条目，引用一个 Provider。字段：id, name, preset_id (FK), provider_id (FK), priority, created_at, updated_at。
- **LLM Provider**: 独立的 LLM 后端连接配置。字段：id, kind, base_url, api_key_encrypted, api_key_env, created_at, updated_at。
- **LLM Preset**: 顶层配置包，包含一组按优先级排序的 Model（fallback 链）和生成参数。字段：id, name (UNIQUE), description, is_default, max_tokens, temperature, created_at, updated_at。
- **Tag（标签）**: 字段：id, name, color (可选), created_at。
- **Category（分类）**: 字段：id, parent_id (可选 FK→自身), name, slug, description (可选), created_at, updated_at。支持树形层级。
- **Capability（能力）**: 字段：name (PK), description, is_dangerous, category_id (可选 FK→Category), created_at。
- **Plugin（插件）**: 字段：id, identifier (UNIQUE), name, description (可选), manifest (可选 JSON), runtime, version, author (可选), repository_url (可选), s3_key, sha256, size_bytes, category_id (可选 FK→Category), created_at, updated_at, deleted_at (可选)。
- **Function（函数）**: 字段：id, identifier (UNIQUE), name, description (可选), kind (1=builtin, 2=custom), input_schema (JSON), output_schema (JSON), plugin_id (可选 FK→Plugin), plugin_export (可选), category_id (可选 FK→Category), required_capabilities (可选 JSON), created_at, updated_at。
- **Workflow（工作流）**: 字段：id, identifier (UNIQUE), name, description (可选), timeout_ms, category_id (可选 FK→Category), input_schema (可选 JSON), start_description (可选), output_schema (可选 JSON), required_capabilities (可选 JSON), created_at, updated_at。不包含 WorkflowNode/WorkflowEdge 子表。
- **Tool（工具）**: 字段：id, identifier (UNIQUE), name, description, kind (1=function-wrap, 2=workflow-wrap), source (workspace|builtin), is_always, function_id (可选 FK→Function), workflow_id (可选 FK→Workflow), input_schema (JSON), output_schema (JSON), category_id (可选 FK→Category), required_capabilities (可选 JSON), created_at, updated_at。
- **Skill（技能）**: 字段：id, identifier (UNIQUE), name, description, frontmatter (可选 JSON), content (markdown text), source, is_always, category_id (可选 FK→Category), required_capabilities (可选 JSON), created_at, updated_at。
- **Agent**: 字段：id, identifier (UNIQUE), name, description (可选), system_prompt, parent_agent_id (可选 FK→自身), depth, model_preset (可选), created_at, updated_at。


## 成功标准 *(必填)*

### 可衡量的成果

- **SC-001**: 用户启动 HiveGUI 后可直接看到首页，无需任何外部服务器配置。
- **SC-002**: 数据源增删改查操作在 1 秒内完成并持久化到本地。
- **SC-003**: 连接测试在 5 秒内返回结果（超时视为连接失败）。
- **SC-004**: 敏感数据（密码）不以明文形式存储。
- **SC-005**: GUI 在后台操作期间保持响应。
- **SC-006**: 全局配置支持每页 20 条分页，搜索和翻页在 500ms 内响应。
- **SC-007**: LLM 配置（Preset/Provider/Model）CRUD 操作在 1 秒内完成并持久化。
- **SC-008**: 新增实体（Tag、Category、Capability、Plugin、Function、Workflow、Tool、Skill、Agent）的 CRUD 操作在 1 秒内完成并持久化。
- **SC-009**: 所有新增实体的列表支持分页（每页 20 条），搜索和翻页在 500ms 内响应。
- **SC-010**: Category 树形视图渲染在 200ms 内完成（即使有 100+ 分类）。
- **SC-011**: identifier 唯一性约束在数据库层面强制执行，重复时立即提示错误。

## 假设条件

- HiveGUI 是面向管理员的桌面工具，运行在 Linux/macOS/Windows 桌面端。
- 用户自行管理目标 MySQL 服务器的网络可达性。
- HiveGUI 不连接任何远程 hiveclaw/hiveweb 服务器，是纯本地桌面应用。
- 界面仅支持中文。
- 数据源密码使用本地加密存储。
