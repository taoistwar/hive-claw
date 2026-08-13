# 功能规格说明书: HiveGUI 独立本地 Agent

**功能分支**: `260517-hivegui-standalone-mode`
**创建日期**: 2026-06-15
**状态**: 草案
**最后更新**: 2026-07-29（本地主密码认证 + 历史 Clarifications 统一压缩）
**输入**: 用户描述: "将 hivegui 变为独立运行的桌面数据源管理工具，不依赖远程 hiveclaw/hiveweb 服务器。"；后续澄清: "hiveweb 是运行在云端的 Agent，hivegui 是运行在本地的 Agent。它们是独立的，只是代码复用，hivegui 不会请求 hiveweb。"

## Clarifications

> 本节只保留**当前生效**的决策。历史"已废弃/已批准/已变更/已更新/已细化" 注解已统一清除。**不保留逐条历史回放**；当前决策按主题归并为下方 Decision Log 表格，最新一期 Session（2026-07-29）保留全文。如需回溯过程请自行查阅 git history。

### Decision Log (按主题归并)

| 主题 | 当前决策 | 涉及 FR/任务 |
|---|---|---|
| 数据库 | hiveweb 用 MySQL，hivegui 用 SQLite；HiveGUI 可连接用户配置的外部 MySQL 数据源，**不**构成对 HiveWeb 的请求/依赖 | FR-001、FR-007、FR-048 |
| 部署边界 | HiveWeb 云端 Agent，HiveGUI 本地 Agent，代码复用但不通信 | 全局 |
| 数据迁移 | 不提供自动迁移；用户手动在 HiveGUI 重建配置 | US-Setup |
| 远程 MySQL 数据源 | 默认隐藏的高级功能，与核心隔离 | US-DS |
| WASM 插件兼容 | 共享 Extism ABI + manifest schema + host_call；缺 Capability 时拒绝导入 | FR-021、US-Plugin |
| 内置函数 | 仅 4 个纯函数：`format_template` / `json_parse` / `json_stringify` / `text_regex_match`；唯一有效 identifier 为下划线名称，点号别名（`format.template` 等）不得注册/查询/执行；受信迁移可重命名旧记录，发生目标碰撞时整体回滚 | FR-008、FR-024 |
| Builtin/Tool 分类 | Function: `builtin|custom|placeholder`；Tool: `function-wrap|workflow-wrap` | FR-008、FR-018 |
| WorkflowNode 节点类型 | 共享 HiveWeb 契约：`start_node|end_node|function_node|generate_answer_node` | FR-018 |
| 关系管理 | 完整本地 Agent：按 Agent 显式分配 Tool/Skill/Capability；`is_always=true` 自动可用 | FR-013、US-Agent |
| 实体字段 | 完整对齐 HiveWeb（含 manifest/schema JSON/s3_key） | FR-013~FR-020 |
| identifier 唯一 | 全部实体 `identifier` 字段 DB 层 UNIQUE | FR-013~FR-020 |
| Workflow 编辑 | 完整 DAG：WorkflowNode/WorkflowEdge 本地管理、可视化编辑、校验、执行 | FR-018、US-Workflow |
| Category 树 | 支持 `parent_id` 层级；单次批量查询加载完整树；搜索保留祖先路径 | FR-014、US-Category |
| 错误恢复 | 临时错误自动重试 ≤ 3 次；提供从备份恢复 | FR-040、US-Backup |
| 备份 | 单一可移植包（版本化 JSON 清单 + 全部托管 WASM + 校验值），事务式恢复；用户口令对包认证加密，跨设备时用目标设备密钥重新加密；恢复前预验证影响 + 计划安全备份位置，用户确认后冻结写入→安全备份→完整替换 | FR-026、US-Backup |
| 升级/迁移 | 仅顺序向前；支持最近 2 个旧 schema/备份格式版本；升级前自动验证安全快照，失败回滚并阻止主 UI；不支持降级 | FR-040、SC-027 |
| 子 Agent 路由 | 唯一默认根 Agent；LLM 仅可路由到直接子 Agent；最大深度 10；拒绝循环 | FR-013、US-Agent |
| 跨设备恢复 | 用户口令对包认证加密；恢复时用目标设备密钥重新加密 | FR-026、SC-029 |
| 恢复前确认 | 预验证展示影响 + 计划安全备份位置；用户确认后冻结写入→安全备份→完整替换 | FR-026、SC-030 |
| Workflow 失败 | Fail-fast；停止未开始节点；保留节点级诊断；不回滚已执行外部副作用 | FR-018、SC-031 |
| Rust 工具链 | 精确固定为查询当日最新稳定版（当前 1.97.1），本地与 CI 一致 | T001、SC-007 |
| SQLx feature | HiveGUI `chrono|macros|runtime-tokio|sqlite`；HiveWeb `chrono|json|macros|mysql|runtime-tokio|rust_decimal|tls-rustls-ring-webpki`；两端不重复 `derive` | FR-048、T017A/T017G |
| MySQL TLS | 仅 `VERIFY_IDENTITY` + 显式 CA/hostname；任何降级/不可用 fail-closed | FR-048、T017B |
| SQLx 动态 SQL | 生产 `QueryBuilder=0`；固定 schema 用 checked macros；有限变体用封闭 enum/`match`；HiveGUI 外部 MySQL 用 `MysqlIdentifier` allowlist；唯一生产 `AssertSqlSafe` = HiveWeb reviewed `named_queries.toml` 中央边界 | FR-048、T017G |
| 7 个 advisory | 全部立即无 ignore 修复（方案 A） | T017A/T017B/T017G |
| `game_service.category_name` | checked static `JSON_OBJECT('name', ?)` + bind；用户值只经 bind | T017B/T017B' |
| T001 独立批次 | 独立分支 `codex/rust-1.97.1-toolchain` 提交 `bf3690d` 已推送；T001 在 PR/CI 一致性证据闭合前保持未完成；T017D 不得把"已推送分支" 冒充"PR/CI 已闭合" | T001 |
| Builtin/Tool 边界 | 消息回复和子 Agent 路由属于运行时控制能力，不进入 Tool CRUD；其他 Tool 用户显式创建分配 | FR-013、US-Tool |
| 性能预算 | 编排/路由 p95 ≤ 200ms；Tool 分派 p95 ≤ 50ms；100 节点 Workflow 调度 p95 ≤ 100ms；外部 LLM/网络/用户代码不计入本地预算 | FR-029、SC-016 |
| 运行日志 | 脱敏结构化日志，execution_id 贯穿全栈；滚动 7×24h 或 100MB；支持导出脱敏诊断包；不默认保存完整输入输出 | FR-031、SC-024 |
| GUI 响应 | 输入到反馈 p95 ≤ 100ms；主 UI 线程阻塞 ≤ 250ms；取消控件始终可操作 | SC-021 |
| 无障碍/键盘 | 全关键流程键盘可达；DAG 节点/连线键盘操作；焦点陷阱 + 焦点恢复；4.5:1 对比度；状态不依赖颜色 | SC-020 |
| Plugin 资源限制 | 默认 30s/128MiB/10MiB，硬上限 120s/512MiB/50MiB；`memory_limit_mb` 仅兼容字段名 | FR-021、SC-018 |
| 会话/消息 | 设备本地密钥加密；默认 100 年保留；可调；进入加密备份包，不进诊断日志/包 | FR-019、SC-019 |
| 取消协作 | Agent/LLM/Tool/Workflow 协作取消；不再调度新节点；可取消网络请求；Plugin 2s 后强终止；状态持久化 `cancelled` | FR-022、SC-022 |
| 数据库故障恢复 | 不存在→新建 v4；迁移失败→保留快照，可重试/退出；完整性损坏→阻止主 UI，可恢复/重建/退出（重建前二次确认） | FR-040、SC-025 |
| 入口 Agent | 正式会话只允许唯一默认根 Agent；指定入口只允许于不导出的测试 helper | FR-013 |
| Category 渲染 | 树用单次批量查询；保留匹配节点的祖先路径；禁止 N+1 | SC-013 |
| 性能证据 | 固定数据量/输入/样本数/计时边界；p50/p95/p99；连接超时用可控时钟验证 5s | SC-008 |
| 跨版本迁移 | 仅向前 + 最近 2 个旧版本；升级前自动验证安全快照；失败回滚并阻断主 UI；不支持降级 | FR-040、SC-027 |
| 屏幕锁事件 | Linux `zbus` `org.freedesktop.ScreenSaver ActiveChanged`、macOS `NSWorkspaceScreensDidSleep`、Windows `WM_WTSSESSION_CHANGE=0x7` | FR-050、T-AUTH-3 |
| 主密码 Argon2id 参数 | m=64MiB, t=3, p=1；派生耗时 < 5000ms 超时 fail-closed | FR-049、T-AUTH-1 |
| i18n | 仅中文 | FR-005 |
| LLM Preset/Model/Provider 关系 | Provider 独立后端；Model 属于 Preset 并引用 Provider；Preset 1→N Model，Provider 1→N Model | FR-011 |
| global_configs schema | 完整复制 HiveWeb `global_configs` 表结构（id, name, key, type, data, created_at, updated_at），支持分页和搜索 | FR-007 |
| 本地 Plugin WASM 存储 | 导入时复制到本地插件目录，DB 保存相对制品键，执行前校验 SHA-256 | FR-021 |

### Session 2026-07-29 (Phase 1A Local Master-Password Authentication 激活)

- Q: 是否恢复 Session 2026-06-15 中标记为"已废弃" 的本地主密码认证？ → A: 是。决定保留并实现首次启动设置主密码 + 后续启动输入主密码解锁的本地认证流程；其工程边界由新增 FR-049（首次设置 / 解锁）、FR-050（自动锁定）、FR-051（无密码重置 / 仅从备份恢复）闭合。FR-012 设备本地密钥、FR-026 备份加密口令与本主密码认证均需独立 security review 并闭合于 `checklists/security.md`。
- Q: 会话自动锁定策略？ → A: 默认空闲 15 分钟后自动锁定，可在 FR-007 `GlobalConfig` 字段中按 `1..=1440` 分钟调整；操作系统屏幕锁或锁屏事件必须立即触发锁定。锁定后所有已派生密钥与未持久化敏感明文必须从内存清零，UI 回到解锁界面。**该行为对"主 UI 始终可见" 的 SC-001/FR-001 引入受控例外** —— 启动后到首次解锁前主 UI 不可见；锁定后再解锁也受相同约束。
- Q: 忘记主密码时如何恢复？ → A: 不提供任何密码重置、找回、旁路或安全问题流程。唯一可恢复路径是 FR-026/T129/T130 的可移植备份恢复。首次设置主密码时必须强制用户在 UI 看到"忘记主密码 = 只能从备份恢复" 风险说明并显式勾选确认；已存在历史 T129 备份时 UI 展示该备份的 SHA-256/创建时间并要求确认；不存在时必须立即引导用户跳到 T129 备份向导并完成首次导出后，才允许回到主 UI。备份恢复后用户必须按 FR-049 重新设置主密码并重新包装设备密钥材料；该过程必须独立 security review 并记录在 `checklists/security.md` 的"无密码重置" 边界。

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

- 数据库文件不存在时，系统必须创建全新 v4 数据库；不得把首次启动视为损坏。新库建成后必须在进入主界面前执行统一 SQLite 健康检查。
- 每次打开已有数据库、创建新数据库以及迁移事务提交前，都必须执行 `PRAGMA integrity_check` 和 `PRAGMA foreign_key_check`；前者必须精确返回唯一的 `ok`，后者必须返回零行。`PRAGMA foreign_keys=ON` 只负责连接期约束执行，不能替代这两项检查。
- 已有数据库在迁移前的任一健康检查失败都属于完整性损坏，系统必须阻止主界面并提供“从备份恢复”“保留损坏文件并重建”和“退出”。重建前必须二次确认数据丢失风险，并把损坏文件复制到带时间戳的隔离位置；未确认前不得修改数据库或托管 Plugin。迁移目标在提交前检查失败则属于迁移失败，必须回滚迁移并保留安全快照。
- schema 迁移失败时不得提供重建捷径，只能保留安全快照并显示“重试迁移”和“退出”。
- 多个实例并发访问本地数据库时，仅允许一个实例写入，第二个实例报错退出。
- 大型数据集应使用分页保持 UI 响应流畅。
- SQLite 运行时不提供 FTS5 trigram tokenizer 时，启动、新建 schema 和迁移必须 fail-closed 并显示不兼容错误；不得回退到前导通配 `LIKE`、业务表全表扫描或内存扫描。
- 搜索规范化算法标识固定为 `hivegui-nfkc-casefold-v1`，其精确定义是 Unicode Standard 17.0.0 Default Case Algorithms 的 R5 `toNFKC_Casefold`：按 Unicode 17.0.0 `DerivedNormalizationProps.txt` 的 `NFKC_CF` 属性逐 Unicode 标量映射（缺失项映射自身），拼接后再按 Unicode 17.0.0 规范化为 NFC；不得用 `lowercase`、简单 case fold 或 `casefold(NFKC(input))` 近似替代。该 ID 作为搜索索引格式元数据持久化。应用支持的 ID、Unicode 数据版本、映射来源校验值或 golden fixture 与数据库不一致时必须通过显式 schema 迁移分配新 ID、重建并验证全部派生搜索索引，不能静默沿用或按新版规则查询旧索引。原始非空搜索词若规范化后为空，公开边界必须返回 `invalid_input { field: "search", reason: "empty_after_normalization" }`，不得把它解释为无筛选或匹配全部。
- 密码字段允许为空（仅修改时表示"不修改密码"）。
- 全局配置的 key 必须唯一。
- Preset 名称必须唯一，最多一个 default=true。
- 删除 Preset 时级联删除关联 Model，不删除独立 Provider；若任一 Agent.model_preset 引用该 Preset 名称则阻止删除并列出引用 Agent。重命名 Preset 必须在同一事务中更新全部 Agent.model_preset；Agent 保存非空 model_preset 时必须引用现有 Preset。
- API Key 界面输入后加密存储，不显示明文。
- Plugin、Function、Workflow、Tool、Skill、Agent 的 identifier 必须唯一（UNIQUE 约束）。
- Category 删除时需处理子分类：若存在子分类，**阻止删除**并提示"该分类下有 N 个子分类，请先删除子分类"。
- 删除 Category 时，引用该 Category 的实体的 category_id 置为 NULL（不级联删除实体）。
- Plugin 支持软删除（deleted_at 字段），列表默认过滤已删除记录。
- Plugin 导入成功后不依赖用户选择的原始 WASM 路径；原文件被移动或删除不影响已导入 Plugin。
- Plugin 的本地相对制品键不得包含绝对路径、`..` 或其他可逃逸 HiveGUI 插件目录的路径片段。所有由制品键或归档条目派生的导入、加载、执行、备份和恢复路径都必须从已打开的受控根目录句柄逐段解析且禁止跟随链接；任一中间段或最终目标为 Unix symlink、hardlink、Windows junction/reparse point、device、FIFO、socket 或其他非普通文件/重定向链接时必须拒绝。最终 WASM 必须是 link count 为 1 的普通文件。新导入只能以同目录原子 no-replace 发布，并发创建任何普通文件也必须返回冲突而不得覆盖；更新必须发布到新的不可变唯一制品键，再以数据库事务原子切换引用，旧对象仅在确认无引用且继续使用同一受控句柄时清理。仅做字符串规范化或先 canonicalize 再按路径重新打开不满足要求，检查与使用之间不得存在可替换路径的 TOCTOU 窗口。
- Plugin 的托管 WASM 文件缺失或 SHA-256 不匹配时，必须在加载和执行前拒绝运行并提示重新导入，不得从 HiveWeb 或隐式网络位置补取。
- Plugin manifest 必须声明 ABI 版本和所需 Capability。ABI 版本不受支持、manifest 不符合共享 schema 或目标 HiveGUI 缺少任一 Capability 时，必须在复制制品和写入 Plugin 记录前拒绝导入，并列出全部不兼容项。
- Plugin 可移植性仅覆盖共享 ABI 与受支持 Capability 的行为，不保证 HiveWeb/HiveGUI 的外部数据、凭据、配置或产品专属 Capability 相同；兼容检查不得请求 HiveWeb。
- Plugin 每次调用默认限制为 30 秒、128MiB 内存和 10MiB 输出；用户可按 Plugin 覆盖，但配置必须大于 0 且不得超过 120 秒、512MiB 和 50MiB。`memory_limit_mb` 的值按 MiB 解释，转换为 64KiB WASM page 时使用 `memory_limit_mb × 16`。达到任一限制时必须立即终止该调用，不得影响 HiveGUI 主 UI 或后续 Agent 会话。
- Plugin 默认不得直接获得 WASI 文件系统、socket/网络、环境变量或宿主进程能力；这些资源只能通过当前 Agent 已授权且目标端支持的 Capability 访问。提高资源限制不得放宽 Capability 或 WASI 隔离。
- Plugin 空闲实例池全局最多保留 8 个实例、每个完整 cache key 最多 1 个实例，并按 LRU 淘汰。cache key 必须包含制品 SHA-256、ABI 版本、runtime 及其版本、fuel、timeout、memory、output 和 Capability policy hash；制品替换、配置或授权策略修改、软删除、取消、timeout、trap、memory/output 超限或 host error 后必须立即淘汰对应实例，只有成功完成且健康的实例可回池。
- ChatSession、ChatMessage、Tool 调用参数/结果和 Agent 执行状态必须使用设备本地密钥加密持久化，默认保留 100 年。到期清理、删除单个会话和清空全部历史必须同时删除关联消息与执行状态。
- 用户缩短保留期或清空历史前，界面必须显示将删除的会话数量并要求确认；清理不得删除 Agent、Tool、Workflow、Plugin 等配置。会话内容只能进入整体认证加密的数据备份包，不得进入结构化运行日志或脱敏诊断包。
- 用户停止 Agent 执行后，取消信号必须传播到当前 Agent、子 Agent、LLM 请求、Tool 和 Workflow；不得再调度新的 Tool、子 Agent 或 Workflow 节点。可取消的网络请求必须取消，Plugin 应先获得最多 2 秒的协作终止时间，超时后仅强制终止该 Plugin 实例。
- 停止操作不得声称回滚已完成的文件、网络或数据库等外部副作用；最终结果必须标记为 cancelled，并区分停止前已完成、取消中断和未开始的步骤。取消过程或 Plugin 强制终止不得使 HiveGUI 主 UI、其他会话或本地运行时退出。
- 本地性能预算只度量 HiveGUI 可控开销：LLM 决策解析到下一本地动作被调度、Tool 调用验证完成到执行器启动、以及固定 100 节点 no-op DAG 的调度。外部 LLM、网络等待和用户 Function/Plugin 实际执行时间必须单独分段，不得混入本地预算或用来掩盖本地超时。每个 benchmark 必须绑定版本化基线、环境指纹和固定 fixture；已有路径在实现前采集基线，新 benchmark 以首个获批 Green 结果建立基线。任一 p50、p95 或 p99 相对基线回归超过 10% 时必须阻断发布，除非取得明确签字并记录理由、范围和到期复核日期。
- 每次本地 Agent 对话必须生成唯一 execution_id，并传播到子 Agent 路由、LLM、Tool、Function、Workflow 节点、Plugin 和 Capability 调用日志。每条 v1 记录必须包含操作名、实体 identifier、结果、稳定错误类别、耗时及至多 512 UTF-8 bytes 的 `cause_summary: string|null`；cause 摘要必须在唯一处理边界中央脱敏，原始 cause 不得持久化。记录不得包含密码、API Key、备份口令或默认保存完整 prompt、模型响应、Tool 输入输出。
- 结构化日志必须先写入 `.open` 活动段，每条记录必须是完整换行结尾的 JSON。滚动时必须依次 flush、fsync 活动文件，原子重命名为不可变 `.jsonl` 段，再 fsync 父目录；基于可注入 UTC 时钟的时间轮转或完成段逐记录重写必须保证任何可见记录自其 `occurred_at` 起实际不超过 7×24 小时，不能用段内最大时间近似；实际文件总量也不得超过 100,000,000 bytes，任一上限先到即清理。活动段不能无限期豁免时间上限：到达最早记录的到期边界前必须在完整记录边界轮转，并只把仍未过期的完整记录发布到新段。启动恢复只能丢弃活动段末尾不完整的单条记录，诊断读取不得观察半写记录。用户导出的诊断包只能包含同样脱敏的日志和非敏感环境信息，不得自动加入数据备份包。
- Agent、LLM、Tool、Workflow、Plugin、备份和恢复预验证等长任务运行时不得占用主 UI 线程。用户输入必须在 p95 100ms 内获得可见反馈，主 UI 线程不得连续阻塞超过 250ms，任务取消/停止控件必须持续可聚焦和可触发。
- 备份包中的每个托管 WASM 必须由清单记录相对制品键、大小和 SHA-256；导出时发现制品缺失或不匹配必须使整个备份失败，不得生成不完整备份。最终目标必须尚不存在；同名目标或并发创建冲突必须要求用户选择新名称，不得覆盖。导出必须在目标目录创建唯一 staging 文件，完整写入后 flush、fsync，以 no-replace 原子重命名为最终文件并 fsync 父目录；全部步骤成功前不得报告有效的最终备份。
- 恢复必须先在隔离暂存区验证包版本、JSON 结构、所有相对路径和全部制品校验值；归档内的 symlink、hardlink、device、FIFO、socket 和任何可逃逸路径，以及暂存目标中预先存在的 symlink、junction、reparse point 或其他特殊文件，均必须拒绝。任一验证或写入失败时不得改变当前数据库和插件目录。
- 备份包不得包含设备本地加密密钥。导出必须要求用户提供备份口令并对整个包进行认证加密。恢复可以流式处理已经通过分块认证的内容，但在读取并成功验证认证流结尾、完成全部格式和制品预检前，不得向 UI、日志、现有数据库或未加密暂存文件释放敏感明文。敏感字段解析后必须立即使用目标设备密钥重新加密到隔离 staging 数据库；错误口令、后段篡改或认证失败时必须保持 current 不变，并在 instance 仍为 unarmed 且 owner 双槽均无时通过 outcome=`aborted_pre_switch` retirement 整目录接管和清理，任何 ownership/identity 歧义都保留并 fail-closed。
- 跨设备恢复时，敏感值必须仅在受控恢复流程中解密，并在写入目标数据库前使用目标设备的本地密钥重新加密；不得把敏感明文写入日志、临时 JSON 或未加密暂存文件。
- 恢复不执行 identifier、ID 或 updated_at 级别的数据合并。目标 HiveGUI 存在数据时，预验证阶段只向用户展示完整替换影响；用户明确确认后必须立即冻结新写入，并在同一冻结周期内先完成当前数据库与 staging 数据库的非 busy checkpoint、关闭连接和 sidecar 验证，再从封闭的当前状态生成并验证安全备份，随后才可完整替换数据库和托管 Plugin 制品。等待确认期间允许发生的写入必须进入该确认后生成的安全备份；用户取消时不创建替换提交点。
- 恢复应用前必须自底向上 fsync 暂存数据库、制品文件及其目录，写入并 fsync 可重放的切换 owner，再通过同一文件系统内的原子 rename/swap 切换数据库与制品并 fsync 所有受影响的父目录。自动安全备份失败、用户取消，或 owner 仍为 `prepared|applying` 时任一步失败，必须恢复并验证完整旧状态；只有新 current 已通过完整 health/search/artifact/identity 验证后才可发布 `committed`，它是唯一系统提交点。`committed` 后仍须保持业务写闸门关闭，只能通过 registry retirement 幂等收口已经验证的新状态，禁止回滚到旧状态；retirement 全部完成后才可重新开放 Store。启动恢复必须先重放 retirement/tombstone，再按 owner 收敛为完整旧状态或完整新状态，安全备份仍保留供手工恢复。
- 启动发现数据库 schema 比应用更新时必须拒绝打开，不得尝试降级或忽略未知结构。schema 旧 1–2 个版本时按版本逐步迁移；更旧版本必须拒绝自动迁移并提示先使用中间版本升级。
- schema 迁移前必须先对源数据库执行统一 SQLite 健康检查，再创建并验证数据库与托管 Plugin 制品的安全快照。全部迁移步骤完成后、事务提交前必须再次对目标数据库执行相同检查；任一迁移或目标检查失败时必须回滚整个迁移事务、保留快照并阻止进入主界面，不得在部分迁移状态下继续运行。
- 任何 SQLite 主文件快照、迁移后发布、自动安全备份或恢复切换都必须先冻结新写入，要求 `wal_checkpoint(TRUNCATE)` 成功且不 busy，关闭全部数据库连接并验证没有可参与恢复的 `-wal`、`-shm` 或 rollback-journal sidecar，再 fsync 主文件与父目录。已提交但未 checkpoint 的 WAL 必须先完整 checkpoint；checkpoint/连接/sidecar 异常必须先完整分类，再按 local-runtime 合同的合法 reason/artifact 配对和固定总优先级稳定报错。hot、归属未知或仍含可恢复状态的 sidecar 必须在 canonical 名称按字节保留且不得开放 Store，绝不能盲删。只有归属与 identity 明确且已证明不含可恢复状态的残留才可通过数据库同目录、SQLite 外部的耐久 cleanup journal，按 `prepared→quarantined→done` 和 identity-bound no-replace quarantine 协议清理；journal 删除前不得快照、进入切换 `applying` 或开放 Store。清理中断后，安全残留的字节可以位于 canonical 或 quarantine，或在 `quarantined` 后已经删除并等待父目录耐久确认；不得要求所有清理失败都恢复原 canonical 路径。不得只复制、哈希或 rename 主文件后忽略已提交 WAL 帧，也不得让旧 sidecar 与新主文件组合。
- 备份恢复仅接受当前格式及最近 2 个旧格式版本；旧格式必须先在暂存区逐步升级到当前格式再执行完整校验和恢复，新于当前应用的备份格式必须拒绝。
- Capability 的 name 为主键，不可重复。
- 4 个 Builtin Function 必须在启动时幂等注册，固定 identifier 不得被自定义 Function 占用；Builtin 可查看和执行，但不可编辑、删除或改为 custom。
- 消息回复、结束对话和路由到直接子 Agent 是 Agent 运行时控制能力，不生成 Tool 数据记录，不出现在 Tool 管理列表，也不受用户删除影响。
- Tag 的 name 必须唯一。
- Category 的 slug 必须唯一。
- Agent 删除时需处理 parent_agent_id 引用：若存在子 Agent，将子 Agent 的 `parent_agent_id` 置为 NULL（不级联删除子 Agent）。
- Tool 的 function_id 和 workflow_id 互斥（CHECK 约束：`kind='function-wrap'` 时 function_id 非空，`kind='workflow-wrap'` 时 workflow_id 非空，另一目标必须为空）。
- Agent 的 parent_agent_id 不允许形成循环引用（如 A→B→A），保存时检测并提示错误。
- 没有 Agent 时允许不存在默认入口；存在 Agent 时必须且只能有一个 `is_default=true`，且默认入口必须是 parent_agent_id 为空、depth=0 的根 Agent。创建首个 Agent 时自动设为默认；切换默认入口必须原子取消旧默认；删除默认入口前必须先指定替代项。
- Agent 的 `depth` 根据 parent_agent_id 自动计算，根 Agent 为 0，最大允许 10；保存会导致深度超过 10 的层级时拒绝。
- 当前活动路由路径中再次出现同一 Agent 时，必须立即终止该路由并报告循环；不得通过累计跳转绕过最大深度。
- Agent 只能调用自身显式分配或 `is_always=true` 的 Tool/Skill；两类资源合并时按实体 ID 去重。
- Agent 执行 Function、Workflow 或 Plugin 前，所需 Capability 必须全部包含在该 Agent 显式分配的 Capability 集合中；缺少任一项时拒绝执行并显示缺失项。
- Workflow 保存和执行前必须验证 DAG：节点键在 Workflow 内唯一、连线端点存在、无环、恰好一个 `start_node` 且至少一个从它可达的 `end_node`；验证失败时不得保存或执行。
- 删除被 WorkflowNode 引用的 Function 时必须阻止删除，并列出引用它的 Workflow。删除未被 Tool 引用的 Workflow 时级联删除其 WorkflowNode 和 WorkflowEdge；若任一 Tool.workflow_id 引用该 Workflow，则必须以 `ON DELETE RESTRICT` 阻止删除，返回 `conflict { field: "id", reason: "referenced_by_tool", references }`，其中 references 只列出引用 Tool 的安全 ID、identifier 或 name，且 Workflow、节点、连线和 Tool 均保持不变。
- Workflow 任一节点失败或超时后必须停止调度尚未开始的节点；已在运行的节点允许完成并记录结果，但其输出不得再触发后续节点。执行结果必须标记失败节点、错误类别、耗时以及未执行节点。
- Workflow 层不得自动重试失败节点，也不承诺回滚已完成节点产生的文件、网络或数据库等外部副作用；需要重试的底层操作必须由明确保证幂等性的 Function/Capability 自行处理。

### 字段验证规则

- **identifier**: 最大长度 255 字符，仅允许字母、数字、下划线、连字符（正则：`^[a-zA-Z0-9_-]+$`）
- **name**: 最大长度 255 字符，不允许为空
- **slug**: 最大长度 255 字符，仅允许小写字母、数字、连字符（正则：`^[a-z0-9-]+$`），手动输入
- **description**: 可选，最大长度 2000 字符
- **JSON 字段**（input_schema, output_schema, manifest, frontmatter, required_capabilities, node_config, mapping）: 必填字段必须是有效 JSON；标注可选的字段可为空，但提供时必须是有效 JSON；单字段最大大小 1MiB
- **Capability 集合**: `required_capabilities` 以及 manifest 内同名字段必须是 JSON 字符串数组；元素去除首尾空白后长度为 1..=255、不得重复，并且每个名称都必须存在于当前目标的共享/本地 Capability registry。manifest 必须是 JSON 对象并包含受支持的 `abi_version` 与该数组。
- **color**（Tag）: 可选，格式为 HEX 颜色代码（如 `#FF5733`）
- **DataSource**: host 去除首尾空白后长度为 1..=255；port 范围 1..=65535；username 长度为 1..=255；创建时 password 不得为空且最大 64KiB，编辑时空 password 表示保留原密文。
- **GlobalConfig**: key 去除首尾空白后长度为 1..=255 且不得包含控制字符；type 长度为 1..=64；data 最大 1MiB。
- **LlmPreset**: max_tokens 范围 1..=1000000；temperature 必须是有限数值且范围为 0.0..=2.0。
- **LlmProvider**: category 必须由本地 provider registry 支持；base_url 为空或为最大 2048 字符的绝对 HTTP/HTTPS URL；token_env 为空或匹配 `^[A-Za-z_][A-Za-z0-9_]*$`；token 最大 64KiB。
- **外键与 ID**: 所有由用户或导入包提供的整数 ID 必须为正整数且目标记录存在；可选 ID 仅允许为空或引用合法目标。关联集合中的每个目标都必须在同一事务写入前完成存在性和重复项校验。
- **Model**: priority 范围 0..=1000000；preset_id 和 provider_id 必须分别引用现有 LlmPreset 与 LlmProvider。
- **Plugin 元数据**: runtime 固定为 `extism`；version 长度为 1..=64；author 为空或最大 255 字符；repository_url 为空或为最大 2048 字符的绝对 HTTP/HTTPS URL；s3_key 必须是托管目录内的安全相对制品键；sha256 必须匹配 `^[0-9a-f]{64}$`；size_bytes 必须大于 0 且与已校验托管制品的实际字节数一致。
- **Workflow**: timeout_ms 范围 1..=120000；start_description 最大 1MiB；category_id 为空或引用现有 Category。
- **WorkflowNode**: node_key 使用 identifier 规则；node_type 仅允许 `start_node|end_node|function_node|generate_answer_node`；position_x 和 position_y 必须是有限数值；node_config 为空或为最大 1MiB 的有效 JSON；workflow_id 必须存在，`function_node` 必须引用现有 Function，其他节点的 function_id 必须为空。
- **WorkflowEdge**: workflow_id 必须存在，src_node_key 和 dst_node_key 必须引用同一 Workflow 的现有节点；mapping 为空或为最大 1MiB 的有效 JSON。
- **Function、Tool 与 Skill**: Function.kind 仅允许 `builtin|custom|placeholder`，Tool.kind 仅允许 `function-wrap|workflow-wrap`；Tool.source 和 Skill.source 仅允许 `workspace|builtin`；custom Function 的 plugin_export 去除首尾空白后长度为 1..=255，builtin Function 的 plugin_export 必须为空；placeholder Function 只保留 identifier/name/description/input_schema/output_schema/category_id，plugin_id、plugin_export、required_capabilities 必须为空且任何公开执行入口必须在 Capability 或 Plugin 查找前返回 `function_not_executable`；Tool 的目标外键必须与 kind 匹配且互斥；Skill.content 最大 1MiB。
- **Agent**: system_prompt 去除首尾空白后不得为空且最大 1MiB；parent_agent_id 为空或引用现有 Agent并满足无环和 depth≤10；model_preset 为空或引用现有 LlmPreset.name；depth、时间戳等派生字段由 Store 生成，不接受 UI 覆盖。
- **分页与搜索**: page≥1；普通列表 page_size 固定为20；search 为空时表示不筛选，非空搜索词长度为 1..=255 个 Unicode 标量值。搜索字段与输入必须使用标识固定为 `hivegui-nfkc-casefold-v1`、基于 Unicode 17.0.0 的确定性 `NFKC_Casefold` 规则后再比较；原始非空但规范化后为空时返回 `invalid_input { field: "search", reason: "empty_after_normalization" }`。`%`、`_`、引号及 FTS 操作符必须由中央字面量编码器安全引用，不得改变纯文本查询语义。
- **本地运行时命令**: session_id 和 execution_id 必须是有效 UUID；user_message 去除首尾空白后长度为 1..=1MiB；retention_filter 必须使用受支持单位且数值大于0。
- **Plugin 资源限制**: timeout_ms 范围 1..=120000；memory_limit_mb 逻辑单位固定为 MiB、范围 1..=512；output_limit_bytes 范围 1..=52428800；未设置时分别使用 30000、128（128MiB）、10485760（10MiB）。64KiB WASM page 数按 `memory_limit_mb × 16` 计算。
- **枚举、布尔值与时间戳**: 所有 enum 和 `is_default`、`is_always`、`is_dangerous` 等布尔输入必须使用实体定义的稳定值；created_at、updated_at、deleted_at、expires_at 和 execution 时间戳由 Store/运行时生成或由受信迁移恢复流程校验，普通 UI 不得覆盖。
- **通用字符串安全**: 所有字符串输入必须拒绝 NUL 和不允许的控制字符。机密字段错误只能返回字段名和脱敏原因，不得回显原值。
- **数据库解释器安全**: 用户输入不得通过字符串拼接进入 SQLite 或 MySQL。所有固定应用 schema SQL 必须使用 SQLx `query!`/`query_as!`/`query_scalar!` 及提交的 offline metadata；值只能通过这些宏的 bind 参数进入查询。有限的条件、表名或列名变体必须由封闭 enum 和穷尽 `match` 选择有限组静态、编译期检查的查询，生产代码禁止 SQLx `QueryBuilder`、运行时 SQL 字符串和任意动态标识符。HiveGUI 外部 MySQL 浏览器属于独立解释器边界：值必须使用 `mysql_async` prepared statement/参数绑定，数据库名、表名和列名不能作为 bind 值，只能先与当前连接加载的服务器 metadata allowlist 精确匹配，再由唯一、经过测试且按上下文序列化的 `MysqlIdentifier` 写入 SQL。HiveWeb 应用 schema 中乐观锁允许的表名必须使用独立封闭 enum，不能接收任意 `&str` 或复用 `MysqlIdentifier`；`game_service` 的 API `category_name` 必须绑定到静态 `JSON_OBJECT('name', ?)`，恶意引号、注释和控制字符不得改变 SQL 结构或查询目标。唯一运行时 SQLx 例外是启动时从受信 `named_queries.toml` 加载的完整 SQL：它只允许通过一个 reviewed-config 中央审计边界，该边界必须 fail-closed 拒绝多语句、SQL 注释、placeholder/参数不匹配、重复参数和 select/execute kind 不匹配，并且是生产代码中唯一允许构造 SQLx `AssertSqlSafe` 的位置。原始用户文本、临时 `format!` 拼接和分散的 escape helper 均不得生成 SQL 或 MySQL 标识符。每个含过滤或关联条件的生产查询必须保存 SQLite `EXPLAIN QUERY PLAN` 或 MySQL `EXPLAIN` 证据，并断言所有过滤/关联列使用预期索引；FTS5 计划中的 `VIRTUAL TABLE INDEX` 必须识别为索引访问，不能仅因计划文本包含 `SCAN` 就误判。非小型表出现未经批准的真实全表扫描、缺少预期索引或计划无法判定时测试必须失败。仅小型固定表或 metadata 查询可例外，且必须记录表大小、理由、审批者、到期日和复核结果。测试环境中的查询计数必须在发现 N+1 时失败。上述 HiveWeb 审计边界仅用于云端 HiveWeb 自身，不构成 HiveGUI 对 HiveWeb 的调用或回退路径。

所有用户输入、配置写入和文件导入必须在进入 Store、运行时或导入服务的公开边界时执行上述校验，不得只依赖 UI 校验。普通校验失败必须返回稳定的 `invalid_input { field, reason }` 且不得携带被拒绝值；唯一性冲突必须返回 `conflict { field, value }`，其中 value 仅允许 identifier、name、slug、key 等安全字段值；引用或状态冲突必须返回不含 value 的 `conflict { field, reason, references }`，references 仅包含安全的实体类型、ID、identifier 或 name。密码、token、消息正文和其他机密值不得回显。失败不得写入或部分修改数据库；UI 必须保留用户输入并显示字段及原因。

### UI 交互规范

- 列表为空时，显示友好的空状态提示（如"暂无数据，点击添加按钮创建"）
- 删除操作必须弹出确认对话框，显示"确定要删除 [实体名称] 吗？此操作不可撤销。"
- 表单提交失败（如数据库写入失败、唯一性约束冲突）时，保留表单数据，显示错误消息，不关闭表单
- 搜索无结果时，显示"未找到匹配项"提示
- 恢复确认对话框必须明确说明现有数据库和托管 Plugin 制品将被完整替换，并显示确认后将在冻结周期内生成的安全备份计划保存位置；用户取消时不得修改任何状态，也不得把确认前的预览或临时副本视为本次安全备份。
- Agent 执行错误界面必须显示 execution_id 和可操作的错误摘要，并提供导出该 execution_id 相关脱敏诊断包的入口。
- 后台任务运行时必须显示进行中状态和取消/停止入口；用户触发停止后，界面必须立即显示已接收请求，即使底层资源仍在完成安全终止。
- 停止完成后界面必须显示 cancelled 最终状态、停止前已完成步骤、未开始步骤及“已完成的外部副作用不会自动回滚”提示；若 Plugin 在 2 秒宽限期后被强制终止，必须显示对应稳定错误类别。
- schema 迁移失败时必须显示恢复界面，包含失败版本步骤、稳定错误类别、安全快照位置以及“重试迁移”和“退出”操作；不得显示主界面或允许业务写入。
- 首页导航、所有 CRUD、搜索、分页、Agent 资源分配、对话、备份/恢复和 DAG 编辑必须可仅用键盘完成，键盘焦点始终可见，操作顺序与视觉顺序一致。
- 模态框打开时焦点必须进入并限制在模态框内，关闭后恢复到触发控件；新增、删除或重排项目后焦点必须移动到可预测且仍存在的控件。
- 交互控件必须向平台无障碍树暴露名称、角色、状态和错误信息；加载、成功、失败、危险、选中和禁用状态不得只通过颜色表达。
- 普通文本与背景对比度必须至少 4.5:1，大字号文字、焦点指示器和关键 UI 图形必须至少 3:1。

### 加密密钥管理

- 加密密钥（chacha20poly1305）存储在本地配置文件中（如 `~/.hivegui/key`）；Unix 文件权限必须设置为 0600，其他平台必须使用仅当前用户可读写的等效 ACL。
- 首次启动时必须使用系统安全随机源生成密钥并原子保存；并发首次启动只能产生并使用一个最终密钥。
- 已有加密数据时，密钥缺失、长度错误、权限不安全或无法读取不得触发静默重新生成，也不得覆盖数据库；应用必须进入阻断式重新配置/从备份恢复流程。
- 设备本地密钥不得写入备份包；可移植备份使用独立的用户备份口令进行整体认证加密。
- 恢复时使用备份口令解密包内敏感值，并在写入前使用目标设备本地密钥重新加密；备份口令和敏感明文不得持久化。
- 会话消息、Tool 调用参数/结果和执行状态属于敏感数据，静态存储时必须与密码/API Key 一样使用设备本地密钥加密。


---

### 用户故事 4 - LLM 配置管理 (优先级: P1)

管理员希望在 HiveGUI 中配置 LLM 预设（Preset）、提供商（Provider）和模型（Model）。Provider 独立维护连接与凭据；Preset 通过一组有优先级的 Model 形成 fallback 链，每个 Model 引用一个 Provider。

**独立测试**: 创建独立 Provider → 创建 Preset → 添加引用该 Provider 的 Model 和 Agent → 重命名 Preset并验证 Agent 引用原子更新 → 阻止删除被 Agent 引用的 Preset → 移除引用后删除 → 重启验证持久化。

**验收场景**:

**Model 管理**:
1. **假设** Model 管理面板已打开，**当** 用户点击"添加"时，**则** 显示 Model 表单（名称、Preset、Provider、优先级），保存后出现在所选 Preset 的列表中。
2. **假设** 存在一个 Model，**当** 用户删除时，**则** 该 Model 从对应 Preset 的列表移除。
3. **假设** 存在多个 Preset，**当** 用户切换 Preset 时，**则** 只显示属于该 Preset 的 Model。

**Provider 管理**:
4. **假设** Provider 管理面板已打开，**当** 用户点击"添加 Provider"时，**则** 显示独立 Provider 表单（name、category 下拉、base_url、token、token_env），不显示 Preset 或 Model 选择；token_env 非空时运行时优先读取对应环境变量。
5. **假设** 存在一个 Provider，**当** 用户编辑并保存时，**则** 更新后的配置被持久化。
6. **假设** 存在一个未被 Model 引用的 Provider，**当** 用户删除时，**则** 该 Provider 从独立列表中移除；被 Model 引用时阻止删除。

**Preset 管理**:
7. **假设** Preset 管理面板已打开，**当** 用户点击"添加"时，**则** 显示 Preset 表单（名称、描述、默认标志、max_tokens、temperature）。
8. **假设** 用户创建 Preset 并标记为默认，**当** 已存在另一个默认 Preset 时，**则** 系统自动取消旧 Preset 的默认标志。
9. **假设** Agent 引用了一个 Preset，**当** 用户重命名该 Preset 时，**则** Preset 名称和全部 Agent.model_preset 在同一事务中更新。
10. **假设** Agent 引用了一个 Preset，**当** 用户尝试删除该 Preset 时，**则** 系统返回 conflict、列出引用 Agent 并保持 Preset、Model 和 Agent 不变。

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

管理员希望在 HiveGUI 中管理和导入本地插件（Plugin）。插件包含标识符、名称、版本、运行时、描述等完整字段；选择的 WASM 文件由 HiveGUI 复制到本地托管插件目录，后续执行不依赖原文件或 HiveWeb。

**独立测试**: 选择 WASM 文件并添加插件 → 验证文件复制到 HiveGUI 托管目录 → 删除原始文件 → 执行插件 → 编辑并替换 WASM → 删除 → 重启验证持久化。

**验收场景**:

1. **假设** 插件管理面板已打开，**当** 用户点击"添加"并选择有效 WASM 文件时，**则** HiveGUI 将文件复制到本地托管插件目录，计算 SHA-256 与大小，并保存插件元数据、相对制品键和可选资源限制；`s3_key` 仅作为兼容字段名，不表示 S3 地址。
2. **假设** 存在一个插件，**当** 用户编辑并保存时，**则** 更新后的数据被持久化。
3. **假设** 存在一个插件，**当** 用户删除并确认后，**则** 该插件从列表移除。
4. **假设** 用户尝试创建 identifier 重复的插件，**当** 保存时，**则** 系统提示 identifier 已存在并阻止保存。
5. **假设** Plugin 已成功导入，**当** 用户移动或删除最初选择的 WASM 文件时，**则** HiveGUI 仍可从托管目录加载并执行该 Plugin。
6. **假设** 托管 WASM 文件缺失或其 SHA-256 与记录不一致，**当** HiveGUI 尝试加载或执行 Plugin 时，**则** 在执行前拒绝并提示用户重新导入文件。
7. **假设** 用户编辑 Plugin 但未选择替换文件，**当** 保存时，**则** 保留现有托管 WASM；选择替换文件时，以 no-replace 发布经过复制与校验的新不可变唯一制品键，再以记录版本条件在数据库事务中切换引用，绝不原地覆盖现有制品。
8. **假设** 来自 HiveWeb 的 Plugin 使用 HiveGUI 支持的 ABI 版本且声明的 Capability 全部可用，**当** 用户导入同一 WASM 和 manifest 时，**则** HiveGUI 无需转换即可加载并按共享 ABI 执行。
9. **假设** Plugin 使用不支持的 ABI、无效 manifest 或声明了 HiveGUI 不支持的 Capability，**当** 用户导入时，**则** HiveGUI 在复制和写库前拒绝，并一次列出 ABI、schema 和全部缺失 Capability 问题。
10. **假设** Plugin 未设置资源覆盖值，**当** 执行时，**则** 使用 30 秒、128MiB、10MiB 默认限制；设置了合法覆盖值时使用覆盖值。
11. **假设** 用户配置超过硬上限或 Plugin 执行达到任一限制，**当** 保存或执行时，**则** 超限配置被拒绝，运行中超限调用被终止并返回 timeout、memory_limit 或 output_limit 稳定错误类别。
12. **假设** Plugin 尝试直接访问 WASI 文件、网络或环境变量，**当** 未通过已授权 Capability 时，**则** 宿主拒绝访问并记录 capability_denied 或 wasi_denied。

---

### 用户故事 9 - 函数管理 (优先级: P1)

管理员希望在 HiveGUI 中管理函数（Function）。函数包含标识符、名称、kind（builtin/custom/placeholder）、input/output schema、可选 plugin 关联和分类。Placeholder 是仅用于 LLM 提示词与 Tool schema 调试的不可执行 Function。

**独立测试**: 执行四个下划线 Builtin 并拒绝四个点号名称 → 添加/执行 Custom Function → 添加 Placeholder 并验证 schema-only UI 与两个公开执行入口均在 Plugin/Capability 查找前稳定拒绝 → 编辑、删除并重启验证持久化。

**验收场景**:

1. **假设** 函数管理面板已打开，**当** 用户点击"添加"时，**则** 显示函数表单（identifier, name, kind, description, input_schema, output_schema, plugin_id, plugin_export, category_id, required_capabilities），保存后出现在列表中。
2. **假设** 存在一个函数，**当** 用户编辑并保存时，**则** 更新后的数据被持久化。
3. **假设** 存在一个函数，**当** 用户删除并确认后，**则** 该函数从列表移除。
4. **假设** 用户尝试创建 identifier 重复的函数，**当** 保存时，**则** 系统提示 identifier 已存在并阻止保存。
5. **假设** HiveGUI 首次启动或升级后启动，**当** Function 存储初始化时，**则** 系统幂等注册 `format_template`、`json_parse`、`json_stringify`、`text_regex_match`，重复启动不产生重复记录。
6. **假设** 用户查看 Builtin Function，**当** 尝试编辑、删除或以相同 identifier 创建 custom Function 时，**则** HiveGUI 拒绝操作并说明该 identifier 由系统保留。
7. **假设** 用户选择 Placeholder，**当** 编辑表单时，**则** 仅显示通用字段和 input/output schema，隐藏 Plugin、Capability 与测试执行操作；保存时清空所有可执行关系。
8. **假设** Placeholder 通过任一公开 Function 或 Capability 执行入口被调用，**当** 运行时解析请求时，**则** 在 Plugin 查找与 Capability 检查前返回 `function_not_executable` 且不启动任何 guest。
9. **假设** 调用四个点号名称之一，**当** Builtin registry 查询或执行时，**则** 返回未找到；这些名称不得作为下划线 Builtin 的别名存在。

---

### 用户故事 10 - 本地 Workflow DAG 管理与执行 (优先级: P1)

管理员希望在 HiveGUI 中构建、编辑并执行本地工作流（Workflow）。Workflow 由主表元数据、WorkflowNode 和 WorkflowEdge 构成，使用可视化 DAG 编辑器维护节点、连线、节点配置和数据映射，并由 HiveGUI 在本地执行。

**独立测试**: 添加 Workflow → 在 DAG 编辑器添加 `start_node`、`function_node` 和 `end_node` 并连线 → 保存并重启验证持久化 → 本地执行并验证输出 → 删除未被 Tool 引用的 Workflow 并验证节点和连线级联删除 → 通过 Tool 引用另一 Workflow 并验证删除被安全阻止且全部相关记录不变。

**验收场景**:

1. **假设** 工作流管理面板已打开，**当** 用户点击"添加"时，**则** 显示工作流表单（identifier, name, description, timeout_ms, category_id, input_schema, start_description, output_schema, required_capabilities），保存后可进入 DAG 编辑器。
2. **假设** Workflow 已创建，**当** 用户在 DAG 编辑器添加、配置、移动或删除 WorkflowNode，并创建或删除 WorkflowEdge 时，**则** 画布即时显示变更且保存后完整持久化。
3. **假设** DAG 包含唯一 start、至少一个可达 end、有效连线且无环，**当** 用户保存并执行时，**则** HiveGUI 按依赖顺序在本地执行节点并返回 Workflow 输出。
4. **假设** DAG 存在重复节点键、悬空连线、循环、多个 start 或没有可达 end，**当** 用户保存或执行时，**则** HiveGUI 拒绝操作并定位具体结构错误。
5. **假设** 存在一个工作流，**当** 用户编辑主表或 DAG 并保存时，**则** 更新后的数据被持久化，重启后结构不变。
6. **假设** 存在一个未被 Tool 引用的工作流，**当** 用户删除并确认后，**则** Workflow、WorkflowNode 和 WorkflowEdge 一并删除。
7. **假设** Tool.workflow_id 引用了一个 Workflow，**当** 用户尝试删除该 Workflow 时，**则** HiveGUI 返回 `conflict { field: "id", reason: "referenced_by_tool", references }`，references 只包含引用 Tool 的安全 ID、identifier 或 name，且 Workflow、WorkflowNode、WorkflowEdge 和 Tool 均保持不变。
8. **假设** 用户尝试创建 identifier 重复的工作流，**当** 保存时，**则** 系统提示 identifier 已存在并阻止保存。
9. **假设** Function 已被 WorkflowNode 引用，**当** 用户尝试删除该 Function 时，**则** HiveGUI 阻止删除并列出引用它的 Workflow。
10. **假设** Workflow 中某节点失败或超时，**当** 执行器收到失败结果时，**则** 不再调度尚未开始的节点，允许已运行节点结束，并返回失败节点、错误类别、耗时和未执行节点列表。
11. **假设** 失败前已有节点完成了外部副作用，**当** Workflow 以失败结束时，**则** HiveGUI 不自动回滚或重新执行这些节点，并向用户明确提示可能存在已完成的外部副作用。
12. **假设** 用户不使用鼠标，**当** 在 DAG 编辑器创建、选择、移动、配置、连接或删除节点和连线时，**则** 所有操作均可通过键盘完成，焦点位置和连接状态可被无障碍技术识别。

---

### 用户故事 11 - 工具管理 (优先级: P1)

管理员希望在 HiveGUI 中管理工具（Tool）。工具包含标识符、名称、kind（function-wrap/workflow-wrap）、source、is_always 等字段。

**独立测试**: 添加工具 → 编辑 → 删除 → 重启验证持久化。

**验收场景**:

1. **假设** 工具管理面板已打开，**当** 用户点击"添加"时，**则** 显示工具表单（identifier, name, description, kind, source, is_always, function_id, workflow_id, input_schema, output_schema, category_id, required_capabilities），保存后出现在列表中。
2. **假设** 存在一个工具，**当** 用户编辑并保存时，**则** 更新后的数据被持久化。
3. **假设** 存在一个工具，**当** 用户删除并确认后，**则** 该工具从列表移除。
4. **假设** 用户尝试创建 identifier 重复的工具，**当** 保存时，**则** 系统提示 identifier 已存在并阻止保存。
5. **假设** Agent 正在回复、结束对话或路由直接子 Agent，**当** 用户查看 Tool 管理列表时，**则** 这些运行时控制能力不作为 Tool 数据记录出现。

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

### 用户故事 13 - 本地 Agent 执行与管理 (优先级: P1)

管理员希望在 HiveGUI 中管理并运行本地 Agent。Agent 包含标识符、名称、描述、system_prompt、parent_agent_id、自动计算的 depth、唯一默认入口标志、model_preset、分配的 Tool、Skill 和 Capability；对话编排、子 Agent 路由、LLM 调用以及 Tool、Skill、Function、Workflow、Plugin 的调用链均由 HiveGUI 本地发起，不经过 HiveWeb。

**独立测试**: 创建默认入口 Agent 及两层子 Agent → 分配 Tool、Skill 和 Capability → 验证 `is_always` 资源自动可用 → 从默认入口发起对话并路由到直接子 Agent → 完成一次本地工具调用 → 编辑 → 删除 → 重启验证持久化；测试期间 HiveWeb 不运行。

**验收场景**:

1. **假设** Agent 管理面板已打开，**当** 用户点击"添加"时，**则** 显示 Agent 表单（identifier, name, description, system_prompt, parent_agent_id, is_default, model_preset、Tool、Skill、Capability）；depth 根据所选父 Agent 自动计算，保存后 Agent 出现在列表中。
2. **假设** 存在一个 Agent，**当** 用户编辑并保存时，**则** 更新后的数据被持久化。
3. **假设** 存在一个 Agent，**当** 用户删除并确认后，**则** 该 Agent 从列表移除。
4. **假设** 用户尝试创建 identifier 重复的 Agent，**当** 保存时，**则** 系统提示 identifier 已存在并阻止保存。
5. **假设** Agent 已配置可用的 LLM 与本地执行资源且 HiveWeb 未运行，**当** 用户向 Agent 发送消息时，**则** HiveGUI 在本地完成对话编排并直接调用所配置的 LLM。
6. **假设** LLM 返回本地 Tool 调用或当前 Agent 加载了 Skill，**当** Tool 包装 Function/Workflow，或自定义 Function 关联 Plugin 时，**则** HiveGUI 在本地解析并执行对应调用链，将结果继续交给本地 Agent 编排。
7. **假设** HiveWeb 不可达，**当** 用户启动 HiveGUI、管理 Agent 或执行 Agent 时，**则** 操作正常进行且 HiveGUI 不发送 HiveWeb 请求。
8. **假设** 本地执行链中的 LLM 或其他外部资源不可达，**当** Agent 执行到该资源时，**则** HiveGUI 显示可定位到失败资源的错误，不回退到 HiveWeb。
9. **假设** 某 Tool 或 Skill 已显式分配给 Agent，**当** Agent 开始执行时，**则** 该资源出现在当前 Agent 的可用资源集合中。
10. **假设** 某 Tool 或 Skill 的 `is_always=true` 且未显式分配给 Agent，**当** Agent 开始执行时，**则** 该资源仍自动出现在可用资源集合中。
11. **假设** 执行资源声明了 Agent 未获分配的 Capability，**当** Agent 尝试调用该资源时，**则** HiveGUI 在执行前拒绝调用并显示缺失的 Capability。
12. **假设** 已配置唯一默认入口 Agent，**当** 用户开始新对话时，**则** HiveGUI 从该 Agent 启动本地编排。
13. **假设** 当前 Agent 有直接子 Agent 和更深层后代，**当** LLM 请求路由时，**则** 只能选择直接子 Agent，跨级目标在执行前被拒绝。
14. **假设** 新的父子关系会使 Agent 深度超过 10 或形成循环，**当** 用户保存时，**则** HiveGUI 拒绝保存并指出违规关系。
15. **假设** 同一会话路由路径准备再次进入已执行过的 Agent，**当** HiveGUI 处理路由请求时，**则** 立即终止路由并报告循环路径。
16. **假设** Agent 或其本地执行链正在后台运行，**当** 用户导航、输入或触发停止时，**则** 界面在响应预算内显示反馈，停止控件保持可操作且主 UI 线程不被长任务阻塞。
17. **假设** 用户完成对话并重启 HiveGUI，**当** 会话仍在保留期内时，**则** 可恢复消息、当前 Agent、execution_id 和最终执行状态，敏感内容在数据库中保持加密。
18. **假设** 用户缩短保留期、删除会话或清空全部历史，**当** 确认操作时，**则** HiveGUI 删除匹配的 ChatSession、ChatMessage 和执行状态，不删除 Agent 及其配置。
19. **假设** Agent 正在执行 LLM、Tool、Workflow 或 Plugin 调用，**当** 用户触发停止时，**则** 取消信号传播到整条调用链、不再调度新工作、可取消的网络请求被取消，Plugin 最多等待 2 秒后被强制终止，最终 AgentExecution 持久化为 cancelled 并提示已完成的外部副作用不会自动回滚。

---

## 需求 *(必填)*

### 功能需求

- **FR-001**: 系统必须提供首页视图，显示应用标题和导航入口。
- **FR-002**: 系统必须提供数据源管理界面，支持添加、编辑、删除远程 MySQL 数据源配置。
- **FR-003**: 系统必须支持测试数据源连接（MySQL），并显示连接测试结果。
- **FR-004**: 数据源配置（名称、主机、端口、用户名、加密密码）必须持久化到本地 SQLite 数据库。
- **FR-005**: 连接测试、保存、Agent、LLM、Tool、Workflow、Plugin、备份和恢复预验证等长任务必须异步执行，不得阻塞主 UI 线程；任务运行时必须提供持续可操作的取消/停止入口和明确状态。
- **FR-006**: 系统必须在启动时区分数据库不存在、schema 迁移失败和完整性损坏。不存在时创建全新 v4；已有数据库打开前、新数据库建成后以及迁移提交前都必须通过同一个 SQLite 健康检查：`PRAGMA integrity_check` 精确返回唯一 `ok` 且 `PRAGMA foreign_key_check` 返回零行，不能以 `PRAGMA foreign_keys=ON` 代替。已有源库任一检查失败属于完整性损坏，必须阻止主界面，并提供从备份恢复、保留损坏文件后重建或退出；迁移目标提交前检查失败属于迁移失败，必须回滚、保留安全快照且只允许重试或退出。重建必须二次确认，且不得在用户确认前修改数据库或托管 Plugin。
- **FR-007**: 系统必须提供全局配置管理界面，支持添加、编辑、删除全局配置项（name, key, type, data），支持搜索和分页。
- **FR-008**: 全局配置数据必须持久化到本地 SQLite 数据库，key 唯一。
- **FR-009**: 系统必须提供 Model 管理界面，支持添加、编辑、删除 Model（name, preset_id, provider_id, priority）；Model 属于一个 Preset，并引用一个 Provider。
- **FR-010**: 系统必须提供独立 Provider 管理界面，支持添加、编辑、删除 Provider（name, category, base_url, token, token_env），不按 Preset 分组。name 必须唯一，category 必须由本地 provider registry 支持；token 在界面中遮蔽并以 token_encrypted 存储，token_env 只保存环境变量名且非空时优先于 token；被 Model 引用的 Provider 不可删除。
- **FR-011**: 系统必须提供 Preset 管理界面，支持添加、编辑、删除 Preset（name, description, is_default, max_tokens, temperature）。删除时级联删除关联 Model且不删除 Provider，但被 Agent.model_preset 引用的 Preset 不可删除并必须列出引用 Agent；重命名 Preset 必须在同一事务中更新全部 Agent.model_preset。
- **FR-012**: 密码和 API Key 必须使用设备本地密钥加密存储，界面显示为遮蔽形式；设备本地密钥不得进入备份包。首次启动必须使用系统安全随机源原子生成设备密钥，Unix 权限必须为 0600，其他平台必须使用仅当前用户可读写的等效 ACL。已有加密数据时，密钥缺失、损坏、权限不安全或不可读必须阻止敏感数据访问并进入重新配置/从备份恢复流程，不得静默生成替代密钥或修改数据库。可移植备份必须使用用户提供的独立备份口令对整个包进行认证加密。**FR-012 设备密钥与 FR-049 主密码认证是两条独立但有严格依赖关系的密钥边界**：设备密钥（FR-012）由系统安全随机源生成并仅由本地 ACL 保护，作为工作密钥加密所有敏感字段（FR-046 会话、API Key、Provider token、DataSource 密码）；KEK（FR-049）由主密码经 Argon2id 派生，作为 wrapping KEK 包装设备密钥材料，二者任一缺失/损坏/失败都独立 fail-closed，不构成相互旁路。
- **FR-013**: Preset 的 Model 列表按 priority 排序，作为 fallback 链。
- **FR-014**: 系统必须提供标签（Tag）管理界面，支持添加、编辑、删除标签（name, color）。
- **FR-015**: 系统必须提供分类（Category）管理界面，支持添加、编辑、删除分类（name, slug, description, parent_id），UI 以树形视图展示层级结构。
- **FR-016**: 系统必须提供能力（Capability）管理界面，支持添加、编辑、删除能力（name, description, is_dangerous, category_id）。数据库中的 Capability 仅是可管理元数据，声明或持久化同名记录不得自动安装本地 handler；运行时可用性只能来自 `hive-runtime-core` 显式注册且当前进程实际持有的本地 handler。仅声明、未知、重复注册或未授权的调用必须返回稳定错误，可用项枚举必须使用确定性顺序，核心注册表不得依赖 HiveGUI/HiveWeb 的存储或传输实现。
- **FR-017**: 系统必须提供插件（Plugin）管理界面，支持导入、添加、编辑、删除插件（identifier, name, description, manifest, runtime, version, author, repository_url, s3_key, sha256, size_bytes, category_id, timeout_ms, memory_limit_mb, output_limit_bytes）。identifier 必须唯一。HiveGUI 保留 `s3_key` 字段名以复用数据结构，但其值必须是 HiveGUI 托管插件目录内的相对制品键，不得解释为远程 S3 地址。
- **FR-018**: 系统必须提供 Function 管理界面。Custom 与 Placeholder Function 支持添加、编辑、删除；Builtin Function 可查看和执行但不可编辑或删除。字段包括 identifier, name, kind, description, input_schema, output_schema, plugin_id, plugin_export, category_id, required_capabilities，identifier 必须唯一；kind 的稳定值为字符串 `builtin|custom|placeholder`。Placeholder 仅用于 LLM 提示词与 Tool schema 调试，plugin_id、plugin_export、required_capabilities 必须为空；全部公开执行入口必须在 Capability 检查和 Plugin 查找前返回稳定错误 `function_not_executable`。
- **FR-019**: 系统必须提供本地 Workflow 管理与 DAG 编辑界面，支持添加、编辑、删除 Workflow 主表（identifier, name, description, timeout_ms, category_id, input_schema, start_description, output_schema, required_capabilities）、WorkflowNode 和 WorkflowEdge，并支持在本地验证和执行 DAG。Workflow identifier 和同一 Workflow 内的 node_key 必须分别唯一。
- **FR-020**: 系统必须提供工具（Tool）管理界面，支持添加、编辑、删除工具，字段完整对齐 hiveweb（identifier, name, description, kind, source, is_always, function_id, workflow_id, input_schema, output_schema, category_id, required_capabilities）。identifier 必须唯一；持久化与序列化 roundtrip 中 kind 的稳定值只能是字符串 `function-wrap|workflow-wrap`，source 只能是 `workspace|builtin`，Function/Workflow 目标必须随 kind 严格互斥，required_capabilities 必须按输入稳定顺序保存、不得重复且通过已知 Capability 校验。该持久化 Tool 契约必须位于无产品存储或传输依赖的共享核心中；消息回复、结束对话和子 Agent 路由不得序列化为 Tool。
- **FR-021**: 系统必须提供技能（Skill）管理界面，支持添加、编辑、删除技能，字段完整对齐 hiveweb（identifier, name, description, frontmatter, content, source, is_always, category_id, required_capabilities）。identifier 必须唯一。
- **FR-022**: 系统必须提供 Agent 管理界面，支持添加、编辑、删除 Agent（identifier, name, description, system_prompt, parent_agent_id, depth, is_default, model_preset），并支持为 Agent 分配 Tool、Skill 和 Capability。identifier 必须唯一；depth 必须由父子关系自动计算，不由用户直接编辑。
- **FR-023**: 除 Agent↔Tool、Agent↔Skill、Agent↔Capability 以及 Workflow↔WorkflowNode↔WorkflowEdge 外，所有新增实体的管理仅做独立增删改查，不实现未明确要求的关系管理（如不为任意实体打标签）。
- **FR-024**: 除 Category 树形视图外，所有新增实体的列表必须支持分页（每页 20 条）和大小写不敏感的纯文本任意位置包含搜索，搜索范围为实体的 `name` 和 `identifier` 字段（如适用）。identifier 的唯一性与精确 lookup 保持 ASCII 字节级、大小写敏感语义，因此 `Test` 与 `test` 是两个不同 identifier；搜索仍按下述大小写不敏感规范化同时返回二者。字段和输入必须使用标识为 `hivegui-nfkc-casefold-v1` 的同一确定性规则：逐标量应用 Unicode 17.0.0 `NFKC_CF` 映射后再执行 Unicode 17.0.0 NFC；数据库必须持久化该搜索索引格式 ID，映射来源 URL 与校验值必须可审计；ID、Unicode 数据、映射来源或 golden fixture 变化只能通过显式 schema 迁移分配新 ID、重建并验证索引。原始非空搜索词规范化后为空时必须返回 `invalid_input { field: "search", reason: "empty_after_normalization" }`。规范化后至少 3 个 Unicode 标量值的搜索词必须使用 SQLite FTS5 trigram 虚拟表上的参数化字面量查询，规范化后长度 1–2 的词必须使用事务同步的 short-gram 索引。FTS 查询允许使用 bind 的完整 phrase 或能保持索引访问的等价参数化模式，但不得在非小型业务表退化为前导通配 `LIKE '%keyword%'` 扫描。SQLite 运行时不提供 FTS5 trigram tokenizer 时，启动、新建 schema 和迁移必须 fail-closed，不得回退到普通表扫描或内存扫描。`%`、`_`、引号和 FTS 操作符必须经中央编码后按字面量匹配，实体写入与两类搜索索引更新必须在同一事务完成。每个实体在 name 与 identifier 同时命中时仍只返回一次，不设置字段相关性优先级；所有列表和搜索使用规范化显示名升序、规范化 identifier/key 升序、数值主键升序的总排序，缺失项用空字符串，保证数据不变时跨页稳定。Category 必须通过一次批量查询加载完整树，禁止 N+1；搜索结果必须保留匹配节点的祖先路径。
- **FR-025**: 系统必须为本地持久化和文件管理操作提供错误恢复机制。对于数据库锁定、文件占用等临时性本地存储错误，自动重试最多 3 次，每次间隔递增（1s, 2s, 4s）。该重试规则不适用于 Workflow 节点执行。系统必须提供从备份包手动恢复数据的功能。
- **FR-026**: 系统必须提供手动导出和恢复单一可移植备份包的功能。备份包必须包含带格式版本、schema 版本和导出时间的 JSON 清单，清单包含所有实体（Tag、Category、Capability、Plugin、Function、Workflow、WorkflowNode、WorkflowEdge、Tool、Skill、Agent、ChatSession、ChatMessage、AgentExecution、DataSource、GlobalConfig、LLM 配置）以及 AgentTool、AgentSkill、AgentCapability 关联的完整数据；备份包还必须包含全部 HiveGUI 托管 Plugin WASM，并为每个制品记录安全的相对制品键、大小和 SHA-256。 `.hivegui-db-staging-v1` registry、`.hivegui-db-instance-v1.json`、SQLite cleanup journal/quarantine、设备密钥、运行日志、诊断包与切换日志均为外部/设备内部状态，不得进入可移植 archive 或本地回滚安全备份。Plugin operation/GC ledger 与派生搜索结构不作为可移植 archive 的实体导出；但本地回滚安全备份是冻结封闭边界上的 SQLite 主文件精确快照，必须随主文件保留这些内部表，恢复旧状态时不得过滤或重建后冒充原快照。整个备份包必须使用用户提供的独立备份口令进行认证加密。导出最终目标必须尚不存在；系统必须在目标目录内写入唯一 staging 文件，完成 flush 与 fsync 后以 no-replace 原子重命名为最终路径并 fsync 父目录；任一步失败都不得覆盖既有目标或把未持久化、部分写入的文件报告为有效备份。
  `.hivegui-db-staging-v1` 下的 live/tombstone instance、manifest/owner/retirement journal final 与固定 `.staging` 槽、SQLite cleanup journal/quarantine 都属于外部 control state，既不进入可移植 archive，也不进入本地回滚安全备份或待切换新树；这不改变安全备份必须逐字节保留 SQLite 主文件内部表的要求。
- **FR-027**: 系统必须支持数据库 schema 和备份格式版本管理。当前应用仅自动支持最近 2 个旧版本，并按版本顺序逐步向前迁移；不支持降级。数据库或备份版本新于当前应用时必须拒绝，更旧版本必须提示通过中间版本升级。
- **FR-028**: HiveWeb 和 HiveGUI 必须作为彼此独立的 Agent 运行：HiveWeb 部署在云端，HiveGUI 在本地执行；二者可以复用代码，但 HiveGUI 不得调用 HiveWeb 的接口，也不得以 HiveWeb 的可用性或配置作为启动、管理或 Agent 执行的前置条件。
- **FR-029**: HiveGUI 必须提供完整的本地 Agent 执行能力，包括对话编排、直接调用用户配置的 LLM，以及在本地解析和执行 Tool、Skill、Function、Workflow、Plugin 调用链；任何执行失败不得回退到 HiveWeb。
- **FR-030**: 本地 Agent 的可用资源集合必须由显式分配给该 Agent 的 Tool/Skill 与所有 `is_always=true` 的 Tool/Skill 合并、去重得到；执行资源所需的 Capability 必须是该 Agent 显式分配 Capability 的子集，否则必须在执行前拒绝调用。
- **FR-031**: Plugin 导入时必须先校验 WASM 文件，计算 SHA-256 和大小，再通过已打开的 HiveGUI 托管插件根目录句柄逐段 no-follow 创建唯一 staging，并以 root-handle-relative 原子 no-replace 发布到从未复用的不可变唯一制品键；并发普通文件占用或身份替换也必须返回冲突，禁止覆盖、截断或采用竞争对象。加载和执行也必须相对同类受控根目录句柄逐段 no-follow 打开文件，在任何字节交给 runtime 前验证大小和 SHA-256，且只能把已验证、仍保持打开的文件句柄交给 runtime，不得再按 `s3_key` 重开路径。Unix symlink、hardlink、Windows junction/reparse point、device、FIFO、socket 或其他非普通文件/重定向链接出现在任一中间段或最终目标时必须拒绝；最终 WASM 必须是 link count 为 1 的普通文件。仅字符串规范化或 canonicalize 后重开路径不能作为安全边界，检查和使用之间不得留下 TOCTOU 替换窗口。字段、ABI、manifest、Capability 及制品内容预校验失败必须发生在任何 ledger 之前，使用户表、`plugin_artifact_operations` 与 `plugin_artifact_gc` 全部零修改。预校验成功后必须先耐久记录 `operation_kind=create|replace`、由 operation_id 确定性派生且唯一的 `staging_name` 与新 tuple 的 `prepared`，再排他创建 staging；完整写入、flush/fsync 和身份复核后先记录 `staging_identity`/`staged`，之后才可 no-replace 发布、fsync 父目录并记录 new identity/`published`。数据库 CHECK 必须要求 `prepared` 的两项 identity 均空、`staged` 仅 staging identity 非空、`published|referenced` 两项 identity 均非空，并要求 `done` 满足 `new_identity IS NULL OR staging_identity IS NOT NULL`、`conflict` 不限制两项 identity。create 的全部 expected-old 字段/revision 在所有状态始终为空，`plugin_id` 在 `prepared|staged|published` 为空，只有插入用户可见 Plugin 行、回填 plugin_id 与标记 `referenced` 的同一事务才能形成用户状态；replace 必须从 `prepared` 起携带 plugin_id、完整旧 tuple 与精确 `row_revision`，原始在线操作发布新不可变键后只以旧 tuple+revision 的 live CAS 事务切换引用、递增 revision 并标记 `referenced`，条件不匹配必须冲突，禁止 last-writer-wins。失败不得留下部分用户可见 Plugin 行或引用，但内部 operation/GC 恢复记录可以且必须按状态机耐久保留。启动重放必须先协调 `prepared|staged|published` 文件状态：`prepared` 且 staging 不存在时必须以单一 SQLite 事务标记 operation `done`；已出现但 identity 未耐久的 staging 必须 blocked 原样保留；`staged` 同时检查 staging/final 两名称，仅 staging 匹配时受保护清理、fsync 父目录并以单一事务标记 operation `done`；仅 final 与 staging identity/size/hash 匹配时必须重新 fsync 父目录、复验并耐久推进 `new_identity/published` 后才能分流；二者均无时以单一事务标记 operation `done`，双重存在、不匹配或身份不明时 conflict/blocked。`published` 只允许 staging 不存在且 final 精确匹配已持久化 new identity/size/hash；staging 重现、final 缺失/不匹配、new_identity 为空或双重存在均 blocked 并阻止开放 Plugin Store。进入 kind 分流后，create 只有在 Plugin 行已精确引用新 tuple 时确认完整提交；没有该行绝不能从 ledger 重建用户意图，必须在同一 SQLite 事务幂等登记本 operation 明确拥有的孤立新对象 GC 并把 operation 标记 `done`，实际删除不得发生在该事务前。replace 只有当前行已精确引用新 tuple 时确认提交；仍匹配旧 tuple/revision 时绝不得在重放中执行 live CAS，必须在同一 SQLite 事务登记本 operation 的新对象 GC 并把 operation 标记 `done`；其它歧义状态 blocked。`referenced` 已与用户 insert/live-CAS 同事务提交，是历史提交事实；后续合法 replace 可使当前行前进并由后继 GC 删除该 operation 的 new key，因此重放不得再要求当前行/new final 匹配或回滚/阻断 Store。create 可直接完成 operation；replace 必须在同一 SQLite 事务幂等登记旧 tuple GC 并把 operation 标记 `done`，实际 GC 与 operation 完成严格解耦且可独立长期 pending/blocked。新旧孤立制品进入持久 GC ledger，只有在无元数据引用、无运行时租约且文件身份、link count、大小和哈希仍匹配时才可受保护清理，无法证明安全时必须保留并标记 blocked。受保护 unlink 后必须 fsync 父目录再删除 GC ledger；若在 unlink+fsync 后、ledger 事务前崩溃，重放必须重新 fsync 父目录并确认目标仍不存在后才可幂等删 ledger，目标以任何 identity 重现时必须 blocked 且不得删除。
  前段中的 `blocked`/`conflict/blocked` 对 operation 只描述“阻断 Store”的效果，绝不新增 operation 状态；所有 ownership/identity/双重存在歧义都必须耐久写为 `plugin_artifact_operations.state=conflict` 并原样保留。`referenced` 后 staging 重现时不得回写历史 operation，可在同一事务执行 `operation→done` 并登记 blocked GC/incident 以保留未知对象。只有 `plugin_artifact_gc.state` 可取 `pending|blocked`：GC worker 必须在每次启动、固定周期以及引用或租约释放事件后按稳定 artifact_key 顺序扫描两种状态；引用/租约等瞬态条件消失且 identity 仍精确匹配时重新尝试删除，identity 重现/不匹配或所有权无法证明的条目持续 `blocked`，只能由显式人工处置解除。
- **FR-032**: 存在 Agent 时必须且只能有一个默认入口 Agent，该 Agent 必须是 depth=0 的根 Agent，所有正式用户新对话都从该 Agent 启动，公开运行时命令不得接受入口 Agent 覆盖。LLM 路由只能选择当前 Agent 的直接子 Agent；Agent 层级最大深度为 10，保存时必须拒绝静态父子循环或超深关系，运行时必须拒绝跨级路由、当前活动路径中的循环路由和任何超过深度限制的路由。
- **FR-033**: Workflow 保存和执行前必须验证节点键唯一、连线端点存在、DAG 无环、恰好一个 `start_node` 且至少一个 `end_node` 可从它到达；执行必须从本地持久化的 WorkflowNode/WorkflowEdge 加载定义。稳定 node_type 仅为 `start_node|end_node|function_node|generate_answer_node`，当前写入不得接受短名称别名。未被 Tool 引用时，删除 Workflow 必须级联删除其节点和连线；`tools.workflow_id` 必须以 `ON DELETE RESTRICT` 阻止删除被 Tool 引用的 Workflow，并在零修改下返回 `conflict { field: "id", reason: "referenced_by_tool", references }`，references 只能包含引用 Tool 的安全标识。删除被节点引用的 Function 必须被阻止。
- **FR-034**: 备份导出必须在打包前验证全部托管 Plugin 制品存在且 SHA-256 匹配，任一失败则不得产出备份包。恢复必须从已打开的受控暂存根目录句柄逐段 no-follow 解包，拒绝归档中的 symlink、hardlink、device、FIFO、socket、绝对路径、`..`、Windows junction/reparse point、暂存区预置链接/特殊文件及其他路径重定向；先在隔离暂存区验证包格式版本、schema 兼容性、JSON 完整性以及全部 WASM 的大小和 SHA-256，并从实体重建和验证当前版本的派生搜索索引。用户最终确认后必须立即冻结写入；current 和 staging 分别完成非 busy 的 `wal_checkpoint(TRUNCATE)`、关闭全部连接并分流 sidecar：已提交未 checkpoint WAL 必须完整合并，异常必须按 local-runtime 的合法 reason/artifact 配对和固定优先级返回精确 `storage_recovery_blocked`。hot/unknown/recoverable sidecar 保持 canonical 名称与字节且 Store 不开放。恢复数据库必须在建库前通过固定 `.hivegui-db-staging-v1/restore-{db_instance_operation_id}/.hivegui-db-instance-v1.json` 耐久登记六元组 manifest，数据库 basename 固定为 `datasources.db`。只有已证明安全的残留才可按 storage-migration 的确定性 v1 cleanup journal/quarantine 状态机处理；journal/quarantine 必须在 `applying` 前耐久删除，registry/live/tombstone instance 及 manifest/owner/retirement final/staging 则始终作为切换外部 locator/control state，不进入 archive、安全备份或待切换新树。随后自底向上 fsync staging 数据库、制品文件和目录，把 manifest 从 unarmed 原子推进 armed，通过固定 owner final/staging 发布 `prepared|applying`，再以同一文件系统内的原子 rename/swap 切换数据库与制品并 fsync 所有受影响父目录。新 current 必须在 owner `committed` 前通过完整 health/search/artifact/identity 验证；`committed` 是唯一系统提交点。启动固定按 `registry retirement/tombstone → live manifest/owner final/staging → current/live cleanup journal → owner 重放或 aborted retirement → Store` 重放；owner `prepared|applying` 时失败必须恢复并验证完整 old，`committed` 后只允许 outcome=`new` retirement 幂等收口已验证的 new。retirement 完成后才开放 Store。任何失败都不得产生混合状态、删除 hot/unknown/recoverable sidecar 或在受控根目录外读写。
  恢复 live instance 的 v1 manifest 必须含 `ownership_state=unarmed|armed`，预验证/用户确认前保持 unarmed 且 owner final/staging 均不存在；只有该组合可在六个 cleanup 槽收敛后派生 `aborted-pre-switch`。任何清理都须先耐久创建 outcome=`aborted_pre_switch` 的 registry-level retirement journal，再把整个 live instance identity-bound no-replace rename 到精确 tombstone；普通文件要求 link-count=1，目录只做 no-follow/identity-bound 逐层复核，owner/manifest 只在 tombstone 内逐叶删除。manifest armed 后 owner final 缺失/被删、staging-only、损坏或不匹配必须原样保留并 fail-closed，绝不能用 absence 推断孤儿。
  恢复切换前先把 manifest 原子推进 armed，再通过固定 `.hivegui-db-recovery-v1.json.staging` 发布 owner final；owner `prepared|applying` 重启恢复并验证完整旧状态，`committed` 只能在新 current 完整 health/search/artifact 验证后发布并成为唯一 commit point。terminal old/new 分别创建绑定 current identity/hash 的 retirement journal，再整目录移入 outcome tombstone。启动顺序精确为 `registry retirement/tombstone → live manifest/owner final/staging → current/live cleanup journal → owner 重放或 aborted retirement → Store`；retirement `prepared|renamed|done`、整目录 rename、tombstone 逐叶 unlink/rmdir、journal 删除及每次 fsync 都必须幂等重放。
- **FR-035**: 备份包不得包含设备本地加密密钥。恢复可以流式处理已经通过分块认证的内容，但敏感值只能在有界内存中短暂出现并必须立即使用目标设备本地密钥重新加密到隔离 staging 数据库。在验证认证流结尾及全部格式、路径、实体和制品预检成功前，不得向 UI、日志、current 或未加密暂存文件释放敏感明文。错误口令、后段篡改或认证失败必须保持 current 不变；只有 unarmed 且 owner 双槽均无的 live instance 可通过 outcome=`aborted_pre_switch` retirement 整目录移入 tombstone 后清理，任何 ownership/identity 歧义都必须保留并 fail-closed。
- **FR-036**: 恢复必须采用完整替换语义，不得执行记录级合并。验证待恢复备份成功后，系统只向用户展示计划使用的安全备份位置和完整替换影响；用户明确确认后必须立即冻结写入，并在同一不中断的冻结周期先对 current 与 staging 数据库完成非 busy checkpoint、关闭连接和 sidecar 验证，再从封闭的 current 生成和验证数据库与托管 Plugin 制品的最终安全备份，确保等待确认期间的合法写入也被包含，最后执行耐久切换。安全备份失败或用户取消时必须保持恢复前状态。owner 为 `prepared|applying` 时失败必须自动恢复并验证完整 old；切换后的新 current 必须先通过完整 health/search/artifact/identity 验证，之后才可发布唯一 commit point `committed`。`committed` 后失败必须保持写闸门关闭并通过 outcome=`new` retirement 幂等收口已验证的 new，不得声称已回滚；retirement 全部完成后才开放 Store。
- **FR-037**: Workflow 执行必须采用 fail-fast。任一节点失败或超时后，执行器必须停止调度尚未开始的节点；已在运行节点可完成并记录结果，但不得继续触发下游。Workflow 层不得自动重试节点或回滚已完成节点的外部副作用，最终错误必须包含失败节点、错误类别、耗时、已完成节点和未执行节点。
- **FR-038**: HiveGUI 启动时必须幂等注册 4 个 Builtin Function：`format_template`、`json_parse`、`json_stringify`、`text_regex_match`。这些 identifier 为系统保留，Builtin 记录不可编辑、删除或改为 custom/placeholder。点号名称 `format.template`、`json.parse`、`json.stringify`、`text.regex_match` 不得注册为记录或运行时别名，lookup/execute 必须失败。消息回复、结束对话和直接子 Agent 路由必须作为 Agent 运行时控制能力实现，不得创建 Tool 数据记录；除用户显式创建和分配的 Tool 外，不自动注册其他 Tool。
- **FR-039**: HiveWeb 与 HiveGUI 必须复用同一版本化 Extism Plugin ABI、manifest schema 和 `host_call` 协议。Plugin manifest 必须声明 ABI 版本和 required_capabilities；HiveGUI 必须在导入前验证 ABI 支持范围、manifest schema 和全部 Capability 可用性。验证全部通过时同一 WASM 不经转换即可执行；任一失败时必须在复制制品或写入数据前拒绝并返回完整不兼容列表。兼容检查不得请求 HiveWeb。
- **FR-040**: HiveGUI 必须分别测量 Agent 本地编排/路由开销、Tool 分派开销和 Workflow 调度开销，并将外部 LLM、网络等待及用户 Function/Plugin 执行耗时记录为独立区段。性能验收必须使用固定输入和固定 no-op 执行器，报告样本量、p50、p95、p99、测试环境和所比较的版本化基线；不得把外部耗时计入本地预算。任一跟踪百分位相对获批基线回归超过 10% 时必须阻断发布，除非存在明确签字并记录理由、影响范围和到期复核日期。
- **FR-041**: 每次本地 Agent 对话必须生成唯一 execution_id，并传播至 Agent、子 Agent、LLM、Tool、Function、WorkflowNode、Plugin 和 Capability 的结构化日志。日志必须使用 v1 固定字段/类型记录操作、实体 identifier、结果、稳定错误类别、可选 `cause_summary` 和分段耗时；`cause_summary` 必须在唯一处理边界中央脱敏、保持合法 UTF-8 且不超过 512 UTF-8 bytes，不得记录原始 cause、凭据、备份口令或默认保存完整 prompt、模型响应和 Tool 输入输出。写入只允许追加到 `.open` 活动段且每条 JSON 记录必须以换行完整结束；滚动时必须 flush、fsync 活动段，原子重命名为不可变 `.jsonl` 段并 fsync 父目录。启动恢复只能舍弃活动段末尾不完整的一条记录，诊断读取不得暴露半写记录。日志边界必须支持可注入时钟和持久化 retention high-watermark；有效时间取注入时钟与 high-watermark 较大值，high-watermark 前进必须通过同目录 staging、flush/fsync、原子 replace 和父目录 fsync 持久化。活动段按最早记录强制时间轮转，混合完成段通过同类耐久协议逐记录压缩，保证每条记录自 `occurred_at` 起实际不超过 7×24 小时，且时钟回拨不复活已过期记录。追加前必须按序列化后的完整记录字节预检容量；若单条记录自身超过 100,000,000 bytes，必须零写入拒绝，否则先耐久轮转/清理到追加后全部可见日志实际总计仍不超过该上限。任一 high-watermark、轮转、压缩、删除、replace 或父目录 fsync 失败时，诊断读取/导出必须 fail-closed。系统必须支持按 execution_id 导出脱敏诊断包；诊断日志不得自动进入数据备份包。
- **FR-042**: 任一后台任务运行期间，HiveGUI 从用户输入到可见反馈的 p95 必须 ≤ 100ms，主 UI 线程连续阻塞必须 ≤ 250ms，取消/停止控件必须始终可聚焦并可触发。停止请求触发后必须立即更新 UI 为“正在停止”或等价状态，底层任务随后执行安全终止。
- **FR-043**: HiveGUI 的首页导航、CRUD、搜索、分页、Agent 分配与对话、备份/恢复及 DAG 编辑关键流程必须可仅用键盘完成。所有交互控件必须具有可见焦点，并向平台无障碍树暴露名称、角色、状态和错误；模态框必须实施焦点陷阱并在关闭后恢复触发焦点。状态不得仅依赖颜色表达，普通文本对比度必须 ≥ 4.5:1，大字号文字、焦点指示器和关键 UI 图形对比度必须 ≥ 3:1。
- **FR-044**: schema 迁移前必须先对 source current 执行 `PRAGMA integrity_check`（精确返回唯一 `ok`）和 `PRAGMA foreign_key_check`（零行），冻结写入、完成非 busy 的 `wal_checkpoint(TRUNCATE)`、关闭全部连接并验证没有可恢复的 WAL/SHM/journal sidecar，再创建并验证 current 数据库与托管 Plugin 制品的安全快照；已提交未 checkpoint WAL 必须完整合并，全部异常按 local-runtime 的合法 reason/artifact 配对和固定优先级返回精确 `storage_recovery_blocked`。hot/unknown/recoverable sidecar 在 canonical 名称按字节保留且 Store 不开放；migration instance 必须在建库前以含 `ownership_state=unarmed` 的六元组 manifest 耐久登记于固定 `.hivegui-db-staging-v1/migration-{db_instance_operation_id}/.hivegui-db-instance-v1.json`，数据库 basename 固定为 `datasources.db`。安全残留只能通过 storage-migration 定义的确定性 v1 cleanup journal `prepared→quarantined→done` 五分支（含 `done` 收尾）与 identity-bound no-replace quarantine 协议耐久清理；损坏/重复/身份不明必须 fail-closed，journal 删除前不得快照或开放 Store。安全快照完成前失败时不得声称从尚不存在的快照恢复：必须保留 current 主文件、sidecar 与 cleanup journal/quarantine，保持 Store 关闭并只允许重放/重试或退出。快照验证完成后，必须把封闭 current 主文件复制到 unarmed migration instance；所有顺序迁移步骤只允许位于 staging SQLite 的同一事务边界内，并在该事务提交前对 staging 重复两项检查。SQLite commit 后必须在 staging 再次 checkpoint、关闭连接、按同一规则分流/重放 sidecar cleanup、fsync 主文件与父目录并重开验证；该 commit 不修改 current，也不是系统 commit point。只有 staging 全验证、manifest armed 且 owner `prepared|applying` 耐久后才可发布到 current；新 current 完整 health/search/artifact/identity 验证成功后才发布唯一系统 commit point `committed`。prepared/applying 失败恢复并验证 old，committed 后只通过 outcome=`new` retirement 收口已验证 new。旧备份格式必须在隔离暂存区逐步升级到当前格式，重建当前搜索索引并完成同样的目标健康检查后再提交恢复，且不得修改原备份包。
  前述迁移事务和 SQLite commit 只允许发生在从封闭 current 复制得到的 `.hivegui-db-staging-v1/migration-{UUID}/datasources.db`，绝不能原地修改 current；SQLite commit 不是系统 commit point。staging 在 commit 前后完成 checkpoint/sidecar/fsync/重开验证后，才把六元组 manifest 从 unarmed 推进 armed、发布 owner prepared、推进 applying 并按受控 rename 协议发布到 current。新 current 完整 health/search/artifact 验证成功后才发布 owner committed；此前失败恢复 old，之后只完成 new。aborted/old/new 都通过 registry-level retirement journal + 整 live instance tombstone rename 收口；armed owner 缺失、owner/manifest/retirement final/staging 歧义、unknown/hardlink/identity 竞态或 fsync 失败必须保留并阻断 Store。
- **FR-045**: 每次 Plugin 调用的默认限制必须为 timeout_ms=30000、memory_limit_mb=128（128MiB）、output_limit_bytes=10485760（10MiB）；用户可按 Plugin 设置正数覆盖值，但不得超过 120000、512（512MiB）、52428800（50MiB）。`memory_limit_mb` 仅为兼容既有 schema 保留字段名，逻辑单位固定为 MiB，64KiB WASM page 数按 `memory_limit_mb × 16` 计算。达到任一限制必须立即终止调用并返回稳定错误类别。Plugin 默认不得直接访问 WASI 文件系统、网络、环境变量或宿主进程资源，所有宿主资源访问必须通过当前 Agent 已授权的 Capability；资源额度调整不得改变隔离或授权结果。
- **FR-046**: HiveGUI 必须使用设备本地密钥加密持久化 ChatSession、ChatMessage、Tool 调用参数/结果和 AgentExecution 状态。默认会话保留期为 100 年，用户可修改保留期、删除单个会话或清空全部历史；删除必须级联到关联消息和执行状态，不得删除 Agent 配置。降低保留期或清空前必须显示受影响会话数量并确认。会话数据必须进入整体加密备份，但不得进入运行日志或诊断包。
- **FR-047**: 用户停止 Agent 执行时，HiveGUI 必须把协作取消信号传播到当前 Agent、子 Agent、LLM、Tool、Workflow 和 Plugin，停止调度新工作并取消可取消的网络请求。Plugin 必须获得最多 2 秒的协作终止宽限期，之后强制终止该 Plugin 实例且不得影响其他执行。最终 AgentExecution 必须持久化为 cancelled，记录已完成、被中断和未开始的步骤，并明确已完成的外部副作用不会自动回滚。
- **FR-048**: 所有用户输入、配置写入和文件导入必须在其进入 Store、运行时或导入服务的公开边界时执行“字段验证规则”定义的校验，不得只依赖 UI 校验。普通校验失败必须返回稳定的 `invalid_input { field, reason }`；唯一性冲突必须返回 `conflict { field, value }`，其中 value 仅允许安全字段值；引用或状态冲突必须返回不含 value 的 `conflict { field, reason, references }`，references 只能包含安全实体标识。密码和 token 不得回显。字段、ABI、manifest、Capability 等预校验失败必须发生在任何 durability state machine 之前，使用户数据与内部 operation/GC ledger 都零修改；进入合法 `prepared` 之后的文件持久化失败仍不得产生用户可见部分状态，但可以且必须保留契约规定的内部恢复记录。UI 必须保留用户输入并显示字段及原因。
- **FR-049** *(Session 2026-07-29 新增)*: HiveGUI 必须实施本地主密码认证。首次启动时强制用户在设置界面设置主密码（最低安全强度：长度 ≥ 12，混合大小写 + 数字 + 符号，且必须严格不等同于 `getpwuid(getuid()).pw_passwd` 同长同字符集），并通过 Argon2id（m=64MiB, t=3, p=1；派生耗时 < 5000ms；参数与 OWASP Password Storage Cheat Sheet 推荐基线一致）派生 wrapping KEK，将 FR-012 设备密钥材料以 ChaCha20Poly1305 包装持久化到 `.hivegui/keystore/wrapped_device_key.v1`（0600）；任何持久化或 fsync 失败必须保持设备密钥仅在内存并阻断进入主 UI。后续启动必须先显示解锁界面，密码字段不接受任何回显或剪贴板复制，最多 5 次连续错误后整应用 backoff 5 分钟并仅显示“从备份恢复” 入口；解锁成功后才把设备密钥材料解包到内存并按 FR-050 规则开始空闲计时。主密码与设备密钥材料相互独立：主密码错误只能让认证失败，绝不修改、删除或重新生成设备密钥或数据库。`keystore/` 目录不得进入任何形式的备份（FR-026 可移植包、本地回滚安全备份、staging/cleanup journal、owner/manifest/retirement 全部排除）；从备份恢复后**必须重新生成设备密钥**并以新主密码重新包装设备密钥材料（旧的 wrapped_device_key 物理字节保留仅用于审计），旧主密码不会随备份进入新设备。
- **FR-050** *(Session 2026-07-29 新增)*: HiveGUI 解锁后必须按 FR-007 `GlobalConfig` 中 `auth.auto_lock_minutes` 字段（默认 15，范围 `1..=1440`，0 视为非法）维持空闲计时；**"活动" 仅包含 keypress、主窗口 mousedown 与 UI 焦点变化**（mousemove 不重置计时以防误触；Agent 对话心跳、Tool 调度、Plugin 调用同样重置）。计时到点后必须立即将所有内存中已派生设备密钥材料、未提交会话/消息明文、未持久化敏感值清零（`zeroize` + 编译器屏障），UI 回到解锁界面，停止任何后台任务并持久化 `current_agent_execution.status=cancelled` 与 `chat_session.status=locked`。操作系统屏幕锁 / 锁屏事件（Windows `WM_WTSSESSION_CHANGE=0x7`、macOS `NSWorkspaceScreensDidSleep`、Linux `org.freedesktop.ScreenSaver ActiveChanged=true`）必须立即触发同样的锁定与内存清零；事件被禁用 / DBus 不可用 / 通知 API 不可用时 **fail-closed = 整应用立即锁定 + 提示"无法验证屏幕锁事件" + 阻断主 UI**（非静默忽略，也非"禁用自动锁定"）。锁定不影响持久化日志的写入（按 FR-041），但禁止任何日志字段携带明文密码、KEK、设备密钥或 session 明文。
- **FR-051** *(Session 2026-07-29 新增)*: HiveGUI 不得提供任何主密码重置、找回、旁路或安全问题流程。唯一可恢复路径是 FR-026/T129/T130 的可移植备份恢复。首次设置主密码时必须强制用户在 UI 看到“忘记主密码 = 只能从备份恢复” 风险说明并显式勾选确认；已存在历史 T129 备份时 UI 展示该备份的 SHA-256/创建时间并要求确认；不存在时必须立即引导用户跳到 T129 备份向导并完成首次导出后，才允许回到主 UI。备份恢复后用户必须按 FR-049 重新设置主密码并重新包装设备密钥材料；该过程必须独立 security review 并记录在 `checklists/security.md` 的“无密码重置” 边界。


### 关键实体

- **数据源配置（DataSource）**: 远程 MySQL 连接配置，包含名称、主机地址、端口号、用户名、加密密码。存储在本地 SQLite。
- **全局配置（GlobalConfig）**: 应用级配置项，包含名称（name）、键（key）、类型（type）、数据值（data）。存储在本地 SQLite。key 全局唯一，支持分页和搜索。
- **Model**: Preset 内的模型条目，引用一个 Provider。字段：id, name, preset_id (FK), provider_id (FK), priority, created_at, updated_at。
- **LLM Provider**: 独立的 LLM 后端连接配置。字段：id, name (UNIQUE), category, base_url, token_encrypted, token_env, created_at, updated_at。legacy kind/api_key_encrypted/api_key_env 仅在迁移中显式映射到 category/token_encrypted/token_env。
- **LLM Preset**: 顶层配置包，包含一组按优先级排序的 Model（fallback 链）和生成参数。字段：id, name (UNIQUE), description, is_default, max_tokens, temperature, created_at, updated_at。
- **Tag（标签）**: 字段：id, name, color (可选), created_at。
- **Category（分类）**: 字段：id, parent_id (可选 FK→自身), name, slug, description (可选), created_at, updated_at。支持树形层级。
- **Capability（能力）**: 字段：name (PK), description, is_dangerous, category_id (可选 FK→Category), created_at。
- **Plugin（插件）**: 字段：id, identifier (UNIQUE), name, description (可选), manifest (JSON，必须符合共享 schema 并包含 abi_version 与 required_capabilities), runtime, version, author (可选), repository_url (可选), s3_key（HiveGUI 托管目录内的相对制品键，非 S3 地址）, sha256, size_bytes, category_id (可选 FK→Category), timeout_ms (可选覆盖), memory_limit_mb (可选覆盖), output_limit_bytes (可选覆盖), row_revision（Store 派生、默认0、成功更新递增）, created_at, updated_at, deleted_at (可选)。
- **Function（函数）**: 字段：id, identifier (UNIQUE), name, description (可选), kind (`builtin|custom|placeholder`), input_schema (JSON), output_schema (JSON), plugin_id (可选 FK→Plugin), plugin_export (可选), category_id (可选 FK→Category), required_capabilities (可选 JSON), created_at, updated_at。Builtin 集合固定为 `format_template`、`json_parse`、`json_stringify`、`text_regex_match`，由系统幂等注册且只读，不提供点号别名。Placeholder 为 schema-only 且不可执行，plugin_id、plugin_export、required_capabilities 必须为空。
- **Workflow（工作流）**: 字段：id, identifier (UNIQUE), name, description (可选), timeout_ms, category_id (可选 FK→Category), input_schema (可选 JSON), start_description (可选), output_schema (可选 JSON), required_capabilities (可选 JSON), created_at, updated_at。包含一组 WorkflowNode 和 WorkflowEdge，构成可执行 DAG。
- **WorkflowNode（工作流节点）**: 字段：id, workflow_id (FK→Workflow), node_key, node_type (`start_node|end_node|function_node|generate_answer_node`), function_id (可选 FK→Function), position_x, position_y, node_config (JSON), created_at；(workflow_id, node_key) 联合唯一。
- **WorkflowEdge（工作流连线）**: 字段：id, workflow_id (FK→Workflow), src_node_key, dst_node_key, mapping (JSON)；同一 Workflow 内端点必须引用现有节点，且不得形成环。
- **Tool（工具）**: 字段：id, identifier (UNIQUE), name, description, kind (`function-wrap|workflow-wrap`), source (workspace|builtin), is_always, function_id (可选 FK→Function RESTRICT), workflow_id (可选 FK→Workflow RESTRICT), input_schema (JSON), output_schema (JSON), category_id (可选 FK→Category SET NULL), required_capabilities (可选 JSON), created_at, updated_at。
- **Skill（技能）**: 字段：id, identifier (UNIQUE), name, description, frontmatter (可选 JSON), content (markdown text), source, is_always, category_id (可选 FK→Category), required_capabilities (可选 JSON), created_at, updated_at。
- **Agent**: 字段：id, identifier (UNIQUE), name, description (可选), system_prompt, parent_agent_id (可选 FK→自身), depth（由层级自动计算，0..10）, is_default, model_preset (可选，引用 LlmPreset.name 的应用层外键), created_at, updated_at。存在 Agent 时 is_default 全局恰好一个且必须指向 depth=0 的根 Agent；非空 model_preset 必须存在，Preset 重命名时原子更新该字段，删除被引用 Preset 时 RESTRICT；通过 AgentTool、AgentSkill、AgentCapability 显式关联可用资源与权限。
- **AgentTool**: Agent 与 Tool 的多对多关联，字段：agent_id, tool_id；联合唯一。
- **AgentSkill**: Agent 与 Skill 的多对多关联，字段：agent_id, skill_id；联合唯一。
- **AgentCapability**: Agent 与 Capability 的多对多关联，字段：agent_id, capability_name；联合唯一。
- **ChatSession（会话）**: 字段：id (UUID), entry_agent_id (FK→Agent), current_agent_id (可选 FK→Agent), title_encrypted, status, execution_id, created_at, updated_at, expires_at；删除时级联删除消息和执行状态。
- **ChatMessage（消息）**: 字段：id, session_id (FK→ChatSession), role, content_encrypted, tool_calls_encrypted (可选), created_at；消息正文和 Tool 调用数据不得明文存储。
- AgentExecution（执行状态）: 字段：execution_id (UNIQUE), session_id (FK→ChatSession), current_agent_id (可选 FK→Agent), status（running|completed|failed|cancelled）, state_encrypted, started_at, finished_at (可选), error_kind (可选)；用于重启后恢复最终状态与调用链关联。
- **AuthKeystore（认证密钥库）**: 不进入 SQLite，位于 `.hivegui/keystore/wrapped_device_key.v1`（0600）。字段：`version=1`、`kdf="argon2id"`、`kdf_params={m,t,p,salt}`、`kek_alg="chacha20poly1305"`、`wrapped_device_key`(nonce||ct)、`kek_verifier`(Argon2id 派生 KEK 对固定常量 0 字节的 ChaCha20Poly1305 密文 + 12 字节 nonce，用于解锁界面密码正确性快速校验，**不暴露 KEK 本身**；比对失败时只返回 `invalid_password`，不允许返回区分"kek_verifier 不匹配" 与"wrapped_device_key 解包失败" 的错误码以避免侧信道)、`updated_at`；本文件被删除/损坏/权限放宽时 FR-012 与 FR-049 同步 fail-closed。**绝不允许**进入任何备份、可移植 archive、staging/cleanup journal、owner/manifest/retirement 槽位。
- **AuthLockState（认证锁定状态）**: 内存中持有，仅在解锁窗口存在；`unlocked_at`、`last_activity_at`、`auto_lock_minutes`(读自 `GlobalConfig.auth.auto_lock_minutes`)、`failure_count`(本进程连续错误次数，5 次后 backoff 5 分钟)。崩溃/退出前不持久化；锁定后立即 `zeroize`。


## 成功标准 *(必填)*

### 可衡量的成果

- **SC-001**: 用户启动 HiveGUI 后可直接看到首页，无需任何外部服务器配置。
- **SC-002**: 在固定验收数据集和100次操作样本中，数据源增删改查 p95 ≤ 1 秒并持久化到本地。
- **SC-003**: 连接测试在 5 秒内返回结果（超时视为连接失败）。
- **SC-004**: DataSource 密码、LLM Provider token、会话标题、消息正文、Tool 调用参数/结果和 AgentExecution 状态等全部公开敏感字段均不得以明文落盘。对每个字段执行公开写入/读取 roundtrip，并用唯一明文 canary 扫描 SQLite 主文件、WAL/SHM/journal、备份 staging/最终认证密文包、普通临时目录、结构化日志和诊断包；正常、错误、崩溃恢复和跨设备恢复路径的明文命中数必须均为 0。
- **SC-005**: 在 Agent、Workflow、Plugin、备份等后台任务运行期间，用户输入到可见反馈 p95 ≤ 100ms，主 UI 线程无连续超过 250ms 的阻塞，且所有测试时刻均可触发取消/停止控件。
- **SC-006**: 全局配置支持每页20条分页；在固定1万条验收数据集上，搜索和翻页 p95 ≤ 500ms。
- **SC-007**: 在固定验收数据集和100次操作样本中，LLM 配置（Preset/Provider/Model）CRUD p95 ≤ 1 秒并持久化。
- **SC-008**: 在各实体固定验收数据集和100次操作样本中，新增实体（Tag、Category、Capability、Plugin、Function、Workflow、Tool、Skill、Agent）CRUD p95 ≤ 1 秒并持久化。
- **SC-009**: 除 Category 外，所有新增实体的列表支持分页（每页 20 条）；在固定验收数据集上，1 字符、2 字符及至少 3 字符的中英文搜索词、ASCII 大小写变体，以及包含 `%`、`_`、引号和 FTS 操作符的字面量输入都返回精确的大小写不敏感任意位置包含结果，搜索和翻页 p95 ≤ 500ms。每类搜索都必须以 `EXPLAIN QUERY PLAN` 证明使用 short-gram 或 FTS5 trigram 索引；FTS 的 `VIRTUAL TABLE INDEX` 计为索引访问，禁止退化为非小型业务表的前导通配全表扫描。`hivegui-nfkc-casefold-v1` fixture 必须锁定规范化结果；非空输入规范化为空时 100% 返回 `empty_after_normalization`，搜索索引格式标识不匹配时 100% 在开放 Store 前失败或经显式迁移重建。
- **SC-010**: Category 使用一次批量查询加载完整树，搜索保留祖先路径；含 100+ 分类时从数据加载到树可见的 p95 ≤ 200ms，查询次数不随节点数量线性增长。
- **SC-011**: Plugin、Function、Workflow、Tool、Skill、Agent 的 identifier 唯一性约束均在数据库层面强制执行；对六类实体逐一提交重复 identifier 时，请求全部在同一次提交中返回 `conflict { field: "identifier", value }`、不产生新记录或部分关联，UI 保留输入并显示安全冲突值。
- **SC-012**: 在 HiveWeb 未配置、不可达或未运行时，HiveGUI 仍可启动、完成本地管理，并完成一次包含 LLM 调用和本地 Tool 执行的 Agent 对话；网络观测中不出现发往 HiveWeb 的请求。
- **SC-013**: 对任一 Agent，其运行时可用 Tool/Skill 集合与“显式分配项 ∪ `is_always=true` 项”完全一致；缺少所需 Capability 的调用全部在执行前被拒绝。仅持久化 Capability 元数据而未注册本地 handler 时运行时可用项数量保持不变且调用稳定失败；显式注册后按确定性顺序可见，重复注册稳定失败。`function-wrap|workflow-wrap` Tool 的序列化 roundtrip 必须保持 kind、互斥目标和 required_capabilities 顺序，并可在不加载 HiveGUI/HiveWeb 存储或网络组件时完成。
- **SC-014**: 100% 的 Plugin 导入、实例化和执行必须相对已打开的受控根目录句柄逐段 no-follow，并使用同一个已验证且仍保持打开的文件句柄读取托管 WASM；在任何字节交给 Extism/runtime 前完成路径边界、最终对象为 link count 1 的普通文件、大小和 SHA-256 校验。最终文件链接、中间目录链接、指向根目录外的链接、device/FIFO/socket 等特殊文件以及检查后并发替换的 TOCTOU 样例必须在 Unix symlink、hardlink 与可用平台的 Windows junction/reparse point 变体上全部拒绝，根目录外哨兵的读写次数必须为 0。导入后删除原始文件不影响执行；托管制品缺失或校验失败时不得实例化或执行。
- **SC-015**: 所有新对话均从唯一默认入口 Agent 启动；跨级、循环或深度超过 10 的路由请求全部在目标 Agent 执行前被拒绝。
- **SC-016**: 合法 Workflow DAG 在保存、重启后节点、连线、位置和配置保持一致并可在本地执行；含循环、悬空连线、非法 start/end 或重复节点键的 DAG 全部在执行前被拒绝。未被 Tool 引用的 Workflow 删除后其节点和连线数量均为 0；被 Tool 引用的 Workflow 删除请求必须返回 `referenced_by_tool` 冲突，安全列出引用 Tool，且 Workflow、节点、连线和 Tool 的前后快照完全一致。
- **SC-017**: 从备份包恢复到干净的 HiveGUI 后，所有实体、关联和 Plugin WASM 均可用且校验值与导出时一致，FTS5/short-gram 派生索引按 `hivegui-nfkc-casefold-v1` 重建并返回精确结果；对损坏、缺失制品、绝对路径、`..`、归档 symlink/hardlink/device/FIFO/socket、暂存区中间或最终 symlink/junction/reparse point/特殊文件、检查后并发替换、指向根目录外的链接以及不兼容版本的备份，100% 在修改现有数据前被拒绝，受控根目录外哨兵的读写次数为 0。
- **SC-018**: 使用正确备份口令可在不同设备恢复全部敏感配置并由目标设备正常解密；错误口令、被篡改备份以及缺少目标设备密钥写入条件的恢复尝试全部在修改现有状态前失败，日志和暂存文件中不出现备份口令或敏感明文。
- **SC-019**: 在已有数据的 HiveGUI 上恢复时，数据库与托管 Plugin 制品要么全部切换到备份快照，要么全部保持恢复前状态；等待确认期间的并发写入必须包含在确认后同一冻结周期重新生成的安全备份中。对固定 `.hivegui-db-staging-v1/{role}-{db_instance_operation_id}/datasources.db`、建库前六元组 v1 manifest、启动 ASCII 顺序 no-follow 发现/异常 fail-closed/locator 终态生命周期/全部内部文件排除、实例 UUID 与 cleanup UUID 分离、冻结写入、非 busy `wal_checkpoint(TRUNCATE)`、连接池关闭、已提交未 checkpoint WAL 与 hot/unknown/recoverable sidecar 分流、cleanup journal 的 UTF-8 db_id/长度前缀 token、精确 final/`.staging` 槽位、`schema_version=1`、排他创建/文件与父目录 fsync、identity-bound no-replace quarantine、`prepared|quarantined|done` 每次状态发布、五分支重放与 `done` 启动收尾、quarantine unlink/journal 删除及父目录 fsync、备份 staging 写入、同文件系统检查、原子 rename，以及 owner 每个 `prepared|applying|committed` 阶段、新 current 提交前完整验证、数据库/制品 rename/swap、retirement `prepared|renamed|done`、整 live instance 到 outcome tombstone 的 rename、tombstone 逐叶删除和每级 fsync 的每个边界执行失败或进程崩溃注入。每个 fixture 的 `storage_recovery_blocked` reason/artifact 必须符合固定配对和总优先级；hot/unknown/recoverable canonical 文件逐字节不变，安全残留只能完整位于 canonical/quarantine 或在 `quarantined` 后耐久删除。安全备份生成前的故障保持 current、cleanup journal 与写闸门关闭，不声称备份存在；安全备份已验证后必须可恢复到用户确认时的操作前状态。owner `prepared|applying` 时重启必须恢复并验证完整 old；新 current 完整验证后才可发布 `committed`，之后必须保持写闸门关闭并只收口已验证的 new；任何结果均不得丢失已提交 WAL 帧、把旧 sidecar 与新主文件组合或产生混合状态。
  验收还必须覆盖六元组 manifest unarmed→armed、manifest/owner final-only/final+staging/staging-only、armed 后 owner 丢失、owner `prepared|applying|committed`、新 current 完整验证前不得 committed，以及 retirement outcome=`aborted_pre_switch|old|new` 的 journal `prepared|renamed|done`、整目录 rename、tombstone 逐叶 unlink/每级 fsync/rmdir/journal 删除。每次重启只能恢复完整 old、完成完整 new、在匹配 retirement journal 下继续 tombstone 清理，或原样 fail-closed；current 数据和引用不得出现混合状态。
- **SC-020**: 对任一 Workflow 节点失败或超时，失败发生后不再启动新的节点，Workflow 层自动重试次数为 0；最终结果完整区分失败、已完成和未执行节点，并提示已完成外部副作用不会自动回滚。
- **SC-021**: 任意次数启动 HiveGUI 后，Builtin Function 集合始终且仅包含 4 个下划线 identifier，记录无重复且均不可修改或删除；四个点号名称的记录和运行时别名数量均为 0、lookup/execute 全部失败；任一 Placeholder 调用均在 Capability/Plugin 解析前以 `function_not_executable` 拒绝；Tool 存储中不存在消息回复、结束对话或子 Agent 路由控制记录。
- **SC-022**: 共享兼容性测试中的同一 Plugin WASM 与 manifest 在 HiveWeb 和 HiveGUI 上产生相同 ABI 输出；所有 ABI 版本不兼容、manifest 无效或 Capability 缺失的样例均在 HiveGUI 写入数据或复制制品前被拒绝，且网络观测中不出现 HiveWeb 请求。
- **SC-023**: 在固定性能基准中，Agent 编排/路由本地开销 p95 ≤ 200ms，Tool 分派开销 p95 ≤ 50ms，100 节点 no-op Workflow 的调度开销 p95 ≤ 100ms；报告必须单列外部 LLM、网络等待和用户代码执行耗时，不得以其延迟作为本地预算超标的豁免。每项报告必须引用版本化基线和环境指纹；p50、p95 或 p99 任一项相对基线回归超过 10% 且没有明确签字、记录理由、影响范围和到期复核日期时，验收失败。
- **SC-024**: 任一次本地 Agent 测试执行产生的 Agent、LLM、Tool、Function、WorkflowNode、Plugin 和 Capability 日志均可通过同一 execution_id 关联；v1 字段/类型完全匹配契约，`cause_summary` 不超过 512 UTF-8 bytes，原始 cause、凭据、备份口令和完整输入输出的泄漏测试结果为 0。对活动段记录写入、时间/容量轮转、flush/fsync、重命名、过期记录重写和父目录 fsync 各边界注入崩溃后，重启只能丢弃末尾不完整的一条 JSON，所有可见 `.jsonl` 记录都必须是完整换行 JSON 且不得重复；使用注入时钟时任何记录从 `occurred_at` 起不得保留超过 7×24 小时，活动段不得豁免，实际文件总量也不得超过 100,000,000 bytes，时钟回拨不复活已过期记录，导出的诊断包只包含脱敏记录。
- **SC-025**: 在同时运行 Agent 对话、100 节点 no-op Workflow 和备份预验证的 UI 响应测试中，输入反馈和主线程阻塞均满足 SC-005，停止控件可用率为 100%。
- **SC-026**: 自动化与人工无障碍测试可仅用键盘完成全部关键流程及 DAG 节点/连线操作；所有模态框通过焦点陷阱与焦点恢复测试，关键控件的名称、角色、状态和错误均可从平台无障碍树读取，颜色对比度测试全部满足 FR-043。
- **SC-027**: 当前版本对当前减 1、减 2 的 schema 和备份格式样例全部可逐步升级并保持实体、关联、敏感值与 Plugin 制品一致；迁移矩阵必须覆盖 Function 整数 `1/2/3`、四个稳定 `*_node` 值、旧点号 Builtin 到下划线名称的事务重命名，以及 v3→v4 搜索索引建表、`hivegui-nfkc-casefold-v1` 回填和一致性验证，未知 kind、名称碰撞或 normalization/index version 不匹配必须整体回滚或进入显式迁移。已有、新建和迁移目标数据库必须分别以结构损坏样例和在 `foreign_keys=OFF` 时制造的孤儿外键样例验证两项健康检查；WAL fixture 必须证明 checkpoint/关闭/sidecar 每个失败边界都不丢失已提交帧。任一失败都不得进入主界面或提交迁移。对迁移各步骤注入失败后数据库保持原版本、主界面不可进入且安全快照可用；新于当前或早于当前减 2 的版本全部被明确拒绝。
- **SC-028**: Plugin 默认与最大资源限制的边界测试全部生效，timeout、memory 和 output 超限调用 100% 被终止且不阻塞主 UI；未授权的直接 WASI 文件、网络和环境变量访问 100% 被拒绝，调高资源限制不改变拒绝结果。
- **SC-029**: 默认配置下新会话的 expires_at 对应创建时间后 100 年；重启后保留期内的会话、消息、execution_id 和执行状态可完整恢复，数据库明文扫描不出现消息或 Tool 调用内容。删除/清空操作级联清除全部关联历史且不影响 Agent 配置，诊断包中会话内容泄漏数为 0。
- **SC-030**: 在 Agent、LLM、Tool、Workflow 和 Plugin 各执行阶段触发停止的测试中，停止后新工作调度数为 0，可取消网络请求均被取消，不协作退出的 Plugin 在 2 秒宽限期后被终止；AgentExecution 最终状态均为 cancelled，其他会话和主 UI 继续可用，结果中完整列出已完成、被中断和未开始的步骤及外部副作用提示。
- **SC-031**: 表驱动验收测试覆盖“字段验证规则”中的全部公开可写字段，且验证目录字段集合必须与公开写入 DTO 字段集合完全相等（仅排除明确标记的 Store 派生字段和受信迁移字段）；覆盖 identifier、name、slug、description、JSON/Capability 数组、color、外键/ID、布尔/枚举、DataSource、GlobalConfig、LLM 配置、Plugin/Workflow/Node/Edge、Function/Tool/Skill/Agent、分页/搜索和本地运行时命令的合法边界、越界及格式错误，并逐项覆盖三种 Function.kind、四种 `*_node`、点号 Builtin 负例和全部未知枚举。所有非法样例通过公开 Store、运行时或导入接口提交时均在数据库修改前返回 `invalid_input { field, reason }`；唯一性冲突返回 `conflict { field, value }` 且 value 仅限 identifier、name、slug、key 等安全值；引用或状态冲突返回不含 value 的 `conflict { field, reason, references }`。密码、token、消息正文和其他机密值只返回字段名与脱敏原因；数据库和 UI 表单数据均保持不变。
- **SC-032**: 首次启动生成的设备密钥权限仅限当前用户，重启后复用同一密钥；删除、损坏或放宽密钥权限的所有测试均阻止敏感数据解密且不修改数据库、不静默生成替代密钥，并显示重新配置或从备份恢复入口。
- **SC-033** *(Session 2026-07-29 新增)*: 全新首次启动必须强制进入主密码设置界面，弱密码（含长度 <12、缺少大小写/数字/符号、与系统 PWD 长度相等且字符集完全相同）100% 拒绝；设置成功后主密码经 Argon2id 派生 KEK，设备密钥材料以 ChaCha20Poly1305 包装到 `.hivegui/keystore/wrapped_device_key.v1`（0600），文件 fsync 与父目录 fsync 失败、权限被放宽（<0600）或文件不存在/不可读时主 UI 不可进入。重启后必须先显示解锁界面，密码字段不接受任何回显，5 次错误后整应用 5 分钟 backoff 且仅显示从备份恢复入口；正确密码可成功解包设备密钥并进入主 UI。`keystore/` 路径不出现在任何备份包、本地回滚安全备份、staging/cleanup journal、owner/manifest/retirement 槽位的物理文件清单中。
- **SC-034** *(Session 2026-07-29 新增)*: 在解锁状态下，连续 15 分钟（按 `GlobalConfig.auth.auto_lock_minutes`）无键盘/鼠标活动、Agent 对话心跳、Tool 调度、Plugin 调用的事件时，应用自动锁定、内存中已派生设备密钥材料与所有未持久化敏感明文全部清零、UI 回到解锁界面、当前 AgentExecution 持久化为 cancelled、ChatSession.status=locked。操作系统屏幕锁/锁屏事件必须立即触发相同锁定与清零。`auth.auto_lock_minutes` 取值范围 `1..=1440`；0 或越界值使保存返回 `invalid_input`，主 UI 始终未解锁前不得出现该字段。锁定后任何对 SQLite 主文件、WAL/SHM、备份 staging、诊断包的明文 canary 扫描命中数为 0。
- **SC-035** *(Session 2026-07-29 新增)*: 首次设置主密码完成后、进入主 UI 之前，UI 必须强制展示“忘记主密码 = 只能从备份恢复” 风险说明并要求用户勾选确认；若不存在 T129 备份文件，必须先引导用户完成 T129 备份向导并验证备份文件存在后才能进入主 UI。恢复备份后必须强制用户重新设置主密码并重新包装设备密钥材料。`GlobalConfig`、`datasources.db`、`.hivegui/keystore/`、`.hivegui-db-*`、`plugin_artifacts/` 的物理文件清单与 SC-033 一致；主密码错误 5 次后整应用 5 分钟 backoff 期间不能从“解锁”入口绕过。

## 假设条件

- HiveGUI 是面向管理员的桌面工具，运行在 Linux/macOS/Windows 桌面端。
- 用户自行管理目标 MySQL 服务器的网络可达性。
- HiveGUI 是完整的本地 Agent，不连接或请求远程 HiveWeb/HiveClaw 服务；它可按用户配置直接访问外部 LLM、MySQL 等资源，这些资源不构成 HiveWeb 依赖。
- 界面仅支持中文。
- 数据源密码使用本地加密存储。
