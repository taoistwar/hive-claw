# Quickstart: Validate HiveGUI as an Independent Local Agent

**Feature**: `011-hivegui-standalone-mode`
**Audience**: implementers and reviewers
**Contracts**: [contracts/README.md](contracts/README.md)
**Data model**: [data-model.md](data-model.md)

本指南描述实现期间与完成后的可运行验收。它不使用 HiveWeb，不要求 MySQL/Redis/Rustfs；外部 MySQL 数据源和用户配置的 LLM 仅在对应场景中按需使用。

## 1. Prerequisites

- Rust 1.97.1（2026-07-22 的最新稳定版，由 `rust-toolchain.toml` 精确固定），workspace `rust-version=1.97.1`。
- `sqlx-cli =0.9.0`（CI 与 workspace SQLx 精确一致）；最终验证按 SQLite/MySQL/rustls feature 安装，不使用浮动版本。
- SQLx crate feature 必须分层：HiveGUI 精确为 `chrono|macros|runtime-tokio|sqlite`，HiveWeb 精确为 `chrono|json|macros|mysql|runtime-tokio|rust_decimal|tls-rustls-ring-webpki`；`macros` 已包含 derive，两端均不重复声明 `derive`，workspace 根不聚合 feature。
- 当前平台所需 GPUI 系统库与图形会话。
- `wasm32-unknown-unknown` target，用于构建/更新 Plugin fixture。
- HiveWeb MySQL TLS 集成测试使用临时 CA/服务器证书、与连接 URL 一致的测试 hostname，并只接受 `VERIFY_IDENTITY`；不得使用真实生产证书或把私钥提交到仓库。
- HiveWeb 服务启动时，主库必须同时设置 `DATABASE_URL`（查询参数含 `ssl-mode=VERIFY_IDENTITY`）、`DATABASE_TLS_CA`（PEM CA 文件）和 `DATABASE_TLS_HOSTNAME`（与 URL host 逐字一致）。启用 `EXTERNAL_DB_URL` 时同样必须设置 `EXTERNAL_DATABASE_TLS_CA` 与 `EXTERNAL_DATABASE_TLS_HOSTNAME`；缺失、宽松模式、CA 无效或 hostname 不一致均 fail-closed，且不会退回明文连接。
- 不提交真实 API key；LLM 端到端测试使用进程内 mock server。

```bash
rustup target add wasm32-unknown-unknown
cargo check -p hivegui --lib
```

### 复制执行清单（可直接复现）

```bash
# 进入项目根目录
cd /home/developer/agent/gpui-claw/hive-claw-worktree

# 本地快速启动前置
git status --short
git diff --check
cargo fmt --all -- --check
rustup target add wasm32-unknown-unknown

# 先验 red/依赖门禁
cargo test -p hivegui --test logging_contract -- --nocapture
cargo test -p hive-runtime-core --test capability_contract -- --nocapture
cargo test -p hive-runtime-core --test persisted_tool_contract -- --nocapture
cargo test -p hivegui --test sqlite_health_contract -- --nocapture
cargo test -p hivegui --test plugin_artifact_schema_contract -- --nocapture

# 快速核验：独立运行与关键契约
cargo test -p hivegui --test hiveweb_independence -- --nocapture
cargo test -p hivegui --test function_test_execution -- --nocapture
cargo test -p hivegui --test workflow_execution -- --nocapture
cargo test -p hivegui --test plugin_compatibility -- --nocapture

# 可复现 fixture
cargo test -p hivegui --test migration_compatibility -- --nocapture
cargo test -p hivegui --test backup_restore -- --nocapture

# 本地 SQLx 离线元数据与安全扫描
SQLX_OFFLINE=true cargo sqlx prepare --workspace --check
cargo deny check advisories

# 完整验收
cargo test -p hive-runtime-core
cargo test -p hivegui --tests

# 启动独立 GUI（确认主流程）
hivegui_test_root="$(mktemp -d)"
XDG_DATA_HOME="$hivegui_test_root/data" \
HIVEGUI_LOG_DIR="$hivegui_test_root/logs" \
cargo run -p hivegui
```

## 2. Protect the workspace

当前分支可能包含未提交 HiveGUI 修改。开始每个 TDD 批次前先检查范围：

```bash
git status --short
git diff --check
```

不要清理、覆盖或暂存无关文件。严格执行：先写测试 → reviewer 审阅 → 运行并看到预期失败 → 最小实现 → 运行通过 → 重构。

### Blocking Red gates

在任何对应生产实现前，先分别运行并记录 T016A-T016F 与 T017G 的可识别 Red；不要把编译错误、ignore、未来故事未激活行或批量命令中被遮蔽的失败当作证据：

```bash
cargo test -p hivegui --test logging_contract -- --nocapture
cargo test -p hive-runtime-core --test capability_contract -- --nocapture
cargo test -p hive-runtime-core --test persisted_tool_contract -- --nocapture
cargo test -p hivegui --test sqlite_health_contract -- --nocapture
cargo test -p hivegui --test plugin_artifact_schema_contract -- --nocapture
cargo test -p hivegui --test management_scroll_contract -- --nocapture
cargo test -p hivegui --test sensitive_persistence_contract -- --nocapture
cargo test -p hivegui --test search_index_contract -- --nocapture
```

T017F 审批 T016A-T016F 的 Foundation 行；T017H 单独审批 T017G。原生滚动与敏感介质 inventory 中的未来故事行必须由各故事 Tests/Reviewer 激活并先观察 Red，T138/T139/T142 只能复跑已经闭合的行。

## 3. Existing green baseline

这些命令在规划期已通过，可用于确认现有局部能力没有先行退化：

```bash
cargo test -p hivegui --test desktop_host_call
cargo test -p hivegui --test runtime_capability_catalog
cargo test -p hivegui --test wasm_exports_test
cargo test -p hivegui --lib ui::dag_editor_view::tests
```

已知旧测试 `integration_test::test_schema_version_management` 仍期待 schema 1，而当前实现已是 2。迁移批次必须先把它改写为 [storage-migration.md](contracts/storage-migration.md) 的 v2/v3/v4 矩阵，并确认旧断言失败。

## 4. Isolated desktop run

使用独立 XDG 数据目录，避免接触日常数据库：

```bash
hivegui_test_root="$(mktemp -d)"
XDG_DATA_HOME="$hivegui_test_root/data" \
HIVEGUI_LOG_DIR="$hivegui_test_root/logs" \
cargo run -p hivegui
```

预期：应用打开 Home，可进入 Ai 与 Tools；数据、密钥、Plugin 和日志只出现在临时根目录。`HIVEGUI_HEADLESS=1` 当前在 Store 初始化前退出，只能做链接冒烟，不能作为本地 Agent、迁移或数据库验收。

## 5. Shared contract and independence

实现 `hive-runtime-core` 后运行：

```bash
cargo test -p hive-runtime-core
cargo test -p hivegui --test hiveweb_independence -- --nocapture
cargo test -p hivegui --test plugin_compatibility -- --nocapture
```

预期：

- HiveGUI 依赖图不包含 hiveweb。
- HiveWeb 未运行时，mock LLM + 本地 Tool 对话完成。
- 网络捕获中没有指向 HiveWeb 的请求或 fallback。
- 同一 WASM fixture 在两端产生等价 ABI success/denied/error envelope。

## 6. Agent resources and routing

```bash
cargo test -p hivegui --test local_agent_runtime -- --nocapture
```

场景：创建默认根 Agent、直接子 Agent 和更深后代；分配 Tool/Skill/Capability；验证 `is_always` 并集去重；完成 LLM→Tool→Function/Plugin 对话；拒绝缺 Capability、跨级、循环和 depth>10 的路由；直接调用 runtime 命令验证 UUID、消息长度、保留期和控制字符边界。

预期：每轮产生唯一 execution_id，全部 runtime 事件可关联；任何失败不请求 HiveWeb。

## 7. Workflow

先验证 Function/Builtin 契约：

```bash
cargo test -p hive-builtins
cargo test -p hivegui --test function_management -- --nocapture
cargo test -p hivegui --test function_test_execution -- --nocapture
```

预期：四个下划线 Builtin identifier 可执行；四个点号名称全部 lookup/execute 失败且没有别名记录；旧整数 `3` 迁移为 `placeholder`；Placeholder 的两个公开执行入口都在 Capability/Plugin 查找前返回 `function_not_executable`。

```bash
cargo test -p hivegui --test workflow_execution -- --nocapture
```

场景：保存并重载使用 `start_node|end_node|function_node|generate_answer_node` 的合法 DAG；执行含并行层的 100 节点 no-op 图；拒绝短节点名称、重复键、悬空边、循环、非法 start/end 结构；注入节点失败并验证 fail-fast、无自动重试、已完成副作用提示。

预期：图保存是单事务；失败后新节点调度数为 0，结果完整区分 completed/failed/skipped。

## 8. Plugin security and limits

```bash
cargo test -p hivegui --test plugin_compatibility -- --nocapture
cargo test -p hivegui --test desktop_host_call -- --nocapture
cargo test -p hivegui --test plugin_artifacts -- --nocapture
```

场景：导入后删除源文件仍执行；缺失/篡改、路径逃逸、最终/中间 symlink、hardlink、junction/reparse、特殊文件和检查后替换全部被拒，根外 sentinel I/O 为 0；最终 WASM 是 link-count 1 的普通文件并由同一已验证句柄交给 runtime。empty/v3/current-v4 fixture 证明 `plugins.row_revision`、operation/GC 表、确定性 UNIQUE staging、六态 operation CHECK 只由 migrations 创建且结构漂移 fail-closed。预校验失败时用户表和两个 ledger 均零；成功后执行 prepared→staged→published→referenced，create 的用户行插入/plugin_id/referenced 同事务，replace 以旧 tuple+revision live CAS 且并发更新恰好一个成功。重放逐项覆盖 prepared/staged/published 的 staging/final/identity 组合：唯一合法组合继续或完成；ownership/identity/双重存在歧义一律把 operation 置 `conflict`、保留对象并阻断 Store。create 无用户行不得重建意图，replace 仍为旧 tuple 时不得重做 CAS，两者都只以“登记 owned object GC + operation→done”事务收敛；referenced 是历史事实。GC 只允许 `pending|blocked`，并在启动、固定周期、引用/租约释放事件后按 artifact_key 稳定扫描；瞬态引用/租约解除且 identity 仍匹配时重试成功，identity 重现/不匹配或所有权未知持续 blocked 且绝不采用竞争对象。对每个写入/fsync/rename/CAS/GC/unlink/ledger 边界注入崩溃；默认/最大资源限制、WASI 拒绝、真实 Capability 和实例池矩阵全部通过。

预期：发布只创建不可变新键，从不覆盖旧键或竞争者文件；并发更新无 last-writer-wins，操作日志/GC ledger 重放不误删未知对象。被 timeout、强制取消、trap 或超限的实例不回池，主 UI 和其它会话继续运行。

## 9. Conversation encryption and retention

```bash
cargo test -p hivegui --test conversation_retention -- --nocapture
```

场景：完成对话后重启恢复；新会话 `expires_at` 为创建时间后100个日历年；修改保留规则、删除单会话和清空历史均先报告数量；级联删除消息与 execution，不删除 Agent。

预期：SQLite 原始字节扫描找不到消息正文、Tool 参数或结果；诊断包不含会话数据。

## 10. Backup and restore

```bash
cargo test -p hivegui --test backup_restore -- --nocapture
```

测试 fixtures 覆盖 format current/current-1/current-2、newer、too-old、wrong-password、认证流末尾 tampered、前段合法后段失败、missing-artifact、path-traversal、同名/并发创建最终导出目标、含已提交未 checkpoint 帧的 WAL、hot journal 和跨文件系统 staging。

预期：

- 正确口令可在新设备密钥下恢复全部实体、关系、会话和 WASM。
- FTS/short-gram 不从包内 shadow 数据恢复，而是按 `hivegui-nfkc-casefold-v1` 从实体重建并通过结果/索引一致性检查。
- 恢复前验证每个 `Agent.model_preset` 均指向包内存在的 Preset；悬空引用在修改现状前被拒绝。
- 错误口令、认证流末尾失败、路径/哈希错误均在修改 current 前失败；unarmed/no-owner restore instance 通过 outcome=`aborted_pre_switch` retirement 整目录移入 tombstone 后清理，任何 identity/owner 歧义原样 fail-closed。
- 已存在或并发创建的导出目标不被覆盖；no-replace 冲突要求选择新名称。
- 在用户确认前写入新记录，确认后立即冻结；先完成 current/staging 的非 busy `wal_checkpoint(TRUNCATE)`、关闭连接以及已提交未 checkpoint WAL 与 hot/unknown/recoverable sidecar 分流，再从封闭 current 生成并验证安全备份。后者必须保持 canonical 名称和字节；已证明安全的残留只经同目录 quarantine 与 SQLite 外部 `prepared→quarantined→done` cleanup journal 清理。current 固定 `datasources.db`/`db_id=current`；restore instance 固定 `.hivegui-db-staging-v1/restore-{UUID}/datasources.db`。建库前发布六元组 unarmed manifest；应用前 arm manifest并通过 owner 双槽发布 `prepared|applying`。对 manifest/owner final-only/final+staging/staging-only、armed owner 丢失、每次 fsync、数据库/制品切换及新 current health/search/artifact/identity 验证逐一注入故障；新 current 验证成功后才可 committed。prepared/applying 恢复并验证 old，committed 后只收口已验证 new。启动先重放 retirement/tombstone，再处理 live manifest/owner、各数据库 cleanup journal 和 owner；安全备份生成前故障保持 current 与关闭的 Store，不声称回滚不存在的备份。任何路径都不得丢失已提交 WAL 帧、组合旧 sidecar 或暴露混合状态。
- 单独注入两类 unarmed/no-owner 崩溃：manifest 已 fsync 但 `datasources.db` 尚未创建，以及数据库/Plugin payload 只写入一部分。manifest 六元组 final/staging 精确为 `.hivegui-db-instance-v1.json`/`.hivegui-db-instance-v1.json.staging`，owner final/staging 精确为 `.hivegui-db-recovery-v1.json`/`.hivegui-db-recovery-v1.json.staging`。只有 unarmed 且 owner 双槽都无才可 outcome=`aborted_pre_switch`；armed 后 owner 缺失/staging-only/损坏必须原样 fail-closed。aborted/old/new 都先发布 retirement final `.hivegui-db-retirement-v1-{role}-{UUID}.json` 或 staging `.hivegui-db-retirement-v1-{role}-{UUID}.json.staging`，按 `prepared→renamed→done` 把整个 live instance identity-bound no-replace rename 到 `.hivegui-db-retired-v1-{outcome}-{role}-{UUID}`；只有匹配 journal 的 tombstone 可逐叶删除、逐级 fsync、rmdir，最后删除 journal并 fsync registry。再加入 owner 正例、owner/manifest/retirement 歧义、unknown tombstone、live+tombstone 双重存在、symlink/hardlink/特殊文件、并发条目与 identity 改变负例；全部歧义保留并 fail-closed，重复启动后 current 字节/引用始终完整 old 或完整 new。
- 设备密钥、备份口令和敏感明文不出现在包、日志或未加密暂存文件中。

## 11. Schema migration

```bash
cargo test -p hivegui --test migration_compatibility -- --nocapture
cargo test -p hivegui --test plugin_artifact_schema_contract -- --nocapture
cargo test -p hivegui --test sqlite_health_contract -- --nocapture
```

预期：FTS5 trigram 可用时空库创建 v4 并写入 `search_normalization_id=hivegui-nfkc-casefold-v1`；v2/v3 顺序迁移且数据/关系/制品不变，v3→v4 同事务创建 `plugins.row_revision`、含 staging ownership/`staged` 状态的 operation/GC 表及约束，并回填/验证 FTS/short-gram 索引；同版本 v4 缺结构或约束漂移 fail-closed，运行时 Store 不补 DDL。tokenizer 不可用、normalization ID 未知、回填失败或索引不一致均 fail-closed。Function kind `1/2/3` 映射为 `builtin/custom/placeholder`，Tool kind `1/2` 映射为 `function-wrap|workflow-wrap`；真实旧点号 Builtin 事务重命名为下划线名称，目标碰撞或未知 Function/Tool kind 时整体 rollback；v1和v5明确拒绝。current 与固定 migration/restore registry 实例的 manifest 发现/异常/lifecycle fixture，以及含已提交未 checkpoint 帧的 WAL fixture，必须在冻结写入、checkpoint、关闭连接、确定性 cleanup journal/sidecar 分流、五分支重放、主文件/父目录 fsync 的每个故障点保持全部已提交数据；多故障返回值必须符合 local-runtime 的合法 reason/artifact 配对及固定总优先级。hot/unknown/recoverable canonical WAL 或 rollback journal fixture 逐字节原样保留且主业务 UI 不开放；安全残留按 canonical/quarantine/耐久删除允许状态幂等收敛，绝不盲删。

## 12. Cancellation

```bash
cargo test -p hivegui --test cancellation -- --nocapture
```

分别在 Agent 路由、LLM、Tool、Workflow 每一层和 Plugin 内触发停止。

预期：停止后新工作调度数为 0；可取消网络 future 被取消；不协作 Plugin 在2秒宽限后由 CancelHandle 中断；AgentExecution 终态为 cancelled；其它会话与主 UI 可用；已完成外部副作用不被描述为已回滚。

## 13. Performance

```bash
cargo +1.97.1 bench --locked -p hivegui --bench local_runtime -- --manifest
cargo +1.97.1 bench --locked -p hivegui --bench local_runtime -- --run-matrix
```

无 `--run <id>` 或 `--run-matrix` 时 bench 默认只打印 manifest，不执行测量。`--run-matrix` 使用源码持有、经评审的干扰最小化顺序，为每个目标启动同一精确 bench 可执行文件的新子进程，并在全部 13 个目标执行后输出机器可读汇总。该顺序降低前序重负载干扰，但不控制主机 affinity，矩阵结果仍可证伪。报告必须包含环境、样本量和 p50/p95/p99，并分别列出：

- LLM 决策解析到下一本地动作调度，p95≤200ms。
- Tool 校验完成到执行器启动，p95≤50ms。
- 固定100节点 no-op DAG 调度，p95≤100ms。

外部 LLM、网络和用户 Function/Plugin 实际执行耗时单列，不可混入本地预算。

## 14. GPUI responsiveness and accessibility

```bash
cargo test -p hivegui --test accessibility -- --nocapture
cargo test -p hivegui --lib ui:: -- --nocapture
```

VisualTestContext 驱动全部关键流程的 keyboard-only 路径、DAG 节点/边、modal focus trap/restore 和 Stop。主题测试计算对比度；AccessKit hook 验证名称/角色/状态/错误。

预期：输入反馈 p95≤100ms，主线程最大连续阻塞≤250ms，后台压力下 Stop 可用率100%，文本≥4.5:1、焦点和关键图形≥3:1。合并前在 Linux/macOS/Windows 各完成一次真实辅助技术 smoke test。

## 15. Storage plans, resilience, and CI security

HiveGUI 外部 MySQL 验收使用独立 CI job 与专用 `HIVEGUI_TEST_MYSQL_URL`，动态创建并销毁隔离 schema/account。它可以复用 MySQL 8 镜像配置，但不得读取仓库 `.env`，也不得依赖 HiveWeb 的 `TEST_DATABASE_URL`、migration、server、client 或运行状态；这只是 HiveGUI 按用户配置直连外部数据库的测试，不建立 HiveGUI→HiveWeb 请求。

依赖安全补救先运行以下 Red contract；它们全部写好并得到 reviewer 批准前，不得修改 Cargo、锁文件或生产连接代码：

```bash
cargo test -p hivegui --test ci_security_contract approved_dependency_remediation -- --nocapture
cargo test -p hiveweb --test contract_mysql_tls_policy -- --nocapture
cargo test -p hiveweb --test contract_sqlx_09_sql_safety -- --nocapture
```

预期 Red 必须可识别地指出尚未满足的精确依赖/feature/lock/vendor、缺失的严格 MySQL TLS 公共边界或未经审计的 SQLx 0.9 查询；无关语法错误或模糊编译失败不算有效 Red。Green 时必须证明：`mysql_async =0.37.0` 使用最小 native-TLS feature；SQLx/CLI 为 0.9.0 且图中无 `mysql-rsa`/`rsa`；HiveGUI SQLx 精确启用 `chrono|macros|runtime-tokio|sqlite` 且不编译 MySQL/TLS；HiveWeb 精确启用 `chrono|json|macros|mysql|runtime-tokio|rust_decimal|tls-rustls-ring-webpki`，固定应用 SQL 使用 checked macros、生产 `QueryBuilder` 为零且只用带 CA/hostname 的 `VERIFY_IDENTITY`；AWS S3 不激活旧 `rustls`；Wayland vendor 来源精确为 `https://github.com/smithay/wayland-rs` 且两处最小兼容差异可审计；锁文件不含 7 个 advisory 对应包且 `deny.toml` 无 ignore。

SQL 安全 Green 还必须直接覆盖：`game_service.category_name` 的恶意引号/注释字符串只作为绑定值、查询结构不变；应用 schema 乐观锁只接受封闭表 enum；`named_queries.toml` 只由一个 reviewed-config 中央边界处理，并对多语句、SQL 注释、placeholder/参数不匹配、重复参数和 select/execute kind 不匹配全部 fail-closed；source contract 必须证明其它生产调用点不能构造 `AssertSqlSafe`。HiveGUI 外部 MySQL 的 `MysqlIdentifier` 仍只服务 metadata allowlist 后的本地直连，不得复用为 HiveWeb 应用 schema 类型或建立 HiveGUI→HiveWeb 请求。

```bash
cargo test -p hivegui --test storage_query_plans -- --nocapture
cargo test -p hivegui --test search_index_contract -- --nocapture
cargo test -p hivegui --test query_count -- --nocapture
cargo test -p hivegui --test sql_safety_contract -- --nocapture
cargo test -p hivegui --test relationship_scope_contract -- --nocapture
cargo test -p hivegui --test store_resilience -- --nocapture
cargo test -p hivegui --test entity_validation -- --nocapture
cargo test -p hivegui --test device_key_lifecycle -- --nocapture
cargo test -p hivegui --test diagnostics_exactly_once -- --nocapture
cargo test -p hivegui --test logging_contract -- --nocapture
cargo test -p hivegui --test sqlite_health_contract -- --nocapture
cargo test -p hivegui --test ci_security_contract -- --nocapture
```

预期：每个含过滤或关联条件的生产查询的 EXPLAIN 计划都命中预期索引并覆盖过滤/关联列，非小型表没有未经批准的扫描；小型固定表/metadata 例外完整记录表大小、理由、审批者、到期日和复核条件总结。`hivegui-nfkc-casefold-v1` 必须逐标量使用官方 Unicode 17.0.0 `NFKC_CF` 映射并以 Unicode 17.0.0 NFC 收口；contract 同时验证 `unicode-normalization` 的 `UNICODE_VERSION=(17,0,0)`、官方源文件/生成表校验值与 golden fixture，原始非空 search 规范化为空返回 `empty_after_normalization`，1/2/3+ 字符分别命中 short-gram/FTS 索引且 tokenizer/ID 不可用时 fail-closed。Category、Agent 资源、Workflow 整图及会话消息/执行批量加载的查询次数不随数据量线性增长。SQLite 动态值只 bind，MySQL 值使用 prepared bind，数据库/表/列名只由精确 metadata allowlist 后的 `MysqlIdentifier` 序列化。v4 关系表、公开 DTO/Store/UI 只包含规格白名单，不出现 Tag 任意关系入口。公开 Store/导入边界覆盖三种 Function.kind、四种 `*_node`、点号 Builtin 负例、DataSource、GlobalConfig、LlmProvider、LlmPreset、分页/搜索和 Plugin 数值限制，拒绝 NUL/控制字符并保证零修改；分页/搜索错误的稳定 reason 精确为 `page=0 → out_of_range`、`page_size!=20 → fixed_value_required`、`search>255 → too_long`、`NUL/控制字符 → control_character` 和 `非空规范化为空 → empty_after_normalization`；重复 identifier 以及 Category children、Provider→Model、Plugin→Function、Function→Workflow/Tool、默认 Agent、Preset→Agent 冲突均返回精确安全 envelope 且零修改。LlmProvider 持久化只使用 `category/base_url/token_encrypted/token_env`，旧字段仅在迁移 fixture 中出现；设备密钥异常不会被静默替换；第二实例被拒绝；临时存储错误严格按1s/2s/4s重试；v1 日志经可注入时钟验证每条记录实际不超过 7×24 小时且总量不超过 100,000,000 bytes，`cause_summary` 中央脱敏且同一内部错误只在处理边界记录一次；CI 中固定版本 secret/dependency/SQLx 检查均为阻断步骤。

固定性能数据集还必须验证：DataSource/LLM/Tag/Category/Capability/Plugin/Function/Workflow/Tool/Skill/Agent CRUD p95≤1s、连接测试5秒超时、搜索/翻页 p95≤500ms，以及100+ Category 从加载到可见 p95≤200ms。每项记录版本化基线与环境指纹并比较 p50/p95/p99；任何无明确签字、记录理由、影响范围和到期复核日期的 >10% 回归都阻断。

## 16. Final quality gates

```bash
cargo +1.97.1 fmt --all -- --check
cargo +1.97.1 clippy --locked -p hive-runtime-core -p agent -p hive-builtins -p hivegui --all-targets -- -D warnings
cargo +1.97.1 test --locked -p hive-runtime-core
cargo +1.97.1 test --locked -p agent
cargo +1.97.1 test --locked -p hive-builtins
cargo +1.97.1 test --locked -p hivegui --lib
cargo +1.97.1 test --locked -p hivegui --tests --no-fail-fast
cargo +1.97.1 check --workspace --all-targets --locked
DATABASE_URL=sqlite::memory: SQLX_OFFLINE=true cargo +1.97.1 sqlx prepare --workspace --check --no-dotenv
cargo deny check advisories
git diff --check
```

CI 还必须使用 `cargo install --locked sqlx-cli --version 0.9.0 --no-default-features --features sqlite,mysql,rustls` 安装 CLI，并执行固定版本的 secret scanner 与 `cargo-deny`；工具安装失败、步骤缺失、SQLx metadata 缺失/陈旧、任一 advisory、advisory ignore 或命令失败都必须阻断。本地一次性扫描不能替代该门禁。最终质量门还必须复跑已审批 benchmark，比较版本化基线并阻断没有明确签字、记录理由、影响范围和到期复核日期的 >10% 回归。

涉及 backup crypto、设备密钥、Plugin sandbox 和 Capability 的批次还必须完成 dependency scan、secret scan、专门 security review 与第二审批。

T001 当前只有已推送的远端分支 `codex/rust-1.97.1-toolchain`/提交 `bf3690d`；自动化 PR API 权限与远端 CI 尚未取得。用户已批准先闭合独立质量基线 `codex/ci-quality-baseline-rust-1.97.1`：现役 HiveWeb integration job 必须使用一次性 MySQL 8、Redis 7、MinIO、测试 bucket 和迁移后的 schema，test harness 只接受数据库名含 `test` 的 `TEST_DATABASE_URL`，不得读取仓库 `.env`。质量基线合并、T001 重基并取得远端 CI 证据前，不得将 T001 记为 Green 或解除 T017D 门禁。
