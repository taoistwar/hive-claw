# Implementation Plan: HiveGUI 独立本地 Agent

**Feature ID**: `011-hivegui-standalone-mode` | **Working Branch**: `260518-fix-spec-consistency` | **Date**: 2026-08-28 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `specs/011-hivegui-standalone-mode/spec.md`

## Summary

把现有 HiveGUI 管理面补齐为完整、独立的本地 Agent：直接使用本地 LLM 配置，执行 Tool、Skill、Function、Workflow 和 Extism Plugin，持久化加密会话与执行状态，并提供版本化迁移、认证加密备份、结构化诊断、全链路取消和无障碍操作。HiveGUI 不依赖、不请求 HiveWeb；两端只通过 `agent`、`providers`、`hive-builtins` 和新的存储无关 `hive-runtime-core` 复用代码与 ABI 契约。

实施以现有 SQLite CRUD、13 个 AI 管理 Tab（含会话）、DAG 编辑器、Function/Plugin 测试运行时为增量基础。先抽取共享协议和纯执行算法，再补齐 HiveGUI 本地 adapter、会话 UI、迁移与备份；不重写已经存在且符合规格的管理界面。

## Technical Context

**Language/Version**: Rust 1.97.1（2026-07-22 确认的最新稳定版；必须由 `rust-toolchain.toml` 精确固定，workspace `rust-version = "1.97.1"`），edition 2024；工具链固定是任何功能实现前的独立前置义务
**Primary Dependencies**: gpui/gpui-component、Tokio、SQLx `=0.9.0`（workspace 无共享 feature；HiveGUI 精确启用 `chrono|macros|runtime-tokio|sqlite`，HiveWeb 精确启用 `chrono|json|macros|mysql|runtime-tokio|rust_decimal|tls-rustls-ring-webpki`；两者的 `macros` 均已包含 derive，不重复声明 `derive`，HiveWeb 不启用 `mysql-rsa`）、HiveGUI 外部数据源用 `mysql_async =0.37.0` 的 `minimal|native-tls-tls`、`unicode-normalization = { version = "=0.1.25", default-features=false, features=["std"] }`（`UNICODE_VERSION=(17,0,0)`，只负责 Unicode 17 NFC；`NFKC_CF` 映射由官方 Unicode 17 数据生成）、`agent`、`providers`、`hive-builtins`、Extism `=1.30.0`（默认 feature 全关）与 Wasmtime `=46.0.3` `anyhow` 兼容 feature、wasmparser、reqwest、serde/serde_json、chacha20poly1305、sha2、tracing/tracing-appender；新增 `tokio-util` cancellation、`age` passphrase encryption、`tar` archive；Linux GUI 图通过 `zbus_xml =5.2.1` 与来自 `https://github.com/smithay/wayland-rs`、带来源证明的本地 `wayland-scanner =0.31.10`/`quick-xml =0.41` 最小兼容补丁；HiveWeb 使用 `aws-sdk-s3 =1.141.0` 且仅开启 `sigv4a|http-1x|default-https-client|rt-tokio`
**Storage**: 本地 SQLite（业务期 WAL；文件快照/切换前冻结写入、checkpoint、关闭连接，并分流可安全清理与 hot/归属未知/仍含可恢复状态的 sidecar，后者在 canonical 名称按字节保留且 fail-closed；安全残留只经同目录 quarantine + SQLite 外部耐久 cleanup journal 清理）；HiveGUI 数据目录中的托管 Plugin WASM、以 `.open` 活动段写入并原子切换的滚动 JSONL 日志、用户选择的认证加密备份文件；所有托管路径通过已打开根目录句柄逐段 no-follow 解析
**Testing**: `cargo test` unit/contract/integration、由完整可写字段目录驱动的公开 Store/运行时/导入边界验证与重复 identifier/引用或状态冲突、`invalid_input { field, reason }`/安全值限定的 `conflict { field, value }`/引用与状态用 `conflict { field, reason, references }` 及 secret 脱敏、关系表负面 scope contract、设备密钥生命周期、GPUI `VisualTestContext`、真实 Extism WASM fixture、mock LLM、故障注入、既有/新建/迁移 SQLite 的 `integrity_check` 与 `foreign_key_check` 双检查、symlink/junction/reparse/hardlink 与路径替换 TOCTOU、文件和父目录 fsync/原子切换/崩溃恢复、日志完整行与 `.open` 段恢复、CI 中固定 `sqlx-cli =0.9.0` 并执行 `DATABASE_URL='sqlite::memory:' SQLX_OFFLINE=true cargo +1.97.1 sqlx prepare --workspace --check --no-dotenv`、依赖 manifest/lockfile/vendored provenance 的 Red contract 与禁止 7 个 advisory 包的 lock contract、HiveWeb MySQL `VERIFY_IDENTITY`/CA/hostname fail-closed 公共边界测试、T017B 已存在且获批的 `game_service` 恶意 `category_name` 绑定回归、named-query reviewed-config fail-closed 验证、乐观锁封闭表枚举与 SQLx 0.9 唯一 `AssertSqlSafe` 所有权审计、生产 `QueryBuilder`/运行时 SQL 零调用审计、FTS5 trigram 与 1–2 字符 short-gram 的字面量包含搜索、每个生产过滤/关联查询的 SQLite `EXPLAIN QUERY PLAN` 与查询计数/N+1 失败门禁（FTS `VIRTUAL TABLE INDEX` 视为索引访问）、Foundation 可运行的纯 MySQL `EXPLAIN` 输出解析 harness；US2 再由独立 HiveGUI CI job 的 MySQL 8.0+ service 和专用 `HIVEGUI_TEST_MYSQL_URL` 直接调用连接边界，使用隔离 schema/account，且不依赖 HiveWeb migration/server/client/运行状态；MySQL metadata allowlist + 单一 `MysqlIdentifier` source contract 和真实 MySQL `EXPLAIN` 预期索引断言闭合外部数据源边界；固定版本 secret/dependency vulnerability scan source contract、带版本化基线和环境指纹的 `cargo bench` 固定样本百分位及 >10% 回归阻断
**Target Platform**: Linux、macOS、Windows 桌面端
**Project Type**: 单一桌面应用 + workspace 共享 Rust libraries；无新增服务或 Web client
**Performance Goals**: 固定验收数据集 CRUD p95≤1s；搜索/分页 p95≤500ms；Category 100+ 节点加载至可见 p95≤200ms；Agent 本地编排 p95≤200ms；Tool 分派 p95≤50ms；100 节点 no-op DAG 调度 p95≤100ms；输入反馈 p95≤100ms；主线程连续阻塞≤250ms；所有跟踪 benchmark 保存版本化基线/环境指纹并比较 p50/p95/p99，任一项回归 >10% 时无明确签字、记录理由、影响范围和到期复核日期即阻断
**Constraints**: 不请求 HiveWeb；Rust 1.97.1 精确固定且 CI/本地使用同一 toolchain；规划期识别的 7 个 advisory 必须零例外清除，当前已清零且 `deny.toml` 不得新增 ignore；HiveWeb MySQL 必须显式 CA + hostname 的 `VERIFY_IDENTITY`，TLS 缺失、失败或任何宽松模式均 fail-closed，且不得启用 SQLx `mysql-rsa`；应用 schema 固定查询只用 SQLx checked macros/offline metadata，有限变体使用封闭 enum/match，生产代码禁止 `QueryBuilder` 和运行时 SQL，唯一例外是 HiveWeb `named_queries.toml` 的 reviewed-config `AssertSqlSafe` 边界；默认无 WASI；Plugin 默认/最大 30s/120s、128/512MiB、10/50MiB；所有 Plugin/归档路径以受控根目录句柄逐段 no-follow 解析并拒绝 symlink/hardlink/junction/reparse/device/FIFO/socket 与 TOCTOU，新导入 no-replace、更新使用唯一不可变对象名并以旧键+`row_revision` CAS 切换，制品操作日志与 GC ledger 必须耐久重放且身份不明对象只能 blocked 保留；Agent 深度≤10；唯一默认根 Agent且公开会话入口不可覆盖；Category 整树一次批量加载；会话默认保留100年且静态加密；设备密钥原子生成并使用 owner-only 权限，已有密文时不得静默替换异常密钥；新建/既有/迁移 SQLite 均须通过完整性与外键双检查；搜索 normalization ID 固定为 `hivegui-nfkc-casefold-v1`，逐标量应用官方 Unicode 17 `NFKC_CF` 后以 Unicode 17 NFC 收口，变更必须显式迁移重建；数据库文件快照/切换前必须冻结写入、非 busy WAL checkpoint、关闭连接并区分可安全清理与 hot/未知 WAL/SHM/journal sidecar；备份恢复在认证流结尾和全部预检成功前只写目标密钥加密的隔离 staging，用户确认后在同一冻结周期先完成 checkpoint/关闭/sidecar 验证，再从封闭 current 生成并验证安全备份；owner `prepared|applying` 时失败恢复并验证 old，只有完整 new current 已通过 health/search/artifact/identity 验证后才可发布唯一 commit point `committed`，之后只通过 registry retirement 收口已验证的 new；日志逐记录精确保留7×24小时或总计100,000,000 bytes；迁移/损坏恢复不可产生混合状态
**Durable sidecar cleanup**: 合法 `storage_recovery_blocked` 配对与总优先级由 local-runtime contract 唯一定义。已证明安全的残留使用 UTF-8 db_id、uint32 big-endian 长度和 domain separator 计算 64 位小写 SHA-256 token，三个 artifact 的 final/`.staging` basename 固定且 `schema_version=1`；孤立 staging 只在 canonical 未修改可证明时受控删除，final+staging、损坏/不匹配/未知版本 fail-closed。journal 位于数据库同目录、SQLite 外部且不进入备份；sidecar 以 identity-bound no-replace 移至唯一 quarantine，按 `prepared→quarantined→done` 逐步 fsync 并执行包含 `done` 收尾的五分支重放。journal 删除前不得快照、进入恢复 `applying` 或开放 Store。hot/unknown/recoverable sidecar 始终保持 canonical 名称和字节；安全残留中断后只保证完整字节位于 canonical/quarantine，或在 `quarantined` 后已删除并等待父目录耐久确认。
**Staging database discovery**: current 固定为数据根句柄下 `datasources.db` 且 `db_id=current`；migration/restore live instance 固定为 `.hivegui-db-staging-v1/{role}-{UUID}/datasources.db`，建库前发布含 `ownership_state=unarmed` 的六元组 v1 manifest，切换前原子推进 armed。owner final 固定为 `.hivegui-db-recovery-v1.json`，owner staging 固定为 `.hivegui-db-recovery-v1.json.staging`；phase 只允许 `prepared|applying|committed`，且完整新 current 验证成功后才可 committed。只有 unarmed+owner final/staging 均无可派生 aborted；armed+owner 缺失/损坏必须 fail-closed。aborted、terminal old/new 都先创建 registry-level retirement journal，再把整个 live instance identity-bound rename 到 outcome tombstone，只有 tombstone 内逐叶清理。启动先重放 retirement/tombstone，再验证 live manifest/owner、cleanup 和 owner；所有 locator/control state 都不进入 archive、安全备份或新树。
**Scale/Scope**: 13 个 P1 用户故事、51 个功能需求、35 个成功标准；3 个顶层路由、13 个 AI 管理 Tab；20条/页；至少100节点 DAG、100+分类；本地单用户多并发会话

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-checked after Phase 1 design.*

### Pre-design gate

| Gate | Result | Evidence / obligation |
| --- | --- | --- |
| I. Code Quality | PASS | 共享 public API 在 contracts 中固化；新增 Store/runtime 责任进入独立模块，T144 审计 changed-public-API 文档、函数复杂度、无归属 TODO 和死代码；fmt/clippy/docs 进入验收门。 |
| II. Test-First | PASS WITH MANDATORY SEQUENCE | 每个实现批次必须先提交并由用户/指定 reviewer 审阅红灯测试，确认失败后才能修改生产代码；contract/integration/visual/perf、公开边界字段验证与设备密钥生命周期均有对应 fixture。 |
| III. UX Consistency | PASS | 复用现有 Home/Ai/Tools 与管理样式；稳定错误类别映射中文文案；键盘、焦点、AccessKit 和对比度由 UI contract 固化。 |
| IV. Performance | PASS WITH BLOCKING EVIDENCE | 规格把外部 LLM/网络/用户代码耗时与本地预算分离；固定 no-op/mock 基准报告 p50/p95/p99 并绑定版本化基线/环境指纹，任一 >10% 回归无明确签字、理由、影响范围和到期复核日期即阻断；任意位置包含搜索以 FTS5 trigram（3+ 字符）和事务同步 short-gram（1–2 字符）保持全词长索引路径；每个生产过滤/关联查询都解析 EXPLAIN 并断言预期索引，FTS `VIRTUAL TABLE INDEX` 按索引访问判定，层级/关系加载用查询计数测试在发现 N+1 时失败。 |
| V. Simplicity/YAGNI | PASS | 新增一个非部署型 shared crate，满足 HiveWeb+HiveGUI 第二调用方条件；不新增服务、不引入第二 UI/async/runtime/workflow framework。 |
| VI. Observability | PASS | execution_id 贯穿 runtime；v1 JSONL 字段固定，`cause_summary` 中央脱敏且≤512 UTF-8 bytes，活动段只写完整换行记录，轮转执行 `.open` flush/fsync、原子 rename 和父目录 fsync；持久化 high-watermark、按最早记录强制轮转与崩溃安全逐记录压缩保证每条记录实际不超过7×24小时且总量不超过100,000,000 bytes；启动只丢弃末尾不完整记录，错误只在处理边界记录一次。 |
| Security | PASS WITH MANDATORY SEPARATE SIGN-OFF (T025R) | 默认 deny、WASI off、Capability gate、受控根目录句柄逐段 no-follow、SHA/大小验证、symlink/hardlink/junction/reparse 与 TOCTOU 拒绝、公开边界输入校验、设备密钥原子创建与 owner-only 权限、age 认证流隔离恢复、文件/目录 fsync 与可恢复切换日志；当前 7 个 advisory 不允许例外，T017A-T017C 必须先固定并审批依赖/lock/vendor、HiveWeb MySQL `VERIFY_IDENTITY` 与 SQL 安全 Red，补充的查询策略 Red/审批也必须在相关实现前闭合，T017D-T017E 再闭合生产与全量扫描；CI 必须以记录的精确版本运行阻断式 secret scanner 与 `cargo deny check advisories`，工具安装失败、发现任一 advisory 或扫描步骤缺失都必须阻断合并；**密码学/依赖/认证变更合并前必须由独立 security review + 第二人签字（每条边界 1 份签字），统一记入 `checklists/security.md` 6 节：①FR-012 设备密钥、②sidecar cleanup、③Plugin sandbox、④FR-026 备份 age 加密、⑤FR-049/050/051 主密码认证 + 自动锁定、⑥HiveGUI 远程 MySQL 公开边界**。按 Constitution v1.5.0 §Security Requirements *Single-developer repository clause*（2026-07-30 增补）批准，本仓库当前仅 1 名 active maintainer，由该 maintainer 同时承担 *dedicated security review* 与 *second approver* 角色，但 /security-review 流程必须完整运行并把条件总结、证据链接、self-attestation 写入 PR 描述与 `checklists/security.md` 对应行；如未来新增 maintainer，"独立 security reviewer + 第二 maintainer 双签字" 立即恢复。FR-012 设备密钥、FR-049 主密码认证、FR-026 备份加密口令三者必须分别独立 security review，不允许一份签字覆盖另一份。 |
| Technology Stack | PASS WITH HIVEGUI PROFILE + ONE ENUMERATED SQLX DEVIATION + BLOCKING TOOLCHAIN PIN | Constitution v1.5.0 的 HiveGUI desktop-local profile 正式授权 SQLite、加密 SQLite 会话/有界进程内缓存和托管本地文件；HiveWeb 继续 MySQL/Redis/Rustfs。应用 schema 固定 SQL 使用 SQLx checked macros，外部 MySQL 使用受维护的 `mysql_async` prepared API；唯一运行时 SQLx 偏离是 HiveWeb `named_queries.toml` 的 reviewed-config `AssertSqlSafe` 边界并在 Complexity Tracking 登记。Rust/gpui/Tokio/SQLx 符合；T001 必须在任何产品实现前把 toolchain 精确固定为 2026-07-22 的最新稳定版 Rust 1.97.1。Extism 的既有选择及维护影响见 Complexity Tracking。 |

**Pre-design Gate Result**: PASS FOR PLANNING / IMPLEMENTATION RESUMPTION BLOCKED。当前已批准的 Constitution v1.5.0 已消除 HiveGUI desktop-local profile 与单维护者审批结构的规划冲突；SQLite、加密本地会话/有界缓存和托管文件均为合规选择。规划可以继续，但T119 Red 与 T123A reviewer approval 已闭合；实现恢复当前进入 T130，后续仍只能按 T119 → T123A（新 reviewer/Red 门）→ T130 → T136/T138/T142 → T147 推进；任何安全、TDD、reviewer 或 Green 证据仍按对应任务阻断。

### Post-design re-check

- `hive-runtime-core` 只含 ABI、取消、Plugin 和 DAG 的纯 runtime，不引入数据库或 transport，未扩大部署单元。
- HiveGUI 与 HiveWeb adapter 分离，依赖图和 `hiveweb_independence` 测试共同防止运行时耦合。
- Capability 元数据与实际本地 handler 明确分离；共享 `CapabilityRegistry` 和 `PersistedTool` 在产品实现前由独立公开契约 Red 验证，且不得引入产品数据库或 transport。
- 数据模型为所有敏感字段、完整公开可写字段目录、设备密钥生命周期、外键、默认 Agent、迁移窗口和终态定义强制点；契约覆盖取消、备份、迁移、Plugin、LLM 和无障碍。普通非法输入统一返回不含原值的 `invalid_input { field, reason }`；唯一性冲突使用安全值限定的 `conflict { field, value }`；引用或状态冲突使用不含 value 的 `conflict { field, reason, references }`，references 仅含安全实体标识；密码、token、消息正文及其他机密值不得回显。
- 备份选择成熟的 passphrase streaming encryption，而不是自造 AEAD 分块协议；分块认证后的敏感值只在有界内存中转换并立即写入目标密钥加密的隔离 staging，认证流结尾和全部预检成功前不得应用现有状态；仍要求 dependency/CVE/license 与密码学安全审查。
- 托管 Plugin、归档和恢复路径从已打开的受控根目录句柄逐段 no-follow 解析，拒绝跨平台链接类型和替换竞态；Plugin 预校验失败时用户表和内部 ledger 均零修改，成功后先耐久化含确定性唯一 `staging_name` 的 `prepared`，staging 写入/fsync/身份复核后记录 `staging_identity`/`staged`，再 no-replace 发布不可变唯一对象并记录 `published`。create 的 expected-old 字段始终为空且用户行插入、plugin_id 回填与 `referenced` 同事务；replace 仅在原始在线操作中以旧 tuple+`row_revision` live CAS 切换引用，禁止 last-writer-wins。启动重放的 `staged` final-only 分支必须重新 fsync/复验并耐久推进 `published`；`published` 只接受 staging 不存在、final/new_identity 精确匹配，任何 ownership/identity 歧义都把 operation 耐久推进 `conflict` 并阻止开放 Store。create 无用户行不得重建用户意图，replace 仍指向旧 tuple 时不得重做 CAS，只以“登记 owned new object GC + operation→done”同一事务收敛。`referenced` 是历史提交事实，不再要求当前引用仍指向历史 new tuple；GC 独立使用 `pending|blocked`，只有无引用/租约且身份、link count、大小、哈希全匹配的对象可删除。备份、日志和恢复切换均同步文件与父目录；用户确认后在同一写冻结周期先 checkpoint、关闭连接并验证 sidecar，安全残留以外部 cleanup journal + quarantine 完成后再从封闭 current 生成安全备份；新状态完整验证成功后才发布 owner `committed`，随后以 retirement outcome=new 整目录接管 locator，post-commit 只收口已验证 new。
- Plugin operation 完成与实际 GC 删除解耦：“登记 GC + operation→done”必须同一 SQLite 事务，文件 unlink 不得发生在该事务前。operation 只允许六态，ownership/identity 歧义持久化为 `conflict` 并阻断 Store；blocked 只属于 GC。GC worker 在启动、固定周期和引用/租约释放事件后按 artifact_key 稳定扫描 `pending|blocked`，瞬态引用/租约解除且 identity 仍匹配时重试，identity 重现/不匹配或所有权未知持续 blocked。unlink 与父目录 fsync 后、ledger 事务前崩溃时，重放重新 fsync 并确认目标仍不存在后才删 ledger。
- SQLite current 固定由数据根句柄下 `datasources.db` 且 `db_id=current` 定位；migration/restore live instance 只能位于 `.hivegui-db-staging-v1/{role}-{UUID}/datasources.db`。registry operation group 精确覆盖 live/outcome tombstone/retirement final+staging；live 内含六元组 manifest final+staging、owner final+staging。启动按 retirement→live owner/cleanup 顺序 no-follow 验证；unarmed/no-owner 仅通过 aborted retirement 接管，armed owner 缺失或任一 identity/outcome/fsync 歧义 fail-closed。cleanup UUID 与 instance UUID 分离，普通文件要求 link count=1，目录仅 identity-bound 逐层复核。
- 搜索词保持大小写不敏感的纯文本任意位置包含语义；FTS5 trigram 与 short-gram 索引在同一实体事务维护，`%`、`_`、引号和 FTS 操作符不会改变语义。固定应用 SQL 使用 checked macros，有限变体使用封闭 enum/match；生产 `QueryBuilder` 为零。
- 设计没有未决技术澄清，也没有绕过 strict TDD、版本化性能基线、EXPLAIN 索引断言、SQLx offline CI 或安全复核的实现捷径。

**Post-design Gate Result**: PASS FOR DESIGN / IMPLEMENTATION RESUMPTION BLOCKED。Phase 0/1 工件与当前 Constitution v1.5.0 对齐，无未解释的宪章偏离或 NEEDS CLARIFICATION；当前阻塞点不是规划缺口，而是 T119 Red 与 T123A reviewer approval 已闭合，当前执行 T130，后续必须依次经过 T123A、T130、T136/T138/T142 与 T147，不得直接开始 T130。

## Project Structure

### Documentation (this feature)

```text
specs/011-hivegui-standalone-mode/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── README.md
│   ├── local-runtime.md
│   ├── llm-provider.md
│   ├── plugin-abi.md
│   ├── backup-package.md
│   ├── storage-migration.md
│   └── ui-accessibility.md
└── tasks.md                 # reconciled T001–T147 implementation baseline
```

### Source Code (repository root)

```text
rust-toolchain.toml                    # exact Rust 1.97.1 pin
deny.toml                              # dependency advisory policy
.gitleaks.toml                         # fixed-version secret scan policy
Cargo.toml
third_party/wayland-scanner/             # 0.31.10 + quick-xml 0.41 minimal compatibility backport
├── Cargo.toml
└── PROVENANCE.md                        # upstream/version/license/exact patch/exit condition
third_party/unicode-17.0.0/
└── PROVENANCE.md                        # official UCD URL/hash/generator + generated-table review

.github/workflows/ci.yml               # scans, SQLx offline check, MySQL service
.sqlx/                                 # committed SQLx offline metadata

crates/hive-runtime-core/              # new, shared, no product storage/transport
├── Cargo.toml
├── src/
    ├── lib.rs
    ├── abi.rs
    ├── capability.rs
    ├── execution.rs
    ├── plugin.rs
    ├── wasm.rs
    ├── workflow.rs
    └── persisted_tool.rs
└── tests/
    ├── capability_contract.rs
    └── persisted_tool_contract.rs

crates/agent/src/
├── runner.rs                           # cancellation checkpoints + typed outcome
├── tools/base.rs                       # execution context propagation
└── tools/registry.rs

crates/hive-builtins/src/               # four underscore canonical identifiers; no dotted aliases
crates/providers/src/                   # existing LLMProvider/FallbackProvider

crates/hivegui/
├── Cargo.toml
├── benches/
│   └── local_runtime.rs
├── src/
│   ├── datasource/
│   │   ├── store.rs                    # pool options, lock, startup gate
│   │   ├── migrations.rs               # v2→v3→v4 atomic migrations
│   │   ├── search_normalization.rs      # Unicode 17 NFKC_CF mapping + NFC
│   │   ├── validation.rs               # shared boundary validation + field errors
│   │   ├── key_store.rs                # atomic owner-only device key lifecycle
│   │   ├── entity_store.rs             # existing CRUD, narrowed over time
│   │   ├── conversation_store.rs       # encrypted session/message/execution
│   │   ├── llm_store.rs
│   │   ├── mysql_client.rs             # prepared MySQL access + cancellable connection test
│   │   ├── plugin_artifacts.rs         # root-handle no-follow import/hash/atomic replace
│   │   ├── backup.rs                   # encrypted export/staged durable restore
│   │   └── crypto.rs
│   ├── runtime/
│   │   ├── local_agent.rs              # AgentRunner orchestration + direct-child routing
│   │   ├── agent_content.rs            # immutable SQLite execution snapshot
│   │   ├── provider_resolver.rs        # local Preset -> providers chain
│   │   ├── tool_adapter.rs             # DB Tool/Function/Workflow adapters
│   │   ├── workflow_executor.rs        # SQLite NodeExecutor adapter
│   │   ├── plugin_executor.rs          # local artifact + shared Extism runtime
│   │   ├── desktop_host.rs             # only real local Capability handlers
│   │   ├── execution.rs                # lifecycle, event bridge, persistence
│   │   └── diagnostics.rs              # redaction, durable retention, export
│   ├── logging.rs                      # .open JSONL segment + fsync/atomic rotation
│   └── ui/
│       ├── app.rs                      # existing Home/Ai/Tools routes
│       ├── ai_view.rs                  # existing 13 tabs
│       ├── conversation_view.rs        # local Agent conversation + stop/history
│       ├── migration_recovery_view.rs
│       ├── key_recovery_view.rs         # blocking missing/corrupt/unsafe key recovery
│       ├── settings_view.rs            # retention, backup/restore, diagnostics
│       ├── dag_editor_view.rs
│       └── management_style.rs         # keyboard/focus/accessibility primitives
└── tests/
    ├── ci_security_contract.rs
    ├── support/
    │   └── performance.rs              # shared baseline/environment comparator
    ├── storage_query_plans.rs
    ├── query_count.rs
    ├── sql_safety_contract.rs
    ├── relationship_scope_contract.rs
    ├── store_resilience.rs
    ├── logging_contract.rs
    ├── diagnostics_exactly_once.rs
    ├── entity_validation.rs
    ├── device_key_lifecycle.rs
    ├── datasource_connection.rs        # direct MySQL 8.0+ service-boundary integration
    ├── hiveweb_independence.rs
    ├── local_agent_runtime.rs
    ├── workflow_execution.rs
    ├── plugin_compatibility.rs
    ├── backup_restore.rs
    ├── migration_compatibility.rs
    ├── conversation_retention.rs
    ├── cancellation.rs
    ├── accessibility.rs
    └── fixtures/
        ├── migrations/
        ├── backups/
        └── plugins/

crates/hiveweb/src/runtime/              # adapters adopt shared ABI/core incrementally
crates/hiveweb/tests/
├── contract_mysql_tls_policy.rs          # VERIFY_IDENTITY + CA/hostname fail-closed contract
└── contract_sqlx_09_sql_safety.rs        # SQLx 0.9 dynamic SQL/identifier audit contract
plugins/smoke-plugin/                    # shared compatibility fixture
```

**Structure Decision**: 保持两个既有 deployable（HiveWeb、HiveGUI）以及 workspace shared libraries。新增 `hive-runtime-core` 是第二调用方驱动的非部署型库，只抽取两个产品必须一致的协议和纯算法；HiveGUI 所有数据、文件、网络和 UI adapter 均留在 `crates/hivegui`。

## Design Execution

### Phase 0: Research — complete

[research.md](research.md) 已解决：依赖方向、共享 crate 边界、AgentRunner 复用、本地 Provider、Workflow 调度、Plugin ABI/WASI/资源限制、取消、SQLite 迁移、会话加密、备份格式、诊断、GPUI 无障碍和测试策略。

### Phase 1: Data and contracts — complete

- [data-model.md](data-model.md) 定义 v4 SQLite schema、关系、不变量、状态机、迁移与备份序列化。
- [contracts/README.md](contracts/README.md) 索引本地 runtime、LLM、Plugin、备份、迁移与 UI 契约。
- [quickstart.md](quickstart.md) 提供后续实现可执行的红灯/绿灯和端到端验证指南。

### Phase 2: Task generation — complete and reconciled

[tasks.md](tasks.md) 已生成 T001–T147（含编号修复后的 T071A 与新增 reviewer/Red 门 T123A）；一致性分析新增的宪章门禁与测试职责已经同步到任务基线。实施时必须保持以下依赖顺序：

1. 独立质量基线 `codex/ci-quality-baseline-rust-1.97.1` 先修复 `origin/main` 既有 fmt/Clippy/测试问题，删除已移除管理聊天能力的陈旧契约，并让现役 HiveWeb 集成测试在一次性 MySQL 8、Redis 7、MinIO 和迁移后的 CI 环境执行；该前置批次未实施 SQLx 0.9、严格 TLS、安全扫描、SQLx offline 或 HiveGUI 外部 DataSource。随后 `codex/rust-1.97.1-toolchain` 保留基础设施 job、以三文件差异固定 Rust 1.97.1，并已由 PR #4 / `2eee211` 与远端 CI run 30996686002 于 2026-08-05 闭合；T002-T008 的后续重验也已完成。原 `bf3690d` 推送后等待 PR/CI 的文字仅属历史 pre-merge 状态。
2. 在不修改 Cargo/锁文件/生产代码的前提下，先写并观察 T017A-T017B 的依赖/lock/vendor、SQLx 0.9、HiveWeb MySQL `VERIFY_IDENTITY` 与动态 SQL Red contract；T017C 记录证据并取得用户/reviewer 批准。用户于 2026-07-23 又批准 T017D 实施前审计修正：HiveGUI 使用 `macros` 而非冗余 `derive`、Wayland 上游地址统一为小写、`game_service` 值绑定、named-query 唯一 reviewed-config 审计边界及乐观锁封闭枚举。`game_service` 的恶意 `category_name` 测试已经属于 T017B 已批准且观察过 Red 的变更集，T017D 只负责用静态 `JSON_OBJECT('name', ?)` 和 bind 使其 Green，不得在实现任务首次新增或弱化该测试。2026-07-28 推荐 A 的补充查询策略 Red/审批必须把两端应用 schema 固定 SQL 收口到 checked macros（HiveWeb 也由旧 `derive`-only 目标升级为 `macros` 且不重复 `derive`）、有限变体收口到封闭 enum/match，并证明生产 `QueryBuilder` 为零、`AssertSqlSafe` 所有者恰好一个。T001 的 PR/CI 与补充审批门禁完成后 T017D 才能执行零例外 Green 迁移，T017E 必须以全量构建、测试、SQLx offline 与联网 advisory scan 闭合。
3. 共享 ABI/错误/Builtin fixture 的红灯契约测试；在任何 Capability/PersistedTool 或日志核心实现前，补充直接调用共享公开边界的 Capability/Tool 与日志持久性 Red；公开边界测试从同一完整可写字段目录生成合法边界、上下界、格式错误、事务零修改与脱敏错误断言，每行标注 `owner_phase` 与激活/审批任务。T016A-T016F 分别拥有日志持久性、Capability/Persisted Tool 公开边界、SQLite 文件协议、Plugin v4 schema、原生滚动 inventory/harness、敏感字段×落盘介质 canary inventory/harness 的补充 Red；T017F 必须记录 T016A-D/F 的真实 Red，并记录 T016E 纯测试基础设施的合成 fixture self-test Green 与审批；T016E/T016F 的未来故事行仍由各故事 reviewer 激活，Foundation 不得把它们标为 Green。搜索 normalization/FTS/SQL source inventory 另由 T017G→T017H 审批。
4. Foundation 只激活并闭合 `owner_phase=Foundation` 的 Store、runtime、import、SQLite schema/source-contract 以及启动恢复 UI 测试；补充门禁必须分别观察新建/既有/迁移数据库的 `integrity_check` 与 `foreign_key_check` Red、含已提交未 checkpoint 帧的 WAL/SHM/journal 文件快照及 cleanup journal crash-matrix Red、Plugin v4 DDL/约束/回滚 Red、日志固定 v1 字段与 `.open`/完整行/轮转持久性 Red，以及 Capability/PersistedTool 公开契约 Red。原生滚动 inventory 必须为每个 surface 固定 Red→review→implementation→Green 所有者；敏感 canary inventory 必须为每个字段×SQLite main/WAL/SHM/journal、backup staging/final、普通 temp、日志和诊断包固定相同链，实际制造介质后扫描。实际 MySQL metadata allowlist、`MysqlIdentifier` 公开边界和 MySQL source contract 不属于 Foundation，由 US2 T035 编写、T037 审批并在 T038 闭合。T015 由 T026 的本地后台执行边界闭合，T016 的迁移/密钥恢复 GPUI 断言由 T023/T025 闭合，依赖后续故事公开边界/UI 的目录行由各故事 Tests/Reviewer 阶段激活并观察 Red。进入 US1 前必须有明确 Foundation Green 门禁，不得以 ignore 或已知 Red 冒充覆盖。
5. hive-runtime-core + AgentRunner cancellation 的最小实现。
6. SQLite v3/v4 migrations、关系表白名单、Plugin `row_revision`/operation/GC 的唯一 DDL owner、单实例锁/重试、新建/既有/迁移数据库的 `integrity_check` 精确 `ok` 与 `foreign_key_check` 零行门禁、current 固定 `datasources.db`（物理 basename）且 `db_id=current`（逻辑标识） 与 `.hivegui-db-staging-v1/migration-{db_instance_operation_id}/datasources.db` 的建库前 v1 manifest/启动发现/lifecycle，以及文件快照前冻结/checkpoint/关闭/sidecar/fsync 协议（已提交未 checkpoint WAL 完整合并；hot/unknown/recoverable canonical sidecar 原样保留并 fail-closed；安全残留经精确 final/`.staging` 槽位、`schema_version=1`、外部 `prepared→quarantined→done` cleanup journal 五分支耐久清理）、CI 中精确执行 `DATABASE_URL='sqlite::memory:' SQLX_OFFLINE=true cargo +1.97.1 sqlx prepare --workspace --check --no-dotenv`、严格按官方 Unicode 17 `NFKC_CF` 映射+同版 NFC 实现的 `hivegui-nfkc-casefold-v1`、FTS5 trigram + 1–2 字符 short-gram 原子搜索索引/迁移回填/一致性验证、每个生产过滤/关联查询的 `EXPLAIN QUERY PLAN` 预期索引断言、N+1 失败门禁和分离的启动恢复 gate；HiveGUI 通过 SQLx `macros` feature 使用 `query!`/`query_as!`/`query_scalar!` 与提交的 offline metadata，有限条件/表变体只能由封闭 enum/match 选择静态 checked query，生产代码禁止 `QueryBuilder`、运行时 SQL和用户派生 SQL。Foundation 中的 MySQL 覆盖只允许不连接服务的纯 `EXPLAIN` 输出解析 harness，不激活真实 MySQL 公开边界。
   Migration 只在从封闭 current 复制出的 unarmed staging 中执行 SQLite transaction；其 commit 不是系统 commit。staging 全验证后 armed→owner prepared→applying 发布，新 current 再验证后才 owner committed；prepared/applying 回 old、committed 保 new，三种终态都通过 retirement journal + 整目录 outcome tombstone 收口。
7. US2 T035/T037 激活 HiveGUI 外部 MySQL 公开边界 Red 契约，T038 在独立 HiveGUI CI job 的 MySQL 8.0+ service 中通过专用 `HIVEGUI_TEST_MYSQL_URL` 动态创建隔离测试 schema/账号并闭合它们；可复用 MySQL 镜像配置，但不得读取仓库 `.env`、复用 HiveWeb `TEST_DATABASE_URL`/migration/server/client/运行状态。测试必须直接调用真实连接函数并覆盖成功、错误凭据、不可达、取消、超时和错误脱敏；MySQL 值必须使用 prepared statement/参数绑定，动态数据库/表/列名必须先精确匹配服务器 metadata allowlist，再只通过单一 `MysqlIdentifier` 类型序列化，普通离线单元测试不得依赖该服务。HiveWeb 自身的应用 schema 乐观锁改用独立封闭表枚举；其启动期 named-query 仅经一个 fail-closed reviewed-config 边界构造 `AssertSqlSafe`，两种边界均不进入 HiveGUI，也不建立 HiveGUI→HiveWeb 调用。
8. Plugin staged import 必须从已打开的受控根目录句柄逐段 no-follow 解析，拒绝 symlink/hardlink/junction/reparse/device/FIFO/socket 和路径替换 TOCTOU，最终对象必须是 link count 1 的普通文件。字段/ABI/manifest/Capability/内容预校验失败时用户表和内部 ledger 均零修改；成功后先持久化含确定性唯一 `staging_name` 的 `prepared`，再创建 staging，写入/fsync/身份复核后记录 `staging_identity`/`staged`，no-replace 发布并 fsync 后记录 `published`；DDL CHECK 精确约束 prepared/staged/published|referenced 的 identity nullability。create 的 expected-old 字段始终为空，只能在插入 Plugin 行、回填 plugin_id 与标记 `referenced` 的同一事务形成用户状态；启动时无精确用户行不得重建创建意图。replace 从 `prepared` 起携带完整旧 tuple/revision，原始在线操作发布新键后以旧 tuple+精确 `row_revision` live CAS 切换引用和递增 revision；重放仍为旧 tuple 时不得执行 CAS，只能以“登记新对象 GC + operation→done”同一事务收敛，实际 GC 独立 pending/blocked。`staged` final-only 要重做父目录 fsync/复验并耐久推进 `published`；`published` 只允许 staging 缺失、final/new_identity 精确匹配。`referenced` 是历史提交事实，不再要求当前行/final 保持该历史 tuple；登记旧对象 GC 与 operation→done 同事务，GC 独立收敛。operation/GC ledger 覆盖每个文件/状态/父目录边界，包括 unlink+父目录 fsync 后但 ledger 事务前崩溃、缺失目标重验和 identity 重现；失败可保留内部 ledger但不得产生部分用户状态，身份不明对象不得删除；运行时只接收仍打开且已验证的文件句柄。随后闭合 WASI off、资源限制、实例取消、兼容性以及有界实例池的完整 key/容量/失效矩阵。
9. Workflow 原子图保存、纯 DAG 调度和 fail-fast。
10. 本地 Provider、Agent snapshot、Tool/Skill/Capability 与直接子路由。
11. 加密会话、100年默认保留和运行事件 UI。
12. 认证加密备份、跨设备恢复和故障注入原子切换；导出使用同目录 staging→文件 fsync→no-replace rename→父目录 fsync。恢复建库前发布 unarmed 六元组 manifest；预验证不 arm、不发布 owner。确认后冻结写入并完成 current/staging checkpoint、关闭、sidecar 分流和安全备份，再把 manifest arm、发布 owner prepared/applying。新数据库/Plugin 树发布并完整重开验证成功后才 committed；prepared/applying 回 old，committed 保持已验证 new。aborted/old/new 都经 registry retirement journal + 整 live instance outcome tombstone 收口，owner/manifest 只在 tombstone 内逐叶删除。registry/live/tombstone/manifest/owner/retirement/cleanup control state 不进入 archive、安全备份或新树；全部 retirement 耐久完成后才开放写闸门。
13. 依赖顺序固定为 US7→US8、US8→US9、US4+US9→US10、US10→US11，US13 再汇合 US4/US7-US12；不得因并行标记绕过引用目标或执行器依赖。
14. CI 以精确版本运行阻断式 secret scan 与 dependency vulnerability scan；同时完成可注入时钟、固定 v1 字段、`cause_summary`≤512 UTF-8 bytes、错误脱敏后单次记录、`.open` 日志完整行与持久化轮转/high-watermark/逐记录压缩、每条记录精确 7×24 小时且总量≤100,000,000 bytes 的保留/诊断、键盘/AccessKit/对比度和全部 CRUD/字面量包含搜索/树性能证据；完整链路诊断 E2E 只复核 Foundation 已定义并由日志实现闭合的核心行为，不得首次定义保留或脱敏；复跑版本化 benchmark，并阻断没有明确签字、理由、影响范围和到期复核日期的 >10% 回归。

### Phase 3: Local Master-Password Authentication (Session 2026-07-29 新增)

- **架构边界**：在 `crates/hivegui/src/auth/` 下新增 `keystore.rs`（AuthKeystore 文件 IO + Argon2id 派生 + ChaCha20Poly1305 包装/解包）、`lock.rs`（AuthLockState 内存态 + 空闲计时 + 屏幕锁事件 + 内存清零）、`policy.rs`（主密码强度规则、5 次错误 backoff、SQL 注入式扫除）、`ui/{setup,unlock,recovery_confirm}_view.rs`（GPUI 视图），与 `crates/hivegui/src/ui/app.rs`、`crates/hivegui/src/main.rs`、`crates/hivegui/src/runtime/mod.rs` 集成。
- **数据模型**：新增实体 `AuthKeystore`（`.hivegui/keystore/wrapped_device_key.v1`、0600）和 `AuthLockState`（仅内存）；不修改 `datasources.db` 已有 schema。`GlobalConfig` 新增 `auth.auto_lock_minutes`（默认 15，范围 `1..=1440`，0 视为非法）。
- **关键依赖**：新增 `argon2 =0.5`（复用 `chacha20poly1305 =0.10` 与现有设备密钥通道）；`zeroize =1.8` 用于内存清零；屏幕锁事件：`windows =0.58`（仅 `Win32_System_Power`，注册 `WM_WTSSESSION_CHANGE=0x7`）、`objc2 =0.5`（macOS `NSWorkspaceScreensDidSleep`）/ 复用 `zbus =4` `org.freedesktop.ScreenSaver ActiveChanged`。
- **依赖顺序**: T001 → T017A-T017E → T018（含 secret scan）→ T-AUTH-1~4 Red（Foundation-auth-Red，Constitution II.3）→ T-AUTH-5 实现（doc 硬门槛）→ T025R 全部 6 边界签字（其中 ⑤ = Local master-password authentication，闭合 Phase 1A）= Phase 1A 闭合；Phase 1A 闭合后才可启动 T022 等 Phase 2 Foundation Red。`auth.*` 任务组不得在任何 Foundation Red 通过前启动。**T022A-T022F / T-AUTHR-* 任务 ID 不存在**（已合并入 T025R 6 边界）。
- **新任务**（详见 [tasks.md Phase 1A](file:///home/developer/agent/gpui-claw/hive-claw-worktree/specs/011-hivegui-standalone-mode/tasks.md)）:
  - T-AUTH-1: Red contract — 首次启动必须进入设置界面，弱密码（含与 `getpwuid(getuid()).pw_passwd` 同长同字符集）100% 拒绝；接受密码后仅测试 wrapper 路径（不重复 T025 已 Red 的设备密钥生命周期）；**Argon2id 派生耗时 < 5000ms 超时 fail-closed**。
  - T-AUTH-2: Red contract — 重启显示解锁界面，密码字段无回显；5 次错误后整应用 5 分钟 backoff 且仅显示从备份恢复入口；错误密码不修改/删除/重新生成设备密钥或 `datasources.db`。
  - T-AUTH-3: Red contract — 15 分钟空闲（仅 keypress / 主窗口 mousedown / 焦点变化重置，mousemove 不重置）自动锁定 + 屏幕锁事件（Linux zbus/macOS NSWorkspace/Windows WTS）立即锁定 + `zeroize` 内存清零 + UI 回到解锁界面 + `AgentExecution.status=cancelled` + `ChatSession.status=locked`；事件被禁用/DBus 不可用/通知 API 不可用时 **fail-closed = 整应用立即锁定 + 提示 + 阻断主 UI**；锁定后 SQLite/WAL/SHM/备份 staging/诊断包明文 canary 扫描命中数 0。
  - T-AUTH-4: Red contract — 无密码重置/找回/旁路；首次设置后必须强制 UI 展示风险说明并要求勾选确认；不存在 T129 备份时必须先跳到 T129 备份向导并完成首次导出；恢复后**必须重新生成设备密钥并以新主密码重新包装**（旧 wrapped_device_key 物理字节保留仅用于审计）；`auth::keystore` 不得暴露 `reset_password`/`recover_from_questions`/`recovery_key` API；**fault-injection**：把 T129 备份内 `wrapped_device_key` 替换为旧设备 wrapped blob，断言新设备使用新主密码仍 fail-closed。
  - T-AUTH-5: 实现 `crates/hivegui/src/auth/{keystore,lock,policy,ui}.rs` 并合并到 `app.rs`、`main.rs`、`runtime/mod.rs`；**doc 硬门槛**（每个新增 `pub fn` 必须在生产代码合并前完成 doc comment，启用 `#![warn(missing_docs)]`，`cargo doc --no-deps` 失败即视为任务未闭合）；**本任务不得在 T025R 第 ⑤ 边界签字前合并**。
  - T025R ⑤ 签字: 按 Constitution v1.5.0 §Security Requirements *Single-developer repository clause*（2026-07-30 批准），本仓库仅 1 名 active maintainer，由该 maintainer 在 `checklists/security.md` "Local master-password authentication" 边界完成 `/security-review` 流程 + self-attestation 后闭合；T-AUTH-5 不得在 T025R ⑤ 签字前合并（**T-AUTH-R 任务不存在，已合并入 T025R**）。
- **备份集成**：`.hivegui/keystore/` 必须显式加入 T026/T129 的备份 manifest exclude 列表（与 `.hivegui-db-staging-v1/`、`.hivegui-db-instance-v1.json`、SQLite cleanup journal/quarantine、设备密钥、运行日志、诊断包与切换日志同列）；T129 恢复后必须强制调用 `auth::keystore::require_new_password_setup()`，旧密码不会随备份进入新设备。
- **屏幕锁事件实现**：Linux 通过 `zbus::blocking::Connection` 订阅 `org.freedesktop.ScreenSaver` 的 `ActiveChanged` 信号；macOS 通过 `objc2::ffi` 注册 `NSWorkspaceDidChangeScreenParametersNotification` + `NSWorkspaceScreensDidSleepNotification`；Windows 通过 `RegisterPowerSettingNotification` + `WM_POWERBROADCAST` + `WM_WTSSESSION_CHANGE=0x7`；事件被禁用 / DBus 不可用 / 通知 API 不可用时 T-AUTH-3 必须 fail-closed 而不是静默忽略。
- **风险声明**：本地主密码认证是 Constitution v1.5.0 Security Requirements 下“auth changes MUST receive dedicated security review + second approver” 范畴；按 2026-07-30 增补的 *Single-developer repository clause*，本仓库 1 名 active maintainer 同时承担 dedicated security review + second approver 角色，self-attestation 与 `/security-review` 条件总结须写入 `checklists/security.md` 对应行；FR-012 设备密钥、FR-049 主密码认证、FR-026 备份加密口令三者必须分别独立 security review，不允许一个签字覆盖另一个。

## Complexity Tracking

| Profile use / Deviation | Why Needed | Simpler Alternative Rejected Because | Safeguard / Exit | Maintenance / Review-Expertise Impact |
| --- | --- | --- | --- | --- |
| Constitution v1.5.0 HiveGUI profile: SQLite | 独立桌面 Agent 必须无需服务器、随应用本地持久化并支持原子文件快照。 | 本地启动 MySQL 会增加服务部署并破坏离线/单应用体验；请求 HiveWeb 直接违反规格。 | SQLx checked macros/offline metadata、外键/WAL/busy timeout、单实例锁、版本化事务迁移；新建/既有/迁移数据库均要求 `integrity_check=ok` 且 `foreign_key_check` 零行；HiveWeb 继续 MySQL。 | HiveGUI datasource owner 维护 schema/迁移和损坏/孤儿外键 fixture；升级需 SQLite/SQLx reviewer，HiveWeb MySQL reviewer 无需承担桌面迁移。 |
| Constitution v1.5.0 HiveGUI profile: 加密 SQLite 会话 + 有界进程内 Plugin pool | HiveGUI 不能依赖本地或远程 Redis；静态会话需要持久加密，而 Plugin 热实例只需进程生命周期内复用。 | 嵌入或启动 Redis 会新增 deployable；完全禁用复用会使固定 Plugin 性能目标不可实现。 | 会话通过 `expires_at` 和确认式清理实现 TTL。闲置 Plugin pool 全局最多 8 个实例、每个完整 cache key 最多 1 个实例；完整 key/失效矩阵见 tasks。 | Runtime owner 维护容量、key 和失效矩阵，crypto/data reviewer 审核会话保留；每次 Extism/额度升级必须重跑池隔离测试。 |
| Constitution v1.5.0 HiveGUI profile: 本地托管 WASM、日志与备份文件 | HiveGUI 必须在 HiveWeb/Rustfs 不存在时导入和执行 Plugin，并让用户保存/恢复本地备份。 | 嵌入 Rustfs 或依赖云对象存储会把额外 deployable 带入桌面端。 | 受控根目录句柄逐段 no-follow，拒绝 symlink/hardlink/junction/reparse 与 TOCTOU；Plugin no-replace/不可变对象、`row_revision` CAS、耐久制品操作/GC ledger 与 blocked 保留；owner-only 权限、同目录 staging、文件和父目录 fsync、原子 rename、恢复切换日志、size/SHA-256、认证加密备份；日志使用 `.open` 活动段、high-watermark 与逐记录压缩。 | Desktop storage/security owners 维护跨平台 root-handle/no-follow/fsync/ACL/原子切换与 GC 重放；文件格式、平台或归档库升级要求安全 reviewer。 |
| Unicode 17 `NFKC_CF` 生成表 + SQLite FTS5 trigram/short-gram 搜索 | 产品契约要求 1..=255 字符大小写不敏感的任意位置纯文本包含，并锁定跨升级规范化语义；SQLite trigram 不负责 Unicode case fold。 | lowercase/simple fold 不满足 Unicode R5 且不能移除 default-ignorables；B-tree 前缀会改变包含语义；只用 trigram 无法覆盖 1–2 字符。 | 从官方 Unicode 17 `DerivedNormalizationProps.txt` 生成映射并记录 URL/SHA/命令，精确固定 `unicode-normalization =0.1.25` 做 Unicode 17 NFC；依赖版本、`UNICODE_VERSION`、表校验值和 golden fixture 进入 Red/reviewer 门禁。3+ 标量走 trigram，1–2 走事务同步 short-gram；EXPLAIN 认可 `VIRTUAL TABLE INDEX` 且拒绝扫描 fallback。 | Datasource/Unicode/security reviewer 维护 provenance、生成器、规范化 ID、事务索引路径与 fixture；Unicode、依赖或 tokenizer 升级必须分配新 ID、显式迁移并重跑全量契约。 |
| HiveWeb `named_queries.toml` reviewed-config SQLx 边界 | 已部署的启动期命名查询是受信配置且查询文本不能在编译时枚举，SQLx checked macros 无法表达该单一现有能力。 | 删除命名查询或为每个部署配置重新编译会破坏现有运维契约；允许通用 `QueryBuilder`/raw SQL 会扩大不可审计面。 | 生产应用 schema 其它 SQL 全部使用 checked macros 或封闭 enum/match；唯一中央边界 fail-closed 拒绝多语句、注释、placeholder/参数不匹配、重复参数和 kind 不匹配，生产 `AssertSqlSafe` 所有者恰好一个、`QueryBuilder` 为零。 | HiveWeb data/security reviewer 共同维护唯一边界和配置审查；每次 SQLx 升级运行所有权 inventory、offline prepare 与恶意配置测试，若命名查询被静态替代则删除偏离。 |
| Vendored `wayland-scanner =0.31.10` 最小兼容补丁 | 固定 GPUI/Wayland 图仍依赖该版本，而其 `quick-xml 0.39.4` 命中 advisory；直接上游 git HEAD 在当前图产生 21 个 API 错误。 | advisory ignore 或等待发布违反已批准零例外门禁；升级整套 GPUI/Wayland 扩大回归面。 | `third_party/wayland-scanner/PROVENANCE.md` 精确固定上游 `https://github.com/smithay/wayland-rs`、版本、MIT 和仅两处差异；当固定 GPUI 可使用原生支持 quick-xml 0.41 的发布版时立即删除 `[patch.crates-io]` 与 vendor。 | Desktop/security reviewer 必须逐行审核差异；每次 GPUI/Wayland 升级检查退出条件，禁止演变为行为 fork。 |
| Extism/Wasmtime 未列入 canonical stack | 规格要求与 HiveWeb 共用版本化 Extism ABI，并执行跨语言 WASM Plugin。 | 裸 Wasmtime 要自造 PDK/ABI；native 动态库缺少同等 sandbox 和跨平台性；禁用 Plugin 不满足规格。 | WASI off、Capability default deny、timeout/fuel/memory/output、CancelHandle、依赖扫描、共享 fixture；若共享 ABI 被替代则退出。 | HiveWeb/HiveGUI runtime owners 共同维护 ABI；Extism/Wasmtime 升级要求两端兼容、安全和 sandbox reviewer。 |
| HiveGUI 本地主密码认证（FR-049/FR-050/FR-051） | 桌面应用需要在 HiveWeb/Rustfs 不存在时保护本地敏感数据并实施用户级访问门禁。 | 接受"永远不锁定"或仅 OS 文件权限会失去对共享/被盗设备的保护；旁路重置/找回/安全问题会扩大攻击面。 | Argon2id (m=64MiB, t=3, p=1) 派生 wrapping KEK + ChaCha20Poly1305 包装设备密钥材料；`keystore/` 0600 + 不进入任何备份；`zeroize` 内存清零；屏幕锁事件 fail-closed；5 次错误 backoff 5 分钟；无密码重置（仅可从 T129 备份恢复）。 | Cryptography/security reviewer 维护参数、OWASP 基线、KCV、备份 exclude 完整性；OWASP/RustCrypto/Argon2/AEAD 升级要求同步参数与派生耗时 < 5000ms 时钟验证。 |
| source `9e392445…` 的 `conversation_list_recent_batched_v2` p50 有期性能例外 | source-owned matrix 已证明 p95/p99、绝对预算及其他 12 个目标 Green；唯一相对 p50 回归经只读诊断未发现 N+1 或表扫描产品回归，user 于 2026-08-27 精确批准 baseline=`121,384ns`、observed/max=`136,200ns`、review due=`2026-09-27`。 | 改写 baseline 会抹除版本化比较；重跑碰运气会抹除已观察 Red；无签字放宽全局 10% 门或把绝对预算 Green 当作相对 Green都会扩大未审范围。 | sidecar SHA-256=`ec7f8bdef3bcac3cd45ba22b0c61d2d44753e4c6de504a4ac58626ddce4f6876`，只绑定完整 source `git:c98cfe22b63f87337455850319bec34346fb5beb+hivegui-source-v1:9e39244542c3aa61d5e39a3310054187c085c8a70b295eeaffdcc8826257fb0c`、该 target 与 p50，cap 零余量；p95/p99/绝对预算/其他 target/后续 source 均不豁免。同源 envelope=`source_stable=true/status=passed`，12 direct Passed + 1 ApprovedException。plan/spec 文档不进入 source fingerprint。 | **历史审计项**：后续 source 变化后该 sidecar 已旁移为 `.superseded.json`，旧签字不跨 source 生效；治理合同已修正为 `canonical_release_baselines_match_all_current_targets` 并 Green，当前 source `53e3ae2b…` 的唯一矩阵为 13 direct Passed、零 active exception。原 signer/scope/review_due 仍保留用于审计，不再是现役发布门。 |

**2026-08-27 restore/maintenance 实现补充**：Store-bound Backup/Restore 现在复用 stable root key 的 exact owner-id/kind/phase maintenance lease；confirmation 把共享 Pool 同步置 closed，并把 OS flock 持有到 terminal startup replay 成功。Archive、closed-current 与 safety snapshot 的 Linux 路径使用 held descriptor/Dir binding，恢复错误以 `NotVerified|Verified|Invalidated` typed safety state 传给 Settings，避免根据字符串或路径存在性猜测。但 SQLx checkpoint/staging 尚未绑定同一 leaf fd/VFS，Plugin switch 与部分 rollback/cleanup 仍有 ambient-path TOCTOU，Windows file-id/reparse 与非 Unix no-follow 也未闭合；因此这只是 T119/T130 supplemental，不改变其 Pending 状态。

**2026-08-28 当前源码与宪章门禁补充**：source canonical matrix 的既有证据保持 source_stable=true/status=passed、13 direct Passed、零 active exception，T137/T145 Closed。Constitution v1.5.0 的规划与设计门禁当前为 PASS；实现恢复门禁仍为 BLOCKED AT T130，唯一允许的恢复链为 T119 → T123A（新 reviewer/Red 门）→ T130 → T136/T138/T142 → T147。不得用既有源码 Green、单维护者条款或规划通过状态绕过该任务链。

`age` 和 `tar` 是 backup 实现依赖，不是新 deployable 或替代 canonical framework；加入前仍按 Security Requirements 做维护状态、许可证、CVE 和密码学 review。

## Known Baseline and Verification Notes

规划期只读验证已确认以下现有测试为绿色：

```bash
cargo test -p hivegui --test desktop_host_call
cargo test -p hivegui --test runtime_capability_catalog
cargo test -p hivegui --test wasm_exports_test
cargo test -p hivegui --lib ui::dag_editor_view::tests
```

T005 必须为每个 benchmark 保存版本化基线文件、固定 fixture 版本、硬件/OS/toolchain/build-profile 环境指纹和采样规则。已有可运行路径在任何性能相关实现前记录当前基线；全新 benchmark 以首个经 reviewer 批准的 Green 结果建立基线。后续 T093/T103/T122/T137/T145/T147 比较 p50/p95/p99，任一 >10% 回归在没有明确签字、理由、影响范围和复核日期时阻断。

**历史状态（已修复）**：早期 `integration_test::test_schema_version_management` 曾仍断言版本 1、而实现已是 2；该基线失败已在迁移批次中修正，当前测试对首次迁移及幂等重跑均断言版本 2。`HIVEGUI_HEADLESS=1` 在 Store 初始化前退出，仍只能证明二进制入口，不能作为本地 Agent 或迁移验收。

**历史前置状态（已由当前账本闭合取代）**：T001 当时要求在独立前置批次把 `rust-toolchain.toml` 精确固定为 Rust 1.97.1，并验证 workspace `rust-version = "1.97.1"`、本地和 CI 的 `rustc --version` 一致；1.97.1 是 2026-07-22 查询 Rust 官方发布记录时的最新稳定版。当时独立分支 `codex/rust-1.97.1-toolchain` 的提交 `bf3690d` 虽已推送，但自动化凭据没有 PR API 权限，尚无 PR/远端 CI 证据，故彼时不得用“分支已推送”冒充闭合。当前 T001 已按 `tasks.md` 的现役状态完成，最终 source 亦重新核对 Rust/Cargo 1.97.1 与 workspace MSRV 一致；本段不再表示现役阻断。

原有 `.github/workflows/ci.yml` 只有 fmt、clippy 和一个无法在无基础设施 runner 上完成的 workspace test 命令，也没有宪章要求的 secret scan、dependency vulnerability scan 或 SQLx offline metadata check。用户于 2026-07-23 批准先在独立质量基线中把现役 HiveWeb contract/integration suite 分到强制 job，并启动一次性 MySQL 8、Redis 7、MinIO、创建测试 bucket、运行迁移；测试 harness 只接受名称含 `test` 的 `TEST_DATABASE_URL`，不得读取仓库 `.env` 或连接共享/生产数据库。此修复只是让当前测试可执行，不替代后续安全任务。T017A-T017B 必须先观察依赖/lock/vendor、SQLx 0.9、HiveWeb `VERIFY_IDENTITY` 和动态 SQL 的可识别 Red，T017C 获批后 T017D 才可修改 Cargo/锁文件/生产代码；T017E 必须用 `cargo check --workspace --all-targets --locked`、固定 `sqlx-cli =0.9.0`、联网 `cargo deny check advisories` 和受影响测试闭合，且不允许 advisory 例外。T018 再加入 security checklist 中记录的精确版本 secret scanner、`cargo-deny` 与 SQLx offline 阻断步骤；任一工具安装失败、任何 advisory、扫描步骤缺失、SQLx metadata 缺失/陈旧或命令失败都必须阻断合并。CI 必须精确运行 `DATABASE_URL='sqlite::memory:' SQLX_OFFLINE=true cargo +1.97.1 sqlx prepare --workspace --check --no-dotenv`；HiveGUI 业务数据源公开边界仍只在 US2 T035/T037 激活、T038 实现，届时由独立 HiveGUI job 和专用 `HIVEGUI_TEST_MYSQL_URL` 动态创建隔离 schema/account 并直接调用产品连接函数，不得读取仓库 `.env`、复用 HiveWeb migration/server/client/运行状态，或硬编码业务 ID、现有数据库、真实凭据。

工作树已有用户的 HiveGUI 源码修改；实现和提交必须逐批只暂存已验证范围，不覆盖或格式化无关文件。

本仓库没有 spec-kit 通常提供的 `.specify/scripts/bash/update-agent-context.sh`；规划期已尝试执行并得到“文件不存在”。因此本命令没有自动改写 `AGENTS.md`，后续应在补回官方脚本后再执行 context update，不能用临时脚本猜测其行为。
