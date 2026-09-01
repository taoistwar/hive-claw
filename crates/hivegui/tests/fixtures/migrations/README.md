# SQLite migration fixtures

本目录保存 HiveGUI 历史 SQLite schema 的黄金夹具。夹具只用于测试
`v2 -> v3 -> v4` 与 `v3 -> v4`；它们不是可由当前 Store 写入的业务数据库。
版本与预期行为以
`specs/011-hivegui-standalone-mode/contracts/storage-migration.md` 为准。

## Fixture matrix

| 文件 | 必须包含的输入 | 预期结果 |
| --- | --- | --- |
| `v2-valid.sqlite` | schema version 2；Function kind `1`、`2`、`3`；Tool kind `1`、`2`；四个旧点号 Builtin；同一 Workflow 中的四个 `*_node` | 单个事务顺序升级到 v4；Function kind 精确变为 `builtin`、`custom`、`placeholder`，Tool kind 精确变为 `function-wrap`、`workflow-wrap`；Builtin 精确重命名为下划线名称；节点值不变 |
| `v2-unknown-function-kind.sqlite` | 在合法基线之外增加 kind `99` | 返回稳定迁移错误并回滚；schema version、记录和关系均不变 |
| `v2-unknown-tool-kind.sqlite` | 在合法基线之外增加 Tool kind `99` | 返回稳定迁移错误并回滚；schema version、记录和关系均不变 |
| `v2-builtin-collision.sqlite` | 每个旧点号 Builtin 与其下划线目标同时存在 | 整个迁移事务回滚；不合并、不覆盖、不删除记录，也不创建别名 |
| `v3-valid.sqlite` | schema version 3；三种稳定 Function kind；四个下划线 Builtin；四个 `*_node`；v3 的 Plugin/Agent 关系 | 升级到 v4；原有实体、关系和托管制品引用逐字段不变 |

`v2-valid.sqlite` 至少使用下列固定记录：

- kind `1`：`format.template`、`json.parse`、`json.stringify`、
  `text.regex_match`；
- kind `2`：`fixture_custom`；
- kind `3`：`fixture_placeholder`，且 Plugin、export 和 Capability 字段为空；
- Tool kind `1`和 `2`：分别指向合法 Function 与 Workflow，迁移后分别为 `function-wrap`和 `workflow-wrap`；
- node type：`start_node`、`function_node`、`generate_answer_node`、
  `end_node`，按该顺序组成一个有效 DAG。

迁移后的唯一 Builtin identifier 是 `format_template`、`json_parse`、
`json_stringify`、`text_regex_match`。点号名称只能作为受信迁移输入，迁移后
不得被注册、查询或执行。

## Deterministic generation

1. 生成器必须执行仓库中对应版本的历史建表 SQL，再插入固定场景数据；禁止从
   v4 数据库复制后只修改 version，也禁止用当前 Store API 反推旧 schema。
2. ID、UUID、时间戳、JSON、密文测试向量和记录顺序必须由场景常量给出。时间统一
   使用 `2000-01-01T00:00:00Z`；JSON 使用 UTF-8、LF、排序后的对象键和无多余
   空白的 canonical 表示。不得读取系统时间、主机名、随机源或用户目录。
3. SQLite 固定使用 UTF-8、4096-byte page、`auto_vacuum=NONE`、
   `journal_mode=DELETE` 和 `foreign_keys=ON`。插入完成后执行 foreign-key 与
   integrity 检查，再以 `VACUUM INTO` 写到临时文件并原子替换输出。提交的夹具
   不得带 `-wal`、`-shm` 或 `-journal` sidecar。
4. 所有列表按稳定 ID 排序；碰撞夹具只能由场景数据生成，不能通过十六进制编辑器
   或交互式 `sqlite3` 修改黄金文件。
5. 若数据库引用 Plugin 制品，其 `s3_key`、`sha256` 和 `size_bytes` 必须来自
   `../plugins/README.md` 定义的同一共享 WASM，不得复制另一份来源不明的二进制。
6. 生成器先写私有临时目录，完成全部校验后再替换目标。任一步失败时不得改动已
   提交的黄金夹具。

生成入口由 `migration_compatibility` 测试提供为显式 ignored regeneration case；
日常测试只读并复制黄金夹具到临时 XDG 目录，绝不原地迁移。

## Validation rules

每次生成以及 `migration_compatibility` 测试都必须：

1. 对原文件只读运行 `PRAGMA integrity_check`、`foreign_key_check`，并断言唯一
   schema version 与文件名一致；
2. 逐表校验列、约束、索引、固定行数和固定主键，而不只校验数据库能打开；
3. 校验 v2 的 Function 整数 kind 集合恰为 `{1,2,3}`、Tool 整数 kind 集合恰为
   `{1,2}`，v3 的 Function 字符串 kind 集合恰为 `{builtin,custom,placeholder}`、
   Tool 字符串 kind 集合恰为 `{function-wrap,workflow-wrap}`；
4. 校验每个合法夹具恰好包含四种 `*_node` 值，且不含短名称；
5. 校验合法 v2 中四个点号源名称各出现一次且下划线目标尚不存在，碰撞夹具中
   四对源和目标各出现一次；
6. 在临时副本上迁移后校验 schema version 为 4、映射精确、关系和非迁移字段
   逐字段不变，并断言点号 Builtin lookup/execute 失败；
7. 对未知 Function/Tool kind、目标碰撞和故障注入记录迁移前后的关闭数据库 SHA-256、逻辑表
   摘要和托管 Plugin 目录摘要，断言 rollback 后三者不变、无部分 v3/v4 对象，
   且重试仍产生相同结果。

黄金 `.sqlite` 是生成物：不得手工编辑。任何 schema 或期望变化都必须先修改版本化
生成器和契约测试，再完整再生并复核语义摘要。
