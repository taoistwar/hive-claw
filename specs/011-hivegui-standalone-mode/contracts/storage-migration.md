# SQLite Migration Contract

## 1. 版本策略

- 当前 schema 版本：4。
- 新数据库直接创建 v4。
- 自动迁移只接受 v2 和 v3，且只执行 `v2→v3→v4` 的顺序前向步骤。
- v1 及更旧版本返回 `schema_too_old`；高于 v4 返回 `schema_newer_version`；不支持降级。

版本比较必须显式区分 `equal`、`older-supported`、`older-unsupported` 和 `newer`，不得把 newer 当作“无需迁移”。

## 2. 启动协议

1. 获取单实例文件锁。
2. 数据库不存在时，先探测运行时 FTS5 trigram tokenizer，并验证规范化实现使用 Unicode 17.0.0 `NFKC_CF` 官方映射、生成表校验值及 Unicode 17.0.0 NFC；可用时在启用外键约束的未发布新文件中创建全新 v4、写入 `search_normalization_id=hivegui-nfkc-casefold-v1` 并建立空派生索引。commit 前执行第 3 节定义的两项健康检查及搜索索引一致性验证，成功后按第 4 节持久化并发布，再以新连接重复全部验证。全部检查成功前不得开放主业务 Store，也不得把失败的新文件当作可用数据库。
3. 对既有数据库以只读方式打开，读取 schema version、执行第 3 节定义的两项健康检查，并解析托管 Plugin 根目录；健康检查失败时不得进入迁移。运行时必须同时探测 FTS5 trigram tokenizer；v4 的 `search_normalization_id` 缺失或不是应用支持的 ID 时，只能进入明确支持的 schema 迁移，否则 fail-closed。
4. 若无需迁移，在业务连接上启用外键约束，重新确认健康检查、trigram 可用性、normalization ID 和派生搜索索引一致性后正常打开 Store。
5. 若需迁移，先冻结新写入，在 current 的独占维护连接上要求 `PRAGMA wal_checkpoint(TRUNCATE)` 返回不 busy 且全部帧已 checkpoint，关闭包括维护连接在内的全部 SQLite 连接，验证不存在可参与恢复的 `-wal`、`-shm` 或 rollback-journal sidecar，再 fsync current 主数据库及父目录；之后创建并验证 current 数据库主文件、托管 Plugin 树及清单的本地安全快照。checkpoint、关闭、sidecar 验证、fsync 或快照任一步失败都不得创建 migration instance 或开始迁移。快照成功后，通过第 4 节共享边界排他创建 `migration-{db_instance_operation_id}` 与初始 `ownership_state=unarmed` 的 v1 manifest，再从仍打开且身份已固定的 current/snapshot 句柄把数据库主文件按字节复制到实例内固定 `datasources.db`，flush/fsync 文件和实例目录并复核大小、SHA-256；迁移过程中 current 主文件和托管 Plugin 根保持只读且逐字节不变。
6. 只对 migration instance 内的 staging 数据库建立新的独占连接并启用外键约束，在同一 SQLite transaction 中顺序执行全部迁移；每步完成后写 schema_versions，并在 SQLite commit 前对该 staging 事务内目标状态执行两项健康检查、托管 Plugin 引用/制品校验和搜索索引一致性验证，任一失败只 rollback staging transaction。SQLite commit 只提交未发布的 staging 数据库，不是系统迁移 commit point，也不得修改 current。commit 后仍保持全局写闸门关闭，对 staging 执行非 busy checkpoint、关闭全部连接、按第 4 节收敛它自己的 sidecar/cleanup journal、fsync staging 主文件和实例目录，再以只读连接重开并重复健康、trigram、normalization、搜索索引和制品验证；任一步失败都保持 current/Plugin 不变，按 `aborted-pre-switch` 清理仍为 unarmed 的实例或在无法安全清理时 fail-closed。
7. staging 全部验证并再次关闭连接后，先把 instance manifest 从 `unarmed` 原子耐久推进为 `armed`，再发布 phase=`prepared` 的 migration `.hivegui-db-recovery-v1.json`；owner 必须记录 current、staging、安全快照和不切换的托管 Plugin 根的受控 identity/path/hash。随后耐久推进 `applying`，复用备份恢复合同的同文件系统、root-handle-relative、逐次 rename/replace 与父目录 fsync 协议，只把已验证 staging 数据库发布到 current 的固定 `datasources.db`，托管 Plugin 根必须保持同一 identity。新 current 在写闸门关闭下以只读连接通过全部健康/搜索/制品验证前，owner 不得推进 `committed`；`prepared|applying` 的任一失败或崩溃都按 owner 和安全快照恢复、验证完整旧 current，随后以 `terminal_outcome=old` 创建 retirement journal。验证完整新 current 后才可把 owner 耐久推进 `committed`，该 phase 是唯一系统迁移 commit point；之后只能完成新状态，并以 `terminal_outcome=new` 创建 retirement journal。两种 outcome 都必须证明 current 位于 instance 外且无 live 引用，再按第 4 节把整个 live instance 原子 rename 到匹配 tombstone、逐叶清理并收口 retirement journal；live 名称下绝不单独删除 owner/manifest。retirement 全部耐久完成后才可开放 Store。

## 3. SQLite 健康检查

- `PRAGMA integrity_check` 必须精确返回一行且内容为 `ok`；零行、多行或任意其他内容均为结构完整性失败。
- `PRAGMA foreign_key_check` 必须返回零行；任何返回行均为外键完整性失败。
- 新建数据库、每次打开既有 current、migration staging transaction 的 SQLite commit 前及 commit 后重开、以及 owner `committed` 前的新 current 都必须执行两项检查。`PRAGMA foreign_keys=ON` 只负责当前连接的外键强制执行，不能替代其中任一检查。

## 4. 文件持久化与 sidecar

- HiveGUI 的业务模式可以使用 WAL，但文件级快照和数据库主文件发布的封闭边界固定为：冻结写入 → 独占连接 `wal_checkpoint(TRUNCATE)` 且结果不 busy/无未 checkpoint 帧 → 关闭全部连接 → 验证没有可恢复的 `-wal`/`-shm`/rollback-journal sidecar → fsync 主文件 → fsync 父目录。
- 快照、恢复或发布不能只复制、哈希、rename 主文件并忽略 sidecar。已提交但未 checkpoint 的 WAL 必须由成功 checkpoint 合并且验证所有帧保留。实现必须先分类全部异常，再严格按 [local-runtime.md](local-runtime.md) 的合法 reason/artifact 配对、reason 总优先级和 `wal > rollback_journal > shm` artifact 次序返回 `storage_recovery_blocked`；不得依赖目录枚举顺序。hot、归属未知、仍含可恢复状态的 sidecar 在 canonical 名称下按字节保持不变并 fail-closed，绝不能盲删。
- 只有 checkpoint/关闭已经证明不含可恢复状态、归属当前数据库且身份固定的残留 sidecar 才能进入清理；清理使用数据库同目录、SQLite 外部且不进入任何备份包的耐久 journal。当前业务库固定由已打开的数据根句柄与 ASCII basename `datasources.db` 定位，`db_id=current`。迁移/恢复 live instance 固定为数据根下 `.hivegui-db-staging-v1/{role}-{db_instance_operation_id}`，role 只允许 `migration|restore`，UUID 必须是 registry 内全局唯一的小写 canonical hyphenated UUID；实例数据库 basename 固定 `datasources.db`，db_id 固定 `migration/{UUID}|restore/{UUID}`。任何名称解析都不执行 Unicode normalization、大小写转换、分隔符替换或 canonicalize。
- 创建 staging 数据库前必须先相对已打开的数据根 no-follow 排他创建 registry/live instance 并 fsync 各父目录，再以 final basename `.hivegui-db-instance-v1.json` 原子耐久发布 v1 manifest。初始 manifest 六元组固定为 `schema_version=1`、role、`db_instance_operation_id`、完整 db_id、`database_name="datasources.db"`、`ownership_state=unarmed`，必须与目录逐字节匹配；manifest final 与 `.staging` 的状态更新按下一条执行。只有初始 final 发布和实例目录 fsync 成功后才可创建数据库。registry 缺失精确表示零 live/tombstone/retirement 条目；registry 存在时须按第 4 节后续规则枚举全部允许项。每个 UUID 全局唯一地标识一个 operation group，组内只允许 retirement 状态机规定的 live/tombstone/journal 组合，组间绝不得复用；随后检查 current 与每个 live instance 的六个 sidecar-journal 槽。缺失/损坏 manifest、未知条目、链接/特殊文件、名称/记录不匹配或 identity 无法证明时全部保留并 fail-closed；尚不能确定具体 artifact 时稳定返回 `storage_recovery_blocked { reason: sidecar_unknown_owner, artifact: wal }`。
- 对任一 `db_id` 与 `wal|shm|rollback_journal`，令 `db_bytes=UTF-8(db_id)`，精确计算 `db_token=lowercase_hex(SHA-256(UTF-8("hivegui-sidecar-cleanup-v1") || byte(0x00) || uint32_be(len(db_bytes)) || db_bytes))`。final basename 固定为 `.hivegui-sidecar-cleanup-v1-{db_token}-{artifact}.json`，状态 staging 固定追加 `.staging`；artifact 使用 ASCII 原值，hex 固定 64 个小写字符。每个组合最多一个 final 和一个 staging，启动直接检查六个精确槽位。final 不存在而 staging 存在只在可证明 canonical 未修改时受控删除 staging并 fsync 父目录；final+staging、staging 内容/名称不匹配或无法证明 canonical 未修改时按槽位 artifact 返回 `sidecar_unknown_owner`。记录至少包含 `schema_version=1`、与 instance UUID 明确不同且不复用的 `cleanup_operation_id`、原样 db_id/数据库 identity、artifact、canonical 相对名、expected identity/size/SHA-256、精确 `quarantine_name=.hivegui-sidecar-quarantine-v1-{cleanup_operation_id}-{artifact}` 和 `prepared|quarantined|done`。重放必须重新计算 token并逐字节匹配 db_id；journal/状态更新都使用确定性 staging、flush/fsync、原子 replace 和父目录 fsync，损坏/重复/未知版本或记录不匹配必须保留并阻断 Store。
- v1 instance manifest 还必须包含 `ownership_state=unarmed|armed`，初次发布固定为 `unarmed`，唯一转换是不可逆的 `unarmed→armed`。每次更新使用精确 staging basename `.hivegui-db-instance-v1.json.staging`，完整写入、flush/fsync、原子 replace final 并 fsync 实例目录；`armed` 必须在首次 owner 发布之前耐久完成。final-only manifest 必须先补做实例目录 fsync并从仍打开句柄复验 identity/bytes；final 与 staging 同时存在时，仅当 final 完整有效且 staging 逐字节匹配同一 manifest identity 并恰为 `unarmed→armed` 时，才删除 staging、fsync 目录并以 final 为准重试转换；staging-only、final 损坏/缺失、降级或不匹配均保留并 fail-closed。terminal-old、terminal-new 和 aborted 的收口证据不再通过改写或删除 live manifest 表示，而必须原子交给下述 registry-level retirement journal；因此 live instance 下的 manifest/owner 在整目录 rename 前始终保持相互匹配。
- 每个有效 staging instance 还具有一个精确恢复所有权槽：final basename 固定为 `.hivegui-db-recovery-v1.json`，发布/phase-update staging basename 固定为 `.hivegui-db-recovery-v1.json.staging`。role=`migration` 时它就是迁移恢复日志，role=`restore` 时它就是备份恢复切换日志；记录必须至少逐字节包含 `schema_version=1`、role、`db_instance_operation_id`、完整 `db_id`、instance basename、manifest identity/`ownership_state=armed`，以及相应迁移/切换状态、旧/新受控相对位置和哈希。只有 manifest `armed` 后才可写 owner staging；final 记录必须以 flush/fsync、原子 replace 和父目录 fsync 耐久完成后，实例才可成为 rename/swap 来源或首次影响 current。未知或不匹配的临时文件、损坏/重复记录、role/UUID/db_id/path/manifest identity 不匹配、一个实例被多条记录声明，或 registry 中同时存在多个未终态 owner 时全部原样保留并 fail-closed。live instance 下不得单独删除 owner 或 manifest；终态 owner 必须与 manifest 一起随整个 instance directory 原子移入 retirement tombstone，之后才由 registry-level retirement journal 驱动逐叶清理。
- 两种 role 的 owner phase 都固定为 `prepared|applying|committed`：`prepared` 表示 owner 已耐久但 current 仍是完整旧状态；`applying` 表示已开始由记录约束的 current/instance 原子移动，重启必须幂等恢复完整旧状态；`committed` 是唯一系统 commit point，重启只能保持写闸门关闭并完成、验证完整新状态。每次 phase 更新都把完整下一状态写入精确 `.hivegui-db-recovery-v1.json.staging`，flush/fsync 后原子 replace final 并 fsync 实例目录。final-only 且完整匹配时，启动必须先重新 fsync 实例目录、从仍打开句柄复验 final identity/bytes，再按 final 重放；final+staging 只在 staging 与同一 owner identity 匹配且是 `prepared→applying` 或 `applying→committed` 的恰好下一 phase 时受控删除 staging、fsync 后按 final 重放，因为 staging 尚未发布且任何下一阶段副作用都不得先于 final；staging-only、final/staging 损坏或不匹配、phase 跳跃/回退均保留并 fail-closed。migration 与 restore 不得另造不同 commit 语义。
- Live instance 的内容不得直接逐项删除。只有三种终态可进入 registry-level retirement：`aborted_pre_switch` 要求 manifest=`unarmed` 且 owner final/staging 均不存在；`old` 要求 owner=`prepared|applying`、记录的完整旧 current 已恢复并通过全部 health/search/artifact/identity/hash 验证；`new` 要求 owner=`committed` 且记录的完整新 current 通过同样验证。三者都必须证明 current 全部位于 instance 外、无 live 路径引用 instance，并先收敛六个 cleanup-journal 槽。retirement final basename 固定为 `.hivegui-db-retirement-v1-{role}-{db_instance_operation_id}.json`，状态 staging 固定追加 `.staging`，tombstone basename 固定为 `.hivegui-db-retired-v1-{terminal_outcome}-{role}-{db_instance_operation_id}`，其中 outcome 只允许 `aborted_pre_switch|old|new`；UUID 沿用且绝不复用。journal 至少包含 `schema_version=1`、role/UUID/db_id/outcome、live/tombstone basename、live directory identity、manifest identity/ownership_state、owner identity/phase（aborted 时必须为空）、已验证 terminal current identity/SHA-256、Plugin 根 identity/hash，以及 `prepared|renamed|done`。排他耐久发布 `prepared` 前不得改动 live instance；随后以 identity-bound no-replace 原子 rename 整个 live directory 到 tombstone、fsync registry、复验 tombstone identity，再推进 `renamed`。只有 `renamed` 后才从 tombstone 根逐叶清理；owner/manifest 随目录移动且不得在 live 名称下单独删除。tombstone 清空并删除、registry fsync 后推进 `done`；`done` 且 live/tombstone 均不存在时删除 retirement journal并再次 fsync registry。
- Registry 直接子项的精确允许集合因此固定为 live instance directory、上述三种 outcome 的 tombstone directory、retirement journal final 及其 `.staging`；全部按原始 ASCII 字节总排序后解析，role/UUID/outcome 在名称和记录间必须一一对应。retirement final-only 时先重新 fsync registry并从仍打开句柄复验 journal identity/bytes；`prepared` 时 live 匹配且 tombstone 不存在则重试 rename，live 不存在且 tombstone identity 匹配则补做 registry fsync并推进 `renamed`；`renamed` 时仅 tombstone 匹配则重试逐叶清理，两目录均不存在则补 registry fsync并推进 `done`；`done` 时两目录均不存在则删除 journal并 fsync。final 不存在而 `.staging` 存在只在 live identity 匹配、tombstone 不存在且 outcome 条件仍成立时可删除 staging、fsync registry并重新开始；final+staging 仅在 staging 是同一记录的下一合法状态时删除 staging、fsync registry并按 final 重放。live/tombstone 同时存在、无 journal tombstone、identity/outcome/current hash/记录不匹配、损坏/重复/未知状态均原样保留并 fail-closed。manifest/owner 在 tombstone 内任一 unlink/fsync 边界丢失后，registry-level journal 仍是继续清理的唯一耐久证据。
- 启动固定按 `registry 全量枚举与 retirement final/staging/tombstone 验证 → 已进入 retirement 的 tombstone 重放 → live manifest final/staging 与 owner final/staging 映射 → current/live instance cleanup journal → live owner 重放或 aborted retirement handoff → Store` 处理。tombstone 一旦由匹配 journal 接管，不再要求其中已被逐叶删除的 manifest/owner 参加 live 映射。对 live instance，`armed + 唯一有效 owner final` 必须按 phase 重放；`armed + owner final 缺失/损坏/staging-only` 必须保留全部实例/current并返回 `sidecar_unknown_owner/wal`，覆盖 owner 在 applying/committed 后丢失或并发删除。只有 `unarmed + owner final/staging 均不存在` 才派生 `aborted-pre-switch`，且只能创建 outcome=`aborted_pre_switch` 的 retirement journal并整目录 rename；不得在 live 名称下打开/修复部分数据库或逐项删除。terminal old/new 也只能创建绑定相应 owner/current 证据的 retirement journal。tombstone 内逐叶清理从已打开根句柄按 ASCII 字节序 no-follow 枚举，拒绝 symlink、hardlink、junction/reparse、特殊文件和 link count 不为 1 的普通文件；目录仅做 no-follow、identity-bound 逐层复核，不要求 link count=1。每次 unlink 后 fsync 直接父目录并重新验证 identity；未知/并发条目、identity 变化或无法证明空目录/所有权时保留 journal/tombstone并返回 `sidecar_unknown_owner/wal`，明确 identity-bound unlink/fsync 失败返回 `sidecar_cleanup_failed/wal`。任一边界崩溃后重复流程必须幂等且不得开放 Store。
- 安全清理顺序固定为：通过已打开目录/文件句柄固定 identity → 排他创建并耐久化 `prepared` journal（此时不得修改 sidecar）→ 使用平台等价的 identity-bound、no-replace 原子操作把 canonical 名称移动到同目录唯一 quarantine 名称 → fsync 父目录并从原句柄重新验证 quarantine identity → 耐久推进到 `quarantined` → 从已验证句柄删除 quarantine 并 fsync 父目录 → 耐久推进到 `done` → 删除 journal 并再次 fsync 父目录。平台无法提供身份绑定且 no-replace 的移动语义时，必须在修改 canonical sidecar 前返回 `sidecar_cleanup_failed`。journal 完成并耐久删除前不得创建文件快照、进入恢复切换的 `applying` 或开放 Store。
- 启动时必须在打开业务 Store 前重放 cleanup journal。`prepared` 且 canonical identity 匹配、quarantine 不存在时重试 quarantine；`prepared` 且 canonical 不存在、quarantine identity 匹配时补做父目录 fsync并推进 `quarantined`；`quarantined` 且 quarantine identity 匹配、canonical 不存在时重试删除；`quarantined` 且二者均不存在时视为 unlink 已发生但耐久性未确认，重新 fsync 父目录后推进 `done`；`done` 且 canonical/quarantine 均不存在时删除 journal 并 fsync 父目录。canonical 出现与记录不符的新 identity（包括 `done` 后重新出现）时返回 `sidecar_reappeared`；`done` 时 quarantine 重新出现、canonical/quarantine 同时存在、任一 identity 不匹配、损坏/重复 journal 或状态无法证明时返回 `sidecar_unknown_owner`，保留所有可见文件并阻断。由 identity-bound cleanup、journal 删除或父目录耐久化操作明确报告的失败返回 `sidecar_cleanup_failed`。
- 对 hot、unknown-owner 和 recoverable sidecar，失败后必须仍位于 canonical 名称且字节/hash 不变。对已证明安全并进入 journal 的残留，失败/崩溃后只要求记录的字节完整地位于 canonical 或 quarantine，或者在 `quarantined` 后已经删除且父目录可通过重放完成耐久确认；不得承诺所有清理失败都能恢复原 canonical 路径。任何状态都不得让 canonical sidecar 与不同主文件组合。
- 安全快照的数据库文件、托管 Plugin 树与快照清单必须全部持久化并通过哈希验证后才算可用；数据库主文件是按字节精确的本地回滚快照，必须保留 Plugin operation/GC ledger、派生搜索表等全部内部表，不得按可移植 archive 的实体过滤规则改写。恢复该快照时先重放持久化恢复日志，且禁止将快照主文件与目标位置遗留的 sidecar 组合。

## 5. 失败行为

- 在安全快照创建完成前发生的 checkpoint、连接关闭、sidecar 分类或 cleanup journal 失败，不得声称回滚/恢复一个尚不存在的快照；必须保留 current 主文件、全部受保护 sidecar、cleanup journal/quarantine，保持 Store 关闭并只提供重放/重试或退出。安全快照验证后、manifest 仍为 `unarmed` 时，迁移 SQL、staging SQLite commit、staging 重开验证、sidecar 或 fsync 失败只能 rollback/丢弃 staging，并通过 `terminal_outcome=aborted_pre_switch` 的 retirement 状态机收口；current 与 Plugin 根从未改变，不得声称从快照恢复。owner 已发布后，`prepared|applying` 的失败才按 owner/安全快照恢复并验证完整旧 current，再以 outcome=`old` 收口；`committed` 后不得回旧，只能完成、验证新 current 并以 outcome=`new` 收口。
- staging 迁移事务内或其 commit 后重开时的目标健康检查失败都属于迁移失败；不得误报为源数据库损坏，也不得提供“保留损坏文件并重建”。SQLite commit 仍只是未发布 staging 内部状态；只有 owner `committed` 是系统 commit point。
- 迁移失败时主业务 Store 不得进入可写状态；应用显示迁移恢复界面，只提供“重试迁移”和“退出”，不得提供绕过迁移的重建。
- 既有源数据库在迁移前的任一健康检查失败（结构损坏或孤儿外键）属于数据库损坏而非迁移失败。应用必须阻止主界面，提供“从备份恢复”“保留损坏文件并重建”和“退出”；重建前二次确认，并先把损坏数据库复制到带时间戳的隔离位置。未确认前不得修改数据库或托管 Plugin。
- 错误信息包含失败版本步骤、稳定错误类别、execution_id 和快照位置，不包含密钥或 SQL 参数明文。

## 6. 迁移内容

- v2→v3：Plugin 资源覆盖、Agent 默认标志、AgentTool/AgentSkill/AgentCapability、约束和索引；Function kind `1/2/3` 精确映射为 `builtin/custom/placeholder`，Tool kind `1/2` 精确映射为 `function-wrap/workflow-wrap`，任一类型出现未知值都失败；真实旧点号 Builtin 精确重命名为四个下划线 identifier，目标名称碰撞时整个事务回滚，不合并记录或保留别名。
- v3→v4：ChatSession、ChatMessage、AgentExecution、会话到期索引、LLM 表关系/索引收口、`plugins.row_revision NOT NULL DEFAULT 0`（既有行回填 0）、`plugin_artifact_operations`、`plugin_artifact_gc`，以及 `schema_metadata`、`search_documents`、FTS5 trigram 虚拟表和 short-gram 表/覆盖索引。operations DDL 必须包含由 operation_id 确定性派生且 UNIQUE 的 `staging_name`、nullable `staging_identity`、UNIQUE `new_s3_key`、`prepared|staged|published|referenced|done|conflict` 状态，以及 create expected-old 始终为空、create `plugin_id` 在 `prepared|staged|published` 为空且在 `referenced` 非空、replace plugin_id/旧 tuple/revision 始终非空的 CHECK；状态 CHECK 还必须要求 `prepared` 两项 identity 为空、`staged` 仅 `staging_identity` 非空、`published|referenced` 两项 identity 均非空，`done` 要求 `new_identity IS NULL OR staging_identity IS NOT NULL`，`conflict` 不限制两项 identity。在同一迁移事务内以 `hivegui-nfkc-casefold-v1` 回填全部可搜索字段、写入 normalization ID，并验证 Plugin 内部表 CHECK/UNIQUE 约束及实体、FTS 和 short-gram 派生数据一致。新建 v4 使用同一完整 DDL；运行时 Store 禁止补做这些结构。

所有迁移脚本必须可在事务 rollback 后重新执行；不得在迁移函数外执行散落的 `ALTER TABLE`。

## 7. 测试矩阵

| 输入 | 结果 |
| --- | --- |
| 空目录 | trigram 可用时创建 v4，写入 `hivegui-nfkc-casefold-v1`，commit 前与重开后健康/搜索索引检查均通过，启动成功 |
| v4 | 不迁移，健康、trigram、normalization ID 与搜索索引一致性检查通过后启动成功 |
| v3 | 同事务创建、回填并验证 FTS5 trigram/short-gram 索引后迁移到 v4，数据/关系/制品不变 |
| v2 | 顺序迁移到 v4，数据/关系/制品不变 |
| v1 | 拒绝并提示中间版本 |
| v5 | 拒绝且不修改文件 |
| 既有数据库结构或索引损坏 | `integrity_check` 拒绝，进入损坏恢复界面，主 UI 不开放 |
| `foreign_keys=OFF` 时构造的孤儿引用 | `foreign_key_check` 拒绝，进入损坏恢复界面，主 UI 不开放 |
| 新建 v4 在 commit 前产生结构或外键违规 | rollback/删除未发布文件，主 UI 不开放 |
| migration staging 在 SQLite commit 前产生结构或外键违规 | 只 rollback staging transaction，current/Plugin 字节不变，进入迁移失败界面 |
| migration staging SQLite commit 后重开验证故障注入 | SQLite commit 不发布系统状态；通过 aborted retirement 丢弃 staging，current/Plugin 字节不变，不声称从快照恢复 |
| owner `prepared|applying` 的每个切换边界失败/崩溃 | 按 owner/安全快照恢复并验证完整旧 current，以 retirement outcome=`old` 收口 |
| 新 current 完整验证后发布 owner `committed` 的每个边界失败/崩溃 | final-only 先补实例目录 fsync/复验；`committed` 前仍恢复 old，已耐久 `committed` 后只完成 new，再以 outcome=`new` 收口 |
| `.hivegui-db-staging-v1` 不存在或为空 | 精确表示零 staging 实例；继续处理当前库，不制造 registry，也不误报恢复阻断 |
| 在 staging 数据库创建前、instance manifest 发布或其父目录 fsync 任一边界崩溃 | 启动从固定 registry 发现该精确实例目录；缺失/未耐久/损坏 manifest 原样保留并以 `sidecar_unknown_owner/wal` fail-closed，不猜测删除 |
| `unarmed` manifest 已耐久，但在建库前崩溃且 owner final/staging 均无 | 六槽收敛后派生 `aborted-pre-switch`；先耐久创建 outcome=`aborted_pre_switch` retirement journal，再整目录 rename 到 tombstone，current 字节/引用不变 |
| `unarmed` manifest 下只存在部分数据库/Plugin payload 且无 owner | 不打开或修复 payload；同样先 retirement handoff，再在 tombstone 中按 ASCII 字节序 no-follow 清理；普通文件必须 link-count=1，目录只做 identity-bound 逐层复核 |
| `armed` manifest 与唯一匹配 owner final 同时存在 | 无论数据库是否缺失或部分存在，都绝不归类为孤儿；先收敛 cleanup journal，再按 owner phase 重放 |
| `armed` manifest 下 owner final 缺失/被删除、staging-only、损坏、重复或不匹配 | 保留全部 live instance 与 current，返回确定性 `sidecar_unknown_owner/wal`；不得以 owner absence 推断 aborted |
| manifest/owner final-only、final+staging、staging-only 的每个写/replace/fsync 边界 | final-only 补目录 fsync和身份复验；合法 final+下一 phase staging 删除 staging后按 final 重放；staging-only或不匹配 fail-closed |
| registry 含未知条目、链接/特殊文件、组间 UUID 复用、目录名/manifest/db_id/database_name/ownership_state 不匹配 | 按原始 ASCII 字节序产生确定性阻断；不打开实例数据库、不处理 owner、不修改未知条目 |
| retirement `prepared|renamed|done` 的 journal staging/final、整目录 rename、tombstone 每个 unlink/fsync、rmdir、journal 删除边界崩溃 | 按 live/tombstone/journal identity 和 outcome 确定性重放；manifest/owner 缺失只在 journal 已接管的 tombstone 内可继续；无 journal tombstone、双目录或 identity/hash 不匹配 fail-closed |
| instance cleanup 与迁移/恢复达到 terminal old/new | owner/manifest 随 live instance 整体进入 outcome tombstone后才逐叶删除；registry/manifest/owner/retirement journal/tombstone 从未进入 archive、安全备份或新树 |
| 有已提交但未 checkpoint 帧的 WAL | 冻结写入并成功 checkpoint/关闭后快照包含全部已提交数据；每个故障点都不丢帧 |
| checkpoint 非 busy 失败/busy、连接未关闭或 hot/未知/仍含可恢复状态的 WAL/SHM/journal | 返回精确 `storage_recovery_blocked { reason, artifact }`，按 local-runtime 合法配对及固定优先级选择；hot/unknown/recoverable canonical sidecar 字节不变，在文件快照/发布前 fail-closed，主 UI 不开放，不复制、不盲删或切换不完整主文件 |
| 安全残留 cleanup journal 的每个持久化/rename/unlink 边界崩溃 | 按 `prepared→quarantined→done` 表重放；记录字节只可位于 canonical 或 quarantine，或在 `quarantined` 后已删除并由父目录 fsync 确认；journal 耐久删除前不快照、不进入 `applying`、不开放 Store |
| `done` 已耐久但 journal 尚未删除时崩溃 | canonical/quarantine 均不存在时删除 journal 并 fsync 父目录；出现 sidecar、journal 损坏/重复或身份不符时按固定错误映射阻断 |
| 目标位置遗留旧 `-wal`/`-shm`/journal | 不得与新主文件组合；只按 cleanup/恢复日志和身份绑定协议处理，身份不符或无法证明时原样保留并 fail-closed |
| FTS5 trigram tokenizer 不可用 | 新建、打开或迁移均 fail-closed，不创建部分 v4、不回退扫描 |
| normalization ID 缺失/未知 | 仅允许明确 schema 迁移重建；否则在开放 Store 前 fail-closed |
| v3→v4 搜索回填或一致性验证失败 | 整个迁移 rollback，旧库与快照保持可用 |
| v2/v3 任一步故障注入 | rollback，主 UI 不开放，快照可恢复 |
| v2 的 Function kind `1/2/3` | 映射为三种稳定字符串且数据不变 |
| v2 的 Tool kind `1/2` | 映射为 `function-wrap/workflow-wrap` 且数据与关系不变 |
| v2 的未知 Function kind | 拒绝、rollback，快照可恢复 |
| v2 的未知 Tool kind | 拒绝、rollback，快照可恢复 |
| 旧点号 Builtin | 事务重命名为下划线 identifier |
| 点号与下划线 Builtin 同时存在 | 冲突、rollback，不合并记录 |

## 8. 设备密钥启动门禁

- 首次启动且不存在加密数据时，使用系统安全随机源原子创建设备密钥；Unix 权限为 0600，其他平台使用仅当前用户可读写的等效 ACL。并发首次启动只能有一个最终密钥生效。
- 已存在加密数据时，密钥缺失、长度错误、权限不安全或不可读分别返回稳定的 `key_missing`、`key_corrupt`、`key_permissions`，不得静默生成替代密钥、覆盖数据库或开放主业务 Store。
- 密钥门禁失败与 schema 迁移失败、数据库完整性损坏是独立状态；UI 只能提供重新配置、从加密备份恢复或退出等不会修改现有密文的操作。
