# Data Model: HiveGUI 独立本地 Agent

**Feature**: `011-hivegui-standalone-mode`
**Date**: 2026-07-22
**Storage**: HiveGUI 本地 SQLite + HiveGUI 托管文件目录

## 1. 存储边界

- SQLite 是 HiveGUI 配置、关系、会话和执行状态的唯一关系型真值源。
- Plugin WASM 存放在 HiveGUI 数据目录下的托管目录；数据库中的 `plugins.s3_key` 是安全、不可变唯一对象的相对制品键，不是 S3 地址。新导入以 no-replace 发布，更新先发布新键再事务切换引用，旧对象仅在无引用且身份绑定验证通过时清理；不得覆盖或删除并发创建/替换的普通文件。制品导入、加载、执行、备份和恢复必须相对已打开的受控根目录句柄逐段 no-follow，拒绝任一中间段或最终目标中的 Unix symlink、hardlink、Windows junction/reparse point、device、FIFO、socket 和其他非普通文件/重定向链接；最终制品必须是 link count 为 1 的普通文件。验证后的文件必须通过仍保持打开的句柄交给消费者，不得按字符串路径重开。
- 设备密钥保存在数据库目录外的本地密钥文件中；首次启动使用系统安全随机源原子创建，Unix 权限为 0600，其他平台使用仅当前用户可读写的等效 ACL。已有密文时，密钥缺失、损坏、权限不安全或不可读属于阻断状态，不得静默生成替代密钥或修改数据库。
- 密码、API Key、会话内容、Tool 调用数据和执行状态以密文 BLOB 入库。
- 运行诊断通过可注入 UTC Clock 的公开边界只追加固定 v1 schema 到 `.open` 活动 JSONL 段，每条记录以换行完整结束；原始 cause 经中央脱敏为最多 512 UTF-8 bytes 的 `cause_summary: string|null`。滚动协议为文件 flush/fsync → 原子重命名为不可变 `.jsonl` → 父目录 fsync；持久化时间 high-watermark、按最早记录强制轮转与崩溃安全逐记录压缩保证每条记录实际不超过 7×24 小时，全部可见日志实际总计不超过 100,000,000 bytes。启动恢复只可丢弃活动段末尾不完整的一条记录。日志不复制会话正文，不属于 SQLite 实体，也不进入数据备份包。
- HiveWeb 的 MySQL、Redis、Rustfs 和 HTTP API 均不在本模型边界内。

每次打开已有 SQLite、创建新 SQLite 以及迁移事务提交前，都调用同一个健康检查：`PRAGMA integrity_check` 必须精确返回唯一 `ok`，`PRAGMA foreign_key_check` 必须返回零行。连接上的 `PRAGMA foreign_keys=ON` 仍然必需，但不是健康检查的替代品。

SQLite 业务连接使用 WAL，但任何主文件快照、迁移后发布或恢复切换必须先冻结写入、在独占连接执行非 busy `wal_checkpoint(TRUNCATE)`、关闭全部连接，再区分可安全清理与 hot/未知/仍含可恢复状态的 WAL/SHM/rollback-journal sidecar，随后 fsync 主文件和父目录。已提交未 checkpoint WAL 必须完整合并；hot/unknown/recoverable sidecar 必须稳定报错、在 canonical 名称按字节保留且 Store 不开放，绝不能盲删。已证明安全的残留只能通过下述 SQLite 外部 cleanup journal 移入同目录 quarantine 并耐久清理；禁止只处理主文件而遗漏已提交 WAL 帧。

所有实体写入、运行时配置和文件导入必须在公开 Store/运行时/导入边界执行 spec“字段验证规则”；UI 校验只提供即时反馈，不能替代边界校验。普通验证失败返回 `invalid_input { field, reason }`；唯一性冲突返回 `conflict { field, value }` 且 value 仅允许安全字段值；引用或状态冲突返回不含 value 的 `conflict { field, reason, references }`，references 只含安全实体标识。密码、token、消息正文等机密字段只返回字段名和脱敏原因。字段/ABI/manifest/Capability 等预校验失败必须在进入任何 durability state machine 前保证用户表和内部 operation/GC ledger 均零修改；一旦文件操作已合法持久化 `prepared`，后续失败仍须保证用户可见实体事务零部分修改，但允许并要求内部恢复 ledger 按状态机耐久保留。分页、搜索、URL、环境变量名、UUID、运行时消息和 retention_filter 与 CRUD 字段使用同一边界错误模型。

公开写入字段目录必须与 spec 同步并由表驱动测试逐项覆盖，且其字段集合必须与公开写入 DTO 字段集合完全相等（仅排除明确标记的 Store 派生字段和受信迁移字段）：用户或导入包提供的整数 ID 必须为正且引用现有记录，可选 ID 只能为空或合法目标；Model.priority 为 0..=1000000；Plugin.runtime 固定为 `extism`、version 为 1..=64 字符、author 为空或最大 255 字符、repository_url 为空或为最大 2048 字符的绝对 HTTP/HTTPS URL、s3_key 为安全相对键、sha256 为 64 位小写十六进制、size_bytes 与已校验制品实际大小一致；Workflow.timeout_ms 为 1..=120000 且 start_description 最大 1MiB；WorkflowNode.type 只能是 `start_node|end_node|function_node|generate_answer_node`，其键、有限坐标和 node_config 以及 WorkflowEdge.mapping 必须满足字段规则；Function.kind 只能是 `builtin|custom|placeholder`，Tool kind 和 Tool/Skill source 只能使用各自稳定枚举，custom Function.plugin_export 为 1..=255 字符，placeholder Function 的 plugin_id/plugin_export/required_capabilities 必须为空，Skill.content 最大 1MiB；`required_capabilities` 和 manifest 内同名字段必须是已知、去重 Capability 名称的 JSON 字符串数组；Agent.system_prompt 去除首尾空白后非空且最大 1MiB。所有枚举、`is_default`/`is_always`/`is_dangerous` 等布尔值、关联集合和恢复输入时间戳也必须在边界校验，派生 ID、depth 和时间戳不接受普通 UI 覆盖。

### 1.1 SQL 解释器边界类型

- HiveGUI SQLite 与 HiveWeb 固定应用 schema 查询使用 SQLx `query!`/`query_as!`/`query_scalar!` 及 offline metadata；值只通过这些宏的 bind 参数进入查询。有限的条件、表名或列名差异由封闭 enum 和穷尽 `match` 选择有限组静态、编译期检查的查询。生产代码禁止 SQLx `QueryBuilder`、运行时 SQL 字符串和任意动态标识符。
- HiveGUI 外部 MySQL 浏览器使用 `mysql_async` prepared statement 绑定值；数据库名、表名和列名必须先与当前连接加载的服务器 metadata 精确匹配，再由唯一 `MysqlIdentifier` 按上下文序列化。该类型不能接受未经验证的文本。
- HiveWeb 应用 schema 的乐观锁表名使用独立的封闭 enum，不接收任意 `&str`，也不复用 `MysqlIdentifier`。`game_service.category_name` 是普通值，只能绑定到静态 `JSON_OBJECT('name', ?)`；恶意字符串不得改变 SQL 结构。
- HiveWeb 启动时加载的 `named_queries.toml` 完整 SQL 只经一个 reviewed-config 中央边界。边界拒绝多语句、SQL 注释、placeholder/参数不匹配、重复参数和 select/execute kind 不匹配后，才可构造 SQLx `AssertSqlSafe`；这是生产代码中唯一的该类型所有者。

HiveWeb 的后两项仅描述同 workspace 云端产品自身的安全迁移，不属于 HiveGUI 数据实体或运行依赖。HiveGUI 不导入、请求或回退 HiveWeb。

## 2. 表与关系总览

```text
LlmPreset 1--* Model *--1 LlmProvider

Category 1--* Category
Category 1--* Capability / Plugin / Function / Workflow / Tool / Skill
Plugin 1--* Function
Workflow 1--* WorkflowNode
Workflow 1--* WorkflowEdge
Function 1--* WorkflowNode
Function 1--* Tool OR Workflow 1--* Tool

Agent 1--* Agent
Agent *--* Tool       through AgentTool
Agent *--* Skill      through AgentSkill
Agent *--* Capability through AgentCapability

Agent 1--* ChatSession 1--* ChatMessage
ChatSession 1--* AgentExecution
```

## 3. 基础配置实体

### DataSource (`data_sources`)

| 字段 | 类型 | 约束 |
| --- | --- | --- |
| id | INTEGER | PK, autoincrement |
| name | TEXT | NOT NULL, UNIQUE |
| host | TEXT | NOT NULL |
| port | INTEGER | NOT NULL, 1..65535, default 3306 |
| username | TEXT | NOT NULL |
| encrypted_password | BLOB | NOT NULL, 设备密钥加密 |
| created_at / updated_at | TEXT | UTC RFC3339 |

### GlobalConfig (`global_configs`)

| 字段 | 类型 | 约束 |
| --- | --- | --- |
| id | INTEGER | PK |
| name | TEXT | NOT NULL |
| key | TEXT | NOT NULL, UNIQUE |
| type | TEXT | NOT NULL |
| data | TEXT | NOT NULL |
| created_at / updated_at | TEXT | UTC RFC3339 |

保留配置键 `conversation_retention`，值为带单位的正数对象，例如 `{"value":100,"unit":"years"}`；默认值为 100 年。创建会话时据此物化 `expires_at`，修改配置不会偷偷改写既有会话，用户确认清理后才删除新规则覆盖的历史。

### LlmPreset (`llm_presets`)

字段：`id`, `name UNIQUE`, `description`, `is_default`, `max_tokens`, `temperature`, `created_at`, `updated_at`。最多一个 `is_default=1`，通过部分唯一索引与事务共同强制。

### LlmProvider (`llm_providers`)

字段：`id`, `name UNIQUE`, `category`, `base_url`, `token_encrypted BLOB`, `token_env`, `created_at`, `updated_at`。`category` 必须由本地 provider registry 支持；`base_url` 为空或为绝对 HTTP/HTTPS URL；`token_encrypted` 使用设备密钥加密；`token_env` 只保存环境变量名且非空时优先于 token。legacy `kind/api_key_encrypted/api_key_env` 仅在迁移中显式映射到 `category/token_encrypted/token_env`。

### Model (`models`)

字段：`id`, `name`, `preset_id FK→LlmPreset CASCADE`, `provider_id FK→LlmProvider RESTRICT`, `priority`, `created_at`, `updated_at`。preset_id/provider_id 必须引用现有记录，priority 范围为 0..=1000000；`(preset_id, priority, id)` 决定 fallback 顺序。

## 4. Agent 资源目录实体

### Tag (`tags`)

字段：`id`, `name UNIQUE`, `color`, `created_at`。`color` 为空或匹配 `^#[0-9A-Fa-f]{6}$`。

### Category (`categories`)

字段：`id`, `parent_id FK→Category SET NULL`, `name`, `slug UNIQUE`, `description`, `created_at`, `updated_at`。保存时拒绝自身引用和祖先循环；存在直接子分类时拒绝删除。Category 不使用普通列表的20条分页，Store 必须在一次批量查询中加载完整树；搜索返回匹配节点及其祖先路径，不得按节点追加查询。

### Capability (`capabilities`)

字段：`name TEXT PK`, `description`, `is_dangerous`, `category_id FK→Category SET NULL`, `created_at`。该表只保存可管理元数据，插入同名记录不得安装或合成本地 handler。运行时可用列表由 `hive-runtime-core` 显式注册且当前进程实际持有的 handler 决定；仅声明、未知、重复注册或未授权调用返回稳定错误，枚举按稳定名称顺序输出。核心注册表不依赖 HiveGUI/HiveWeb 的数据库、HTTP 或其他产品传输类型；未知名称不能进入 AgentCapability 或资源声明。

### Plugin (`plugins`)

| 字段 | 类型 | 约束 |
| --- | --- | --- |
| id | INTEGER | PK |
| identifier | TEXT | NOT NULL, UNIQUE |
| name | TEXT | NOT NULL |
| description | TEXT | nullable |
| manifest | TEXT | 有效 JSON；含 `abi_version`、`required_capabilities` |
| runtime | TEXT | 必须为 `extism` |
| version | TEXT | NOT NULL, 1..64 字符 |
| author / repository_url | TEXT | nullable；repository_url 为空或为最大 2048 字符的绝对 HTTP/HTTPS URL |
| s3_key | TEXT | 托管目录内安全相对键 |
| sha256 | TEXT | 64 位小写十六进制 |
| size_bytes | INTEGER | > 0 |
| category_id | INTEGER | FK→Category SET NULL |
| timeout_ms | INTEGER | nullable, 1..120000；空值=30000 |
| memory_limit_mb | INTEGER | nullable, 1..512 MiB；空值=128 MiB；字段名为 schema 兼容保留 |
| output_limit_bytes | INTEGER | nullable, 1..52428800 bytes；空值=10485760 bytes（10MiB） |
| row_revision | INTEGER | NOT NULL DEFAULT 0；Store 派生的单调并发令牌，每次 Plugin 元数据或制品引用成功修改时 +1，不接受用户直接写入 |
| created_at / updated_at | TEXT | UTC RFC3339 |
| deleted_at | TEXT | nullable，软删除 |

`s3_key` 不得是绝对路径，不得包含空组件、`.`、`..` 或链接逃逸，并且每次导入都必须分配从未复用的不可变唯一键。新导入相对于同一受控父目录句柄原子 no-replace 发布；更新发布新对象后必须用 `WHERE id=? AND s3_key=? AND row_revision=?` 的等价条件事务切换 `s3_key` 并把 `row_revision` 精确加一，受影响行数不是 1 时返回并发冲突，不得原地改写或 last-writer-wins。所有路径段必须相对已打开的托管根目录句柄逐段 no-follow 解析；Unix symlink、hardlink、Windows junction/reparse point、device、FIFO、socket 或其他非普通文件/重定向链接出现在中间段或最终目标时均拒绝，最终制品必须是 link count 为 1 的普通文件；并发创建或身份替换的普通文件也必须冲突。仅字符串规范化或 canonicalize 后按路径重开不能满足边界；导入、读取和执行必须消除检查与使用之间的 TOCTOU 窗口。`sha256` 必须匹配 `^[0-9a-f]{64}$`，`size_bytes` 必须大于 0。Plugin 导入、读取和执行前通过仍保持打开的受控文件句柄重新计算实际大小与 SHA-256，任一项不匹配即拒绝；runtime 只能接收该已验证句柄，不得按 `s3_key` 重开。旧对象只可在无人引用且路径仍绑定同一普通文件身份、link count 与哈希时受保护删除，否则保留待幂等 GC。`memory_limit_mb` 转换为 64KiB WASM page 时使用 `memory_limit_mb × 16`；默认128MiB=2048 pages，硬上限512MiB=8192 pages。

### PluginArtifactOperation / PluginArtifactGc（内部耐久状态）

`plugin_artifact_operations` 不是用户实体，也不进入备份包；字段至少包含 `operation_id UUID PK`、`operation_kind (create|replace)`、`target_identifier`、`plugin_id nullable`、`expected_old_s3_key nullable`、`expected_old_sha256 nullable`、`expected_old_size_bytes nullable`、`expected_old_identity nullable`、`expected_row_revision nullable`、`staging_name UNIQUE`（由 operation_id 确定性派生）、`staging_identity nullable`、`new_s3_key UNIQUE`、`new_sha256`、`new_size_bytes`、`new_identity nullable`、`state (prepared|staged|published|referenced|done|conflict)`、`created_at`、`updated_at`。状态 CHECK 必须精确等价于：`prepared` 两项 identity 均空；`staged` 的 staging identity 非空且 new identity 为空；`published|referenced` 两项均非空；`done` 满足 `new_identity IS NULL OR staging_identity IS NOT NULL`，从而只允许 none/staging-only/both 三种既有阶段形状；`conflict` 不限制这两列以原样保留观测字段。`create` 的全部 `expected_old_*`/revision 在所有状态始终为空，`plugin_id` 在 `prepared|staged|published` 为空且在 `referenced` 非空，插入 Plugin 行和写回 `plugin_id`/`referenced` 必须在同一事务；`replace` 的 plugin_id、全部旧值和 revision 始终非空。在创建 staging 前先持久化包含 staging_name 的 `prepared`；staging 完整写入、flush/fsync 并从仍打开句柄验证后，必须先持久化 `staging_identity` 与 `staged`，才可发布新键；发布及父目录 fsync 后记录新文件身份并持久化 `published`；用户可见 Plugin 插入或 live CAS 与状态 `referenced` 同事务提交。

启动开放 Plugin Store 前必须按 kind 重放所有非终态 operation。`prepared` 且 staging 不存在时以单一 SQLite 事务标记 `done`；staging 已出现但 `staging_identity` 未耐久时把 operation 置 `conflict`、原样保留并阻断 Store。`staged` 同时检查 `staging_name` 与 `new_s3_key`：仅 staging 与记录 identity/size/hash 匹配时身份绑定删除、fsync 并完成 operation；仅 final 与 staging identity/size/hash 匹配时先重新 fsync、复验并耐久推进 `new_identity/published`；二者均无时完成 operation；双重存在、任一不匹配或身份无法证明时置 `conflict` 并保留全部对象。`published` 只接受 staging 不存在且 final/new identity/size/hash 精确匹配；其它组合置 `conflict` 并阻止开放 Store。仅 final 匹配不得直接修改用户行：create 只确认精确引用 new tuple 的完整提交，无用户行或同 identifier 指向其它 tuple 时只以“登记 owned new object GC + operation→done”同一事务收敛；replace 当前仍精确匹配 old tuple/revision 时不得重做 live CAS，也采用同一登记事务，只有当前已引用 new tuple 才确认提交。其它引用/revision/identity 歧义置 `conflict`。`referenced` 是历史提交事实；create 完成 operation，replace 以“登记旧 tuple GC + operation→done”同事务完成。其 staging 名重现时不得回写历史 operation，只登记 GC/incident 并保留对象。operation 永远只允许 `prepared|staged|published|referenced|done|conflict`，所有歧义都落 `conflict`。

`plugin_artifact_gc` 同样不进入备份包；字段至少包含 `artifact_key PK`、`expected_sha256`、`expected_size_bytes`、`expected_identity`、`source_operation_id`、`state (pending|blocked)`、`attempts`、`last_error`、`created_at`、`updated_at`。登记 GC 与 source operation→`done` 必须同一 SQLite 事务，operation 完成不等待实际删除。GC 每次删除前都须确认没有任何 Plugin 元数据引用、没有当前进程运行时句柄/实例租约，并通过受控目录句柄证明路径仍绑定相同普通文件身份、link count=1、大小和哈希；任一条件不满足就保留对象并标记 `blocked`。worker 在每次启动、固定周期以及引用/租约释放事件后按 `artifact_key` 稳定顺序扫描 `pending|blocked`；引用或租约等瞬态条件已消失且 identity 仍精确匹配时必须重试，identity 重现/不匹配或所有权无法证明时持续 blocked，除显式人工处置外不得采用新 identity。删除成功后 fsync 父目录，再以事务只删除 GC ledger；若崩溃发生在 unlink+父目录 fsync 后、ledger 删除前，重放必须重新 fsync 父目录并确认目标仍不存在后幂等删除 ledger。目标以任何 identity 重现时不得删除，必须 blocked。崩溃后重复执行不能删除竞争者或未知对象。

### SidecarCleanupJournal（SQLite 外部内部文件）

该 journal 位于所控制数据库同目录，不是 SQLite 表、用户实体或备份内容。current 固定为数据根句柄下 `datasources.db` 且 `db_id=current`；migration/restore live instance 固定为 `.hivegui-db-staging-v1/{role}-{db_instance_operation_id}/datasources.db`，db_id 固定 `migration/{UUID}|restore/{UUID}`。role/UUID/name 均按 storage-migration 合同的精确 ASCII 语法解释，不做 normalization/case-fold/canonicalize。建库前先发布六元组 v1 manifest：schema version、role、UUID、db_id、database_name、`ownership_state=unarmed`；唯一状态转换为 `unarmed→armed`，manifest final basename 固定为 `.hivegui-db-instance-v1.json`，staging basename 固定为 `.hivegui-db-instance-v1.json.staging`。

对 db_id/artifact 的 `db_token`、三个 final/三个 staging cleanup-journal 槽、独立且不复用的 `cleanup_operation_id`、quarantine basename、identity/size/SHA-256 与 `prepared|quarantined|done` 字段完全按 storage-migration 合同。live/tombstone instance、manifest/owner/retirement final/staging、cleanup journal/quarantine 都是外部 locator/control state，不进入 archive、本地安全备份或待切换新树。

### StagingDatabaseRecoveryOwner（SQLite 外部恢复所有权）

每个 live instance 有且仅有一个 owner final/staging 槽：`.hivegui-db-recovery-v1.json` 与 `.hivegui-db-recovery-v1.json.staging`。owner 至少包含 schema/role/UUID/db_id/instance、`ownership_state=armed` manifest identity、旧/新受控位置/identity/hash 和 phase。两个 role 的 phase 都只允许 `prepared|applying|committed`：前两者重启恢复并验证完整旧状态，`committed` 是完整新状态已验证后的唯一系统 commit point。owner final/staging 的 next-phase、final-only fsync/复验与损坏/缺失分支完全按 storage-migration 合同。`armed+owner 缺失/staging-only/损坏` 必须 fail-closed；只有 `unarmed+owner final/staging 均无` 才是 `aborted-pre-switch`。live instance 下绝不单独删除 owner/manifest。

### StagingDatabaseRetirementJournal（registry-level 终态接管）

retirement final basename 固定为 `.hivegui-db-retirement-v1-{role}-{UUID}.json`，staging basename 固定为 `.hivegui-db-retirement-v1-{role}-{UUID}.json.staging`，tombstone 固定为 `.hivegui-db-retired-v1-{terminal_outcome}-{role}-{UUID}`。outcome 只允许 `aborted_pre_switch|old|new`；journal 字段至少包含 schema/role/UUID/db_id/outcome、live/tombstone basename、live directory/manifest/owner identity、owner phase（aborted 为空）、terminal current 与 Plugin identity/hash、`state=prepared|renamed|done`。`aborted_pre_switch` 只接管 unarmed/no-owner；`old` 只接管 prepared/applying 已恢复验证的旧状态；`new` 只接管 committed 新状态。prepared journal 耐久后才可 identity-bound no-replace rename 整个 live instance；只有匹配 journal 的 tombstone 内才能逐叶删除 owner/manifest/payload。journal/tombstone 的崩溃重放保证任何 manifest/owner unlink 窗口仍可恢复，未知 tombstone、双目录、identity/outcome/hash 不匹配都 fail-closed。

状态不变量：

- `prepared` 已通过排他创建、文件 flush/fsync 和父目录 fsync 耐久化，canonical sidecar 尚未修改；随后只能使用 identity-bound、no-replace 原子操作移入同目录 quarantine。
- `quarantined` 只能在 rename、父目录 fsync 和 quarantine identity 复验都成功后，以 staging→flush/fsync→atomic replace→父目录 fsync 耐久发布；此状态下 canonical 不存在且 quarantine 与 expected identity/size/hash 完全一致。
- `done` 只能在从已验证句柄删除 quarantine 并重新 fsync 父目录后发布；journal 删除及再次父目录 fsync 完成前 Store 仍保持关闭。
- 启动恢复按 `prepared|quarantined|done` 和 canonical/quarantine 的实际 identity 幂等收敛。`prepared` 允许“canonical 匹配”或“quarantine 匹配”二选一；`quarantined` 允许 quarantine 匹配，或二者均不存在但仍需补做父目录 fsync；`done` 只允许二者均不存在，随后删除 journal 并 fsync 父目录。canonical 出现新 identity 为 `sidecar_reappeared`；`done` 时 quarantine 重现、两处同时存在、identity 不匹配、journal 损坏/重复或状态不可证明时全部保留并以 `sidecar_unknown_owner` fail-closed；明确 cleanup/delete/fsync 失败才是 `sidecar_cleanup_failed`。

hot、unknown-owner、recoverable sidecar 从不进入该 journal，且必须在 canonical 名称字节不变。已进入 journal 的安全残留在故障后只保证完整字节位于 canonical 或 quarantine，或者在 `quarantined` 后已经删除并等待目录耐久确认；不得要求任一 fsync/unlink 故障都把它恢复到原路径。合法 `storage_recovery_blocked` 配对与确定性优先级以 local-runtime contract 为唯一真值源。

### Function (`functions`)

字段：`id`, `identifier UNIQUE`, `name`, `description`, `kind` (`builtin|custom|placeholder`), `input_schema`, `output_schema`, `plugin_id FK→Plugin RESTRICT`, `plugin_export`, `category_id FK→Category SET NULL`, `required_capabilities`, `created_at`, `updated_at`。

- `builtin` 的 `plugin_id`、`plugin_export` 为空且 identifier 属于四个保留值。
- `custom` 必须绑定未软删除的 Plugin 与非空 export。
- `placeholder` 是用户可管理的 schema-only 记录，仅用于 LLM 提示词和 Tool schema 调试；`plugin_id`、`plugin_export`、`required_capabilities` 必须为空，任何执行入口都必须在 Capability 检查和 Plugin 查找前返回 `function_not_executable`。
- input/output/required_capabilities 均以 JSON 文本存储，保存时完成结构与大小校验。
- 被 WorkflowNode 或 Tool 引用时拒绝删除。

### Workflow (`workflows`)

字段：`id`, `identifier UNIQUE`, `name`, `description`, `timeout_ms`, `category_id`, `input_schema`, `start_description`, `output_schema`, `required_capabilities`, `created_at`, `updated_at`。timeout_ms 范围为 1..=120000，start_description 最大 1MiB；category_id 为空或引用现有 Category。

未被 Tool 引用时，删除 Workflow 会通过 `workflow_nodes.workflow_id` 和 `workflow_edges.workflow_id` 的 `ON DELETE CASCADE` 同时删除其节点与连线。若任一 `tools.workflow_id` 引用该 Workflow，`ON DELETE RESTRICT` 必须阻止删除；公开 Workflow 删除边界返回 `conflict { field: "id", reason: "referenced_by_tool", references }`，`references` 只包含引用 Tool 的安全 `id`、`identifier` 或 `name`，且 Workflow、节点、连线和 Tool 均保持不变。

### WorkflowNode (`workflow_nodes`)

字段：`id`, `workflow_id FK→Workflow CASCADE`, `node_key`, `node_type` (`start_node|end_node|function_node|generate_answer_node`), `function_id FK→Function RESTRICT`, `position_x`, `position_y`, `node_config`, `created_at`；`UNIQUE(workflow_id,node_key)`。

- `function_node` 必须有 `function_id`；其他节点必须为空。
- 同一 Workflow 恰好一个 `start_node`，至少一个从 `start_node` 可达的 `end_node`。
- `node_key` 使用 identifier 规则；`node_type` 仅允许四个稳定值；`position_x`、`position_y` 必须为有限数值。
- `node_config` 为空或为最大 1MiB 的有效 JSON。

### WorkflowEdge (`workflow_edges`)

字段：`id`, `workflow_id FK→Workflow CASCADE`, `src_node_key`, `dst_node_key`, `mapping`；`UNIQUE(workflow_id,src_node_key,dst_node_key)`。mapping 为空或为最大 1MiB 的有效 JSON；保存图时在同一事务内验证 workflow_id 存在、端点属于同一 Workflow 且存在、无自环、全图无环。

### Tool (`tools`)

字段：`id`, `identifier UNIQUE`, `name`, `description`, `kind` (`function-wrap|workflow-wrap`), `source` (`workspace|builtin`), `is_always`, `function_id FK→Function RESTRICT`, `workflow_id FK→Workflow RESTRICT`, `input_schema`, `output_schema`, `category_id FK→Category SET NULL`, `required_capabilities`, `created_at`, `updated_at`。

`kind` 只允许两个稳定字符串值且与目标外键严格互斥；`source` 只允许 `workspace|builtin`。Function 包装器的 schema 必须与 Function 相同，Workflow 包装器的 schema 必须与 Workflow 边界兼容。共享核心中的 persisted Tool 类型必须在不依赖 HiveGUI/HiveWeb 存储或传输类型的条件下完成序列化 roundtrip，保持 kind、source、互斥目标和 required_capabilities 的输入顺序；重复或未知 Capability 必须在构造边界拒绝。消息回复、结束对话和子 Agent 路由是运行时控制能力，不属于该持久化类型。

### Skill (`skills`)

字段：`id`, `identifier UNIQUE`, `name`, `description`, `frontmatter`, `content`, `source`, `is_always`, `category_id`, `required_capabilities`, `created_at`, `updated_at`。source 只允许 `workspace|builtin`；frontmatter 为空或为最大 1MiB 的有效 JSON，content 为最大 1MiB 的 markdown。

### Agent (`agents`)

| 字段 | 类型 | 约束 |
| --- | --- | --- |
| id | INTEGER | PK |
| identifier | TEXT | NOT NULL, UNIQUE |
| name | TEXT | NOT NULL |
| description | TEXT | nullable |
| system_prompt | TEXT | NOT NULL；去除首尾空白后非空，最大 1MiB |
| parent_agent_id | INTEGER | FK→Agent SET NULL |
| depth | INTEGER | 0..10，由父关系计算 |
| is_default | INTEGER | 0/1 |
| model_preset | TEXT | nullable，引用 LlmPreset.name 的应用层外键；保存时验证存在 |
| created_at / updated_at | TEXT | UTC RFC3339 |

存在 Agent 时，部分唯一索引保证最多一个 `is_default=1`，服务事务保证至少一个；默认 Agent 必须是 `parent_agent_id IS NULL AND depth=0`。parent_agent_id 为空或引用现有 Agent，保存时验证父链无环且计算后的 depth≤10；depth 不接受 UI 输入。创建首个 Agent 自动设为默认，删除默认 Agent 前必须原子指定替代项。Preset 重命名必须在同一事务中更新全部 `agents.model_preset`；删除被任一 Agent 引用的 Preset 必须 RESTRICT 并列出引用 Agent。备份恢复必须拒绝悬空 model_preset。

### AgentTool / AgentSkill / AgentCapability

- `agent_tools(agent_id FK CASCADE, tool_id FK CASCADE, PRIMARY KEY(agent_id,tool_id))`
- `agent_skills(agent_id FK CASCADE, skill_id FK CASCADE, PRIMARY KEY(agent_id,skill_id))`
- `agent_capabilities(agent_id FK CASCADE, capability_name FK→Capability RESTRICT, PRIMARY KEY(agent_id,capability_name))`

运行时 Tool/Skill 集合为显式关联与 `is_always=1` 的并集并按 ID 去重。Capability 不继承父 Agent。

## 5. 会话与执行实体

### ChatSession (`chat_sessions`)

字段：`id TEXT PK`（UUID）, `entry_agent_id FK→Agent RESTRICT`, `current_agent_id FK→Agent SET NULL`, `title_encrypted BLOB`, `status` (`active|completed|failed|cancelled`), `execution_id TEXT`, `created_at`, `updated_at`, `expires_at`。

默认 `expires_at` 按 `created_at + 100 个日历年` 计算。删除会话必须级联 ChatMessage 和 AgentExecution。

### ChatMessage (`chat_messages`)

字段：`id INTEGER PK`, `session_id FK→ChatSession CASCADE`, `seq`, `role` (`system|user|assistant|tool`), `content_encrypted BLOB`, `tool_calls_encrypted BLOB`, `created_at`；`UNIQUE(session_id,seq)`。正文和 Tool 调用参数/结果均不得明文写入数据库或日志。

### AgentExecution (`agent_executions`)

字段：`execution_id TEXT PK`（UUID）, `session_id FK→ChatSession CASCADE`, `current_agent_id FK→Agent SET NULL`, `status` (`running|completed|failed|cancelled`), `state_encrypted BLOB`, `started_at`, `finished_at`, `error_kind`。

`state_encrypted` 是版本化 JSON，保存路由路径、已完成/中断/未开始步骤、最后稳定检查点和外部副作用提示。UI 的 `cancelling` 仅是内存态；持久化终态为 `cancelled`。

## 6. 状态转换

### AgentExecution

```text
running ──success──> completed
running ──error────> failed
running ──stop─────> [cancelling in memory] ──> cancelled
```

终态不可重新进入 running；重试创建新的 execution_id。应用异常退出后，启动恢复把遗留 running 记录转为 failed/interrupted，不自动重放可能有副作用的步骤。

### Plugin

```text
import validation -> active -> soft-deleted
active -> missing/corrupt (derived health, not persisted state) -> re-import -> active
```

健康状态由托管文件存在性和 SHA-256 派生；不允许隐式网络补取。

### Workflow execution

节点状态为 `pending|running|completed|failed|cancelled|skipped`，保存在加密 AgentExecution 状态中。第一个失败或取消发生后不再启动 pending 节点；已完成副作用不回滚。

## 7. 跨实体不变量

1. Agent 父链无环、最大深度 10；运行路由只能进入直接子 Agent，当前路径不得重复 Agent。
2. 执行资源的 required_capabilities 必须是当前 Agent 显式 Capability 集合的子集。
3. Plugin manifest ABI 版本、声明 Capability、托管路径、大小、SHA-256 全部验证通过后才可执行。
4. Builtin Function 集合始终且仅为 `format_template`、`json_parse`、`json_stringify`、`text_regex_match`；不得保留点号记录或 lookup/execute 别名。
5. Workflow 图与元数据必须在一个 SQLite 事务中保存，失败不留下半张图。
6. 删除/清理会话不影响 Agent、Tool、Workflow、Plugin 或 LLM 配置。
7. 敏感 BLOB 的解密只发生在调用边界；日志与诊断包只记录长度、哈希或脱敏摘要。
8. v4 关系表白名单仅包含 AgentTool、AgentSkill、AgentCapability、WorkflowNode 和 WorkflowEdge 及本模型明确列出的外键；Tag 不建立任意实体关系表，公开 DTO、Store 方法和 UI 均不得暴露未定义关系管理。

## 8. Schema 迁移设计

目标 schema 版本为 **4**：

- 新数据库只能在 SQLite 运行时探测确认 FTS5 trigram tokenizer 可用后创建 v4 schema；不可用时 fail-closed 并阻止进入主界面，不得退回 `LIKE` 或全表扫描。v4 必须写入 `search_normalization_id=hivegui-nfkc-casefold-v1`，建库完成后通过统一 SQLite 健康检查和搜索索引一致性验证，才可写入版本 4 并开放使用。
- **v2 → v3**：添加 Plugin 资源覆盖列、Agent `is_default`、三个 Agent 关联表、约束与索引；回填唯一默认根 Agent；把遗留 Function kind `1/2/3` 显式映射为 `builtin/custom/placeholder`，把 Tool kind `1/2` 映射为 `function-wrap/workflow-wrap`，未知值使迁移失败；若旧库存在 `format.template`、`json.parse`、`json.stringify`、`text.regex_match`，迁移事务必须将其精确重命名为下划线 identifier，目标名称已存在时失败并回滚，不合并或保留别名。
- **v3 → v4**：添加 ChatSession、ChatMessage、AgentExecution 和 `plugins.row_revision NOT NULL DEFAULT 0`（既有 Plugin 回填 0），统一 LLM 表与外键索引，并创建本节定义的 `plugin_artifact_operations`、`plugin_artifact_gc`、`schema_metadata`、`search_documents`、FTS5 trigram 虚拟表和 short-gram 索引；在同一迁移事务中按 `hivegui-nfkc-casefold-v1` 回填全部可搜索实体、写入 normalization ID，并验证 Plugin 内部表约束以及实体与派生索引一一对应。新建 v4 必须一次性包含同样的列、表、CHECK/UNIQUE 约束和索引，不得由运行时 Store 追加 DDL。
- 当前版本接受 v2、v3、v4；v1 及更旧版本拒绝自动迁移并提示先用中间版本升级；高于 v4 的数据库拒绝打开。
- 每次打开已有数据库及迁移开始前先运行统一 SQLite 健康检查并验证运行时 FTS5 trigram 与存储的 normalization ID。创建文件级安全快照前必须冻结写入、完成非 busy `wal_checkpoint(TRUNCATE)`、关闭全部连接、证明没有可恢复的 `-wal`/`-shm`/rollback-journal sidecar，并 fsync 主文件与父目录；hot/unknown/recoverable sidecar 在 canonical 名称原样保留并 fail-closed，只有 checkpoint 后归属和 identity 明确且不含可恢复状态的残留才可经 `SidecarCleanupJournal` 耐久清理，journal 删除前不得快照或开放 Store。v2→v3→v4 在单个 SQLite 事务中顺序执行；迁移提交前对目标数据库再次运行两项健康检查和搜索索引一致性验证，commit 后重复 checkpoint/关闭/sidecar/fsync 与重开验证。FTS5 trigram 不可用、normalization ID 不匹配且没有显式迁移、回填或索引验证失败、任一迁移步骤失败、`integrity_check` 非唯一 `ok` 或 `foreign_key_check` 返回任意行，都必须回滚或从安全快照恢复并阻止进入主界面。

## 9. 索引与查询

- 除 Category 外，所有管理列表使用固定 `LIMIT 20 OFFSET ?`。identifier 唯一索引与精确 lookup 使用 SQLite `BINARY`/等价 ASCII 字节级、大小写敏感语义；`Test` 与 `test` 可同时存在，普通 Store 不得偷偷 lowercase。可搜索字段目录为 DataSource=`name`，GlobalConfig=`name/key`，LlmPreset/LlmProvider/Model/Tag/Capability=`name`，Plugin/Function/Workflow/Tool/Skill/Agent=`name/identifier`；复合过滤、排序或关联查询另按其生产查询清单建立覆盖 B-tree 索引。
- 搜索文本与输入都使用标识固定为 `hivegui-nfkc-casefold-v1` 的同一确定性 Unicode 规则分别生成规范化字段；该 ID 精确定义 Unicode Standard 17.0.0 Default Case Algorithms R5：逐 Unicode 标量应用官方 `DerivedNormalizationProps-17.0.0.txt` 的 `NFKC_CF` 映射（缺失项 identity），拼接后使用 Unicode 17.0.0 NFC。生成表的官方源 URL、SHA-256 和生成命令必须记录在 provenance，`unicode-normalization =0.1.25` 的 `UNICODE_VERSION`、生成表校验值和算法行为由提交的 contract/golden fixture 锁定；不得用 lowercase、simple fold 或其它组合近似。依赖、源校验值或 Unicode 表升级导致 fixture 变化时必须分配新 ID 并通过显式 schema 迁移重建。原始非空输入长度必须为 1..=255 个 Unicode 标量值，规范化结果为空时返回 `invalid_input { field: "search", reason: "empty_after_normalization" }`，否则索引分派按规范化结果的标量值数量决定。不得把 `name` 与 `identifier` 拼接后产生跨字段伪匹配。`%`、`_`、引号和 FTS 操作符都是普通文本；规范化后长度至少 3 的查询通过中央 FTS 字面量编码器把完整文本引用为一个 phrase，内部双引号按 FTS 规则加倍，`%` 与 `_` 保持字面量，再将结果作为单个参数 bind 到静态 `MATCH ?` 查询，不能进入 SQL 源文本。只有同样保持 `VIRTUAL TABLE INDEX` 访问的参数化等价查询才可替代该 `MATCH` 形式。
- v4 schema 包含 `schema_metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL)` 且必须有唯一行 `('search_normalization_id','hivegui-nfkc-casefold-v1')`；还包含 `search_documents(id INTEGER PRIMARY KEY, entity_type TEXT NOT NULL, entity_key TEXT NOT NULL, field TEXT NOT NULL, normalized_text TEXT NOT NULL, UNIQUE(entity_type, entity_key, field))`、外部内容 FTS5 表 `search_documents_fts(normalized_text, content='search_documents', content_rowid='id', tokenize='trigram case_sensitive 1')`，以及 `search_short_grams(document_id INTEGER FK→search_documents ON DELETE CASCADE, gram_len INTEGER CHECK(gram_len IN (1,2)), gram TEXT NOT NULL, PRIMARY KEY(document_id, gram_len, gram)) WITHOUT ROWID` 和覆盖索引 `(gram_len, gram, document_id)`。SQLite 运行时必须提供 FTS5 trigram tokenizer；启动探测、normalization ID 或迁移探测失败时 fail-closed，不得回退前导通配 `LIKE`、普通表全表扫描或内存扫描。
- 每次可搜索实体新增、更新、软删除或删除时，基础实体、`search_documents`、对应 FTS delete/insert 命令以及由规范化文本全部不同 1/2 Unicode 标量窗口组成的 `search_short_grams` 必须在同一 SQLite 事务内更新。1–2 字符搜索只走 short-gram 索引，至少 3 字符搜索只走 FTS5 trigram；同一实体的多个字段命中先按实体键去重，不做 name/identifier 相关性优先级。所有非 Category 列表与搜索的静态实体加载查询必须使用总排序 `normalized_display_name ASC, normalized_identifier_or_key ASC, id ASC`，缺失的第二键取空字符串；每个排序键使用 `hivegui-nfkc-casefold-v1` 派生值，数值主键是最终 tie-break，数据不变时跨页不得重排或重复。schema 必须提供 `(entity_type, field, normalized_text, entity_key)` 或等价覆盖索引，并由 EXPLAIN 证明去重、排序和分页不退化为未经批准的业务表全扫。
- Category 使用一次批量查询加载完整树；搜索在内存或单次递归查询中补齐祖先，不得逐节点查询。
- `chat_sessions(updated_at DESC)`、`chat_sessions(expires_at)` 支持最近会话和过期清理。
- `chat_messages(session_id, seq)` 支持稳定重放。
- `agent_executions(session_id, started_at DESC)` 与 `agent_executions(status)` 支持恢复和诊断。
- `workflow_nodes(workflow_id)`、`workflow_edges(workflow_id)` 支持一次加载完整 DAG，禁止逐节点 N+1 查询。
- 用户输入不得通过字符串拼接进入 SQL。固定应用 schema SQL 使用 SQLx `query!`/`query_as!`/`query_scalar!` 及 offline metadata；有限结构变体只允许封闭 enum 和穷尽 `match` 选择静态 checked query，生产代码中的 SQLx `QueryBuilder`、运行时 SQL 字符串和任意动态标识符数量必须为 0。HiveGUI 外部 MySQL 浏览器使用 `mysql_async` prepared statement 绑定值；数据库名、表名和列名不能 bind，必须精确匹配从当前服务器 metadata 预先加载的 allowlist，并且只能由单一、经过测试、按上下文序列化的 `MysqlIdentifier` 类型写入查询。唯一运行时 SQLx 例外是 HiveWeb `named_queries.toml` 的单一 reviewed-config `AssertSqlSafe` 所有者。禁止直接使用用户文本、临时字符串格式化或分散的 escape helper 生成标识符。
- 每个含过滤或关联条件的生产查询必须保存 SQLite `EXPLAIN QUERY PLAN` 或 MySQL `EXPLAIN` 验收证据，并由测试解析计划、断言全部过滤/关联列使用预期索引；FTS5 计划的 `VIRTUAL TABLE INDEX` 是有效索引访问，即使 detail 文本包含 `SCAN` 也不能据此单独判失败。非小型表出现未经批准的真实全表扫描、缺少预期 `SEARCH`/索引或计划无法判定时必须失败。小型固定表或 metadata 查询例外必须记录表大小、理由、审批者、到期日和复核条件总结。Agent 资源快照、Workflow 整图、Category 树，以及按会话批量加载 ChatMessage/AgentExecution 必须以查询计数测试证明无 N+1，测试环境检测到 N+1 时必须失败。会话列表、消息/执行记录加载、过期计数与清理、遗留 running 扫描必须分别验证其过滤、排序和关联索引。

## 10. 备份序列化

备份清单中的每种实体使用稳定复数键，并携带 `schema_version=4`、`backup_format_version=3`、`exported_at`。当前格式中的 Function.kind 只能是 `builtin|custom|placeholder`，WorkflowNode.node_type 只能是四个 `*_node` 值，Builtin identifier 只能是四个下划线名称；整数 kind、短节点名称和点号 Builtin 必须拒绝。`search_documents`、FTS5 shadow tables 和 `search_short_grams` 是派生索引，不进入清单；恢复时在隔离数据库事务内从实体字段重新构建，并在提交前完成 SQLite 健康检查和搜索索引一致性验证。

导出最终目标必须尚不存在；同名目标或并发创建冲突不得覆盖。协议在用户选择的目标目录创建唯一 staging 文件，认证加密流及全部制品写完后 flush、fsync 文件，以 no-replace 原子重命名为最终备份名并 fsync 父目录；全部步骤成功前不得报告有效最终文件。旧 format 1/2 只能在隔离 live instance 中一次性升级，点号到下划线发生碰撞时保持 current 不变；unarmed/no-owner instance 必须经 outcome=`aborted_pre_switch` retirement 整目录移入 tombstone 后清理，不能直接删除 live payload。敏感字段在受控导出过程中从设备密钥解密，只存在于外层认证加密流内。

恢复只能相对已打开的受控 staging 根目录句柄逐段 no-follow 解包，归档 symlink、hardlink、device、FIFO、socket、绝对路径、`..`、Windows junction/reparse point、预置 staging 链接/特殊文件和其他非普通文件/重定向目标全部拒绝；仅 canonicalize 后重新按路径打开不满足边界。恢复可以流式处理已通过分块认证的内容，但敏感值只在有界内存中短暂出现，并立即使用目标设备密钥写入隔离 staging 数据库；认证流结尾和全部预检成功前不得应用到现有状态。错误口令、截断或后段篡改必须保持 current 不变；只有 unarmed/no-owner instance 可经 aborted retirement 整目录接管清理，其余状态原样 fail-closed。设备密钥、结构化日志和诊断包不得进入备份。

恢复预验证只展示影响；用户最终确认后必须立即冻结 current Store 写入，并在同一冻结周期先对 current 和 staging 数据库完成非 busy `wal_checkpoint(TRUNCATE)`、关闭全部连接并按存储合同分流 sidecar；hot/unknown/recoverable sidecar 在 canonical 名称原样保留并阻断，安全残留的 cleanup journal 必须先耐久完成和删除。完成封闭文件边界后，从冻结的 current 生成并验证安全备份，再自底向上 flush/fsync staging 数据库、每个制品文件及其目录；把 instance manifest 从 unarmed 推进 armed，通过 owner final/staging 耐久发布 `prepared|applying`，以位于各自目标同一文件系统内的 staging 源执行原子 rename/swap，完整切换数据库和托管制品目录，并 fsync 所有受影响父目录。无法保证同一文件系统或原子语义时必须在进入 applying 前 fail-closed。新 current 必须在发布 `committed` 前通过完整 health/search/artifact/identity 验证；prepared/applying 失败恢复并验证完整 old，committed 后只通过 outcome=`new` retirement 收口已验证 new。retirement 全部完成后才开放 Store。任何阶段失败或崩溃后，启动恢复都不能丢失已提交 WAL 帧、删除 hot/unknown/recoverable sidecar、组合旧 sidecar 与新主文件或暴露混合状态；受控根目录外不得发生任何读写。
