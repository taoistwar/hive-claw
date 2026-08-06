# Encrypted Backup Package Contract

## 1. 文件层次

备份包是单一认证加密文件，外层为带口令的流式加密 envelope，内层为 tar archive：

```text
manifest.json
entities/<entity-name>.json
plugins/<plugin-id>/plugin.wasm
```

设备本地密钥、运行日志、诊断包、`.hivegui-db-staging-v1` registry、live/tombstone instance、instance manifest/owner/retirement journal 的 final/`.staging`、SQLite cleanup journal/quarantine 都不进入 archive 或本地回滚安全备份。`plugin_artifact_operations`、`plugin_artifact_gc` 与派生搜索表也不作为 archive 实体导出；它们与用户实体的区别不得被误读为从本地完整 SQLite 回滚快照过滤：该快照必须保留冻结封闭边界上的完整主文件及其内部表。

## 2. Manifest

```json
{
  "format": "hivegui-backup",
  "format_version": 3,
  "schema_version": 4,
  "exported_at": "RFC3339 UTC",
  "entity_files": [{"name":"agents","path":"entities/agents.json","count":1,"sha256":"..."}],
  "artifacts": [{"plugin_id":1,"path":"plugins/1/plugin.wasm","size_bytes":123,"sha256":"..."}]
}
```

所有 archive path 必须是 UTF-8 安全相对路径，不得重复、绝对化、含空组件、`.` 或 `..`。archive 只允许目录和普通文件条目；symlink、hardlink、device、FIFO、socket 及其他特殊条目一律拒绝。恢复端必须从已打开的私有 staging 根目录句柄逐段解析并创建目标，且每段都使用平台等价的 no-follow 语义；已有或并发替换为 Unix symlink、Windows junction/reparse point、hardlink、device、FIFO、socket 或其他非普通文件/可重定向链接的中间目录和最终目标都必须失败。仅做字符串规范化，或先 `canonicalize` 再按路径重新打开，不满足本契约；路径校验、写入、哈希和后续切换必须绑定同一组已验证且仍打开的目录/文件句柄，不得留下 TOCTOU 窗口。

实体集合必须完整包含 DataSource、GlobalConfig、LlmPreset、LlmProvider、Model、Tag、Category、Capability、Plugin、Function、Workflow、WorkflowNode、WorkflowEdge、Tool、Skill、Agent、AgentTool、AgentSkill、AgentCapability、ChatSession、ChatMessage 和 AgentExecution；不得因列表为空而省略实体文件或清单项。
恢复预检必须验证每个非空 Agent.model_preset 对应一个 LlmPreset.name；悬空引用必须在构建或切换现有状态前拒绝。
当前 format 3 的 Function.kind 只能是 `builtin|custom|placeholder`，WorkflowNode.node_type 只能是 `start_node|end_node|function_node|generate_answer_node`，Builtin identifier 只能是 `format_template|json_parse|json_stringify|text_regex_match`。整数 kind、短节点名称和点号 Builtin 均视为当前格式无效。

## 3. 加密

- 导出要求用户输入独立备份口令，使用经过审计的 passphrase KDF + AEAD 流式格式。
- 认证必须覆盖全部 archive 字节和版本 header。reader 可以处理已经通过分块认证的内容，但只有读取并验证认证流结尾后才算整包认证成功；错误口令、截断或任意位置篡改都必须在任何现有状态修改前失败。
- 敏感 SQLite 字段在受控导出流中解密，进入外层密文；不得写入未加密临时 JSON。
- 恢复时敏感值只在有界内存中短暂转换，并立即用目标设备密钥重新加密后写入隔离暂存数据库；不得向 UI、日志或未加密暂存文件释放敏感明文。

## 4. 版本兼容

当前 reader 接受 format 3、2、1；旧格式在隔离暂存区逐步升级到 3，不修改原包。旧格式中的 Function `1/2/3`、稳定 `*_node` 值和真实点号 Builtin 由受信升级器一次性规范化；点号重命名发生目标碰撞时保持 current 不变，不提供运行时别名。若 live instance 仍为 unarmed 且 owner 双槽均无，则必须通过 outcome=`aborted_pre_switch` retirement 把整目录移入匹配 tombstone 后清理；不得直接删除 live payload。format>3 返回 `backup_newer_version`，format<1 返回 `backup_too_old`。schema 兼容性遵循 storage-migration.md。

## 5. 导出

1. 建立一致性 SQLite 读快照。
2. 枚举全部实体和 Agent 关联表；解密敏感值，仅写入加密流。
3. 逐个验证托管 WASM 路径、大小和 SHA-256，再写入 archive。
4. 最终目标必须尚不存在；若同名目标存在，公开边界返回稳定冲突并要求用户选择新名称，不覆盖或移动旧目标。在用户所选目标文件的同一目录内以排他创建方式创建唯一 staging 文件；生成并验证 manifest，完成认证加密流后依次 flush 和 fsync staging 文件。
5. 以 no-replace 原子 rename 将 staging 发布为目标文件并 fsync 目标的父目录；只有父目录持久化成功后才报告导出成功。rename 前失败必须删除 staging。rename 后父目录同步失败必须返回稳定的 `backup_publish_uncertain` 并绝不能声称成功；实现可以通过同一目录句柄移除本次新目标并再次 fsync，或保留该路径供下次启动重新验证后由用户明确采用/删除。因为最终目标在步骤 4 保证原先不存在，该不确定状态不得造成既有备份丢失。并发创建同名目标必须使 no-replace rename 失败，不得覆盖另一文件。

## 6. 恢复

1. 先从已打开的 HiveGUI 数据根 no-follow 打开或创建固定 `.hivegui-db-staging-v1` registry，排他创建精确 `restore-{db_instance_operation_id}` live instance 并 fsync 父目录；在任何暂存数据库文件创建前，原子耐久发布 `.hivegui-db-instance-v1.json`，其 v1 六元组 `schema_version=1`、`role=restore`、小写 canonical hyphenated UUID、`db_id=restore/{UUID}`、`database_name=datasources.db`、`ownership_state=unarmed` 必须和目录逐字节一致。随后才可在已打开实例内解密并流式验证 header、backup manifest、实体 schema、路径、大小和哈希；所有条目都遵循第 2 节的 no-follow 规则，分块认证成功不等于整包认证成功。预验证和用户确认前不得把 manifest 推进 `armed`，也不得发布 owner final/staging；这段窗口的合规崩溃只能由 storage-migration 合同以 outcome=`aborted_pre_switch` 的 retirement journal + 整目录 tombstone rename 收敛，绝不能影响 current。
2. 以实例内固定 basename `datasources.db` 构建目标 schema 的暂存 SQLite 和暂存 Plugin 目录；按依赖顺序导入，敏感字段经有界内存立即使用目标设备密钥重新加密，禁止未加密落盘。`search_documents`、FTS shadow tables 和 short-gram 不从包中信任导入，必须从实体字段按当前 `hivegui-nfkc-casefold-v1` 在同一暂存事务中重建。暂存 SQLite 可以在其数据库同目录临时产生 cleanup journal/quarantine，但它们不得进入 archive、安全备份或待切换新树，并必须在进入 `applying` 前耐久收敛。registry、live/tombstone instance、manifest final/staging、recovery owner final/staging 与 retirement journal final/staging 都是切换外部 locator/control state，不参与新树/回滚安全备份；终态只按 storage-migration retirement 状态机整目录接管和清理。
3. 对暂存数据库执行健康检查：`PRAGMA integrity_check` 必须精确返回单行 `ok`，`PRAGMA foreign_key_check` 必须返回零行；随后验证 FTS5 trigram tokenizer 可用、normalization ID 正确、实体与两类派生搜索索引一致，以及内置数据和全部制品。连接级 `PRAGMA foreign_keys=ON` 不能替代上述检查。
4. 只有认证流结尾、全部 archive 条目和步骤 3 均验证成功后，才可向用户显示计划使用的安全备份位置、完整替换影响并请求最终确认；此时不得修改当前状态，也不得把确认前生成的预览或临时副本视为本次操作的安全备份。
5. 用户最终确认后必须立即关闭 Store 业务写闸门，并在同一个不中断的冻结周期内对当前数据库与暂存数据库分别执行第 7 节要求的 checkpoint、关闭和 sidecar 验证；随后从已冻结的当前状态重新生成并完成文件、目录、哈希和可恢复性验证的安全备份，向用户显示其最终位置，再执行耐久切换协议。安全备份完成后到切换成功、完整回滚或明确取消完成前不得重新接受业务写入，确保该备份精确对应进入 `applying` 前的当前状态。
6. 认证流结尾失败或预检失败必须在进入切换阶段前按 storage-migration 的 outcome=`aborted_pre_switch` retirement 状态机接管整个 unarmed instance，原子 rename 到 tombstone 后受控清理，并保持 current 不变；无法证明安全时保留 journal/locator、阻断 Store，不能递归跟随路径或猜测删除。用户确认后、安全备份验证前的 checkpoint、关闭连接、sidecar 分类或 cleanup 失败必须保留 current、受保护 sidecar、cleanup journal/quarantine 与关闭的写闸门，不得声称从尚不存在的安全备份或 owner 回滚。只有安全备份已验证、manifest=`armed` 且 owner=`prepared|applying` 后的失败才通过 owner 恢复并验证完整旧状态，再以 retirement outcome=`old` 收口；owner=`committed` 后只能完成、验证新状态并以 outcome=`new` 收口。安全备份保留供手工恢复。

不做记录级合并，不比较 identifier、ID 或 updated_at。结果只能是全新快照全部生效，或原状态全部保留。

## 7. 耐久切换与崩溃恢复

1. 当前 Store 写入冻结后，current 与 staging 数据库必须分别执行非 busy `wal_checkpoint(TRUNCATE)`、关闭全部连接并证明没有可参与恢复的 sidecar；分类、固定优先级、确定性 cleanup-journal final/`.staging`、quarantine 与五分支重放完整复用 [storage-migration.md](storage-migration.md) 第 4 节。registry、live/tombstone instance、manifest/owner/retirement final/staging、cleanup journal/quarantine 都不得进入 archive、安全备份或待切换新树。hot/unknown/recoverable canonical sidecar 保持字节不变；cleanup journal 耐久删除前不得生成安全备份、进入 `applying` 或开放 Store。之后 flush/fsync staging SQLite/制品与各级目录；切换源只取固定 payload。当前状态的安全备份必须在同一冻结周期、无活跃 sidecar的封闭边界完成文件、目录、哈希和可恢复性验证；SQLite 主文件按字节完整保留 Plugin operation/GC ledger 与派生搜索内部表。
2. 安全备份验证成功且实例仍为 `unarmed` 后，先用 `.hivegui-db-instance-v1.json.staging` 把 manifest 原子耐久推进 `armed`；再在 restore instance 内通过固定 `.hivegui-db-recovery-v1.json.staging` 发布 final owner。owner 至少逐字节记录 schema/role/UUID/db_id/instance/manifest identity、旧/新数据库与 Plugin 目录的受控位置/identity/hash和 `prepared|applying|committed` phase。final owner 发布与每次 phase 更新都执行 staging flush/fsync、原子 replace、实例目录 fsync；final-only/final+staging/staging-only 按 storage-migration 矩阵重放。`prepared` final 未耐久前不得让实例成为切换来源，`applying` final 未耐久前不得执行首个 rename/swap。
3. 数据库 staging 与数据库目标、Plugin staging 与 Plugin 目标必须各自在同一文件系统；进入 applying 前必须验证 rename 不会跨文件系统，无法保证或返回 `EXDEV` 时 fail-closed。数据库文件和 Plugin 目录的每次移动都必须相对于已打开的受控父目录句柄执行原子 rename/replace，并在每次操作后 fsync 对应父目录。切换期间的路径解析继续禁止 symlink、hardlink、junction/reparse point 和其他重定向链接。
4. 在 `applying` 下完成数据库/Plugin 目录移动与各父目录 fsync 后，仍须保持写闸门关闭，以只读连接重开新 current，并完成 health、FTS/normalization/search、全部实体关系、制品 identity/hash 和 current 不引用 instance 的验证；任一步失败都必须恢复并验证完整旧状态，owner 仍不得变为 `committed`。只有完整新状态验证成功后才发布 `committed` final；它是唯一 commit point。随后创建 outcome=`new` retirement journal，把整个 live instance 原子 rename 到对应 tombstone并收口；`prepared|applying` 回旧成功则创建 outcome=`old` retirement journal。owner/manifest 只在 tombstone 内逐叶删除。retirement 全部完成后才可开放写闸门；`committed` 后任何失败只能幂等完成 new。
5. 启动固定按 storage-migration 的 `registry/retirement/tombstone → live manifest/owner → current/live cleanup journals → owner replay或 aborted retirement → Store` 顺序处理。registry 缺失表示零条目；存在时按 ASCII 字节序只接受 live、outcome tombstone、retirement final/`.staging` 精确语法并验证 operation group。tombstone 由匹配 retirement journal 接管，不因内部 manifest/owner 已删而误判。live `armed+owner` 必须重放；`armed+owner 缺失/损坏/staging-only` 原样保留并 fail-closed；仅 `unarmed+owner final/staging 均无` 可创建 outcome=`aborted_pre_switch` retirement journal。任何 cleanup/retirement 未耐久收口前不得开放 Store；任意故障重启都不得丢失已提交 WAL 帧、组合错误 sidecar或暴露数据库/Plugin 混合状态。
