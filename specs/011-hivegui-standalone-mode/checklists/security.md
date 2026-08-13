# Security Review Checklist: HiveGUI 独立本地 Agent

**Purpose**: Feature 011 的依赖、备份加密、归档解析、Plugin sandbox 与 CI 安全门禁
**Created**: 2026-06-15
**Current review**: 2026-08-11（仅文档复核更新；T138 汇总尚未运行）
**Feature**: [spec.md](../spec.md)

## T006 与补充 T017G 当前候选依赖评估（方案目标已确认，实施审批仍 Pending）

**工具链基线**: Rust 1.97.1。版本信息来自 2026-07-22 的 registry metadata 和上游发布记录；实际合并仍由固定版本 `cargo deny check advisories` 重新扫描提交后的完整 `Cargo.lock`。

**方案确认状态**: `Approved for T007`。用户/feature owner 于 2026-07-22 先选择推荐选项 A，批准下表精确版本、最小 feature、Extism 安全升级和 GPUI revision 固定；在发现 Extism 1.30.0 缺失 Wasmtime `anyhow` feature 后，又确认下方补充选项 A1。该确认仅授权本地 T007 实现；宪章要求的专门 security review 和第二批准仍是合并前门禁，由 T138 及最终 PR 审批闭合，不伪装成已经取得的 reviewer 签字。

**Advisory 修复确认状态**: `Historical Red approved; supplemental T017G/T017H pending; Green pending T001 PR/CI`。用户/feature owner 于 2026-07-23 选择推荐方案 A，批准下方 7 个 advisory 的无例外修复目标，随后批准 T017A-T017B Red 证据，并再次批准实施前审计提出的 SQLx feature、Wayland provenance、`game_service` 绑定、named-query 中央审计边界和乐观锁封闭枚举修正。2026-07-28 用户又批准 checked-static-query 推荐 A，使 HiveWeb 最终 SQLx feature 由历史 `derive`-only 目标升级为 `macros`、生产 `QueryBuilder` 为零；同日“全 A”批准 Unicode 17 `NFKC_CF`+NFC 的规范目标与候选依赖方向。上述目标必须先由 T017G 编写、实际观察 Red，并由 T017H 的 Unicode/data、dependency/security 与 SQL reviewer 审批；方案批准不代表 T017H、Green、专门 security review 或第二批准已经完成。

### 依赖决策表

| 依赖 | 精确候选与最小 feature | 维护、许可证与 MSRV | 已知 advisory / 处置 | 放行条件 |
| --- | --- | --- | --- | --- |
| `tokio-util` | `=0.7.18`，`default-features=false`，仅 `rt`（`CancellationToken`） | Tokio 项目持续维护；MIT；声明 Rust 1.71 | 2026-07-22 未检索到 `tokio-util` 包 advisory；完整锁文件仍必须由 cargo-deny 阻断扫描 | 推荐批准 |
| `age` | `=0.12.1`，`default-features=false`；只用 library passphrase streaming API，不启用 `plugin`、SSH、CLI/pinentry | rage/age 上游 2026 年发布；MIT OR Apache-2.0；声明 Rust 1.74 | RUSTSEC-2024-0433 的修复范围包含 `>=0.11.1`；同时禁用触发外部 age plugin 执行的 feature | 推荐批准 |
| `tar` | `=0.4.46`，`default-features=false`（不启用 xattr） | composefs/tar-rs 仍维护并发布安全修复；MIT OR Apache-2.0；声明 Rust 1.63 | 0.4.46 包含最新 PAX desync 修复，并高于 RUSTSEC-2026-0067/0068 的 `>=0.4.45` 修复线；实现仍禁止对不受信包调用 `unpack`/`unpack_in` | 推荐批准，但必须执行下方归档约束 |
| `unicode-normalization` + Unicode 17 生成表 | `unicode-normalization = { version = "=0.1.25", default-features=false, features=["std"] }` 只负责 NFC；`NFKC_CF` 从官方 Unicode 17.0.0 `DerivedNormalizationProps.txt` 确定性生成 | unicode-rs 维护；MIT OR Apache-2.0；声明 Rust 1.36；crate 内置 `UNICODE_VERSION=(17,0,0)` | 当前 lock 已传递解析 0.1.25，但直接依赖、官方 UCD 使用条款、源 SHA-256、生成器/输出 checksum 与数据更新流程仍须 T017G Red、T017H 独立 reviewer 和 T138 最终复核；依赖或数据漂移必须 fail-closed 并分配新 normalization ID | 文档方案 A 已确认；实际直接依赖和生成数据保持 Pending，不得在 T017H 前实施 |
| `extism`（现有依赖升级） | 从 `1.21.0` 改为 `=1.30.0`，`default-features=false`；禁止 Extism 自动 HTTP/filesystem 注册 | Extism 上游最新 1.30.0；BSD-3-Clause；使用 Wasmtime 43 | 当前 1.21.0 带入 Wasmtime 41.0.4，受 RUSTSEC-2026-0114 影响且 41.x 无修复线；1.30.0 升到 Wasmtime 43，当前解析 43.0.2，满足 advisory 的 `>=43.0.2` 修复线 | **阻断式推荐升级**；不得为 41.x 建立无到期例外 |
| `wasmtime`（Extism 兼容 feature） | `=43.0.2`，`default-features=false`；HiveGUI 直接声明只新增 `anyhow` 并用于 Cargo feature-unification，有效图仍包含 Extism/wasi-common 的必要 feature | Bytecode Alliance 持续维护；Apache-2.0 WITH LLVM-exception；声明 Rust 1.91 | 43.0.2 位于 RUSTSEC-2026-0114 修复线；不用 `wasmtime-default-features` 进一步扩大功能面 | A1 已确认；必须验证 Extism HTTP/filesystem/default feature 仍关闭 |
| `gpui-component` / assets（现有 git 依赖固定） | 两项都增加 `rev="49f4b4fb57553daa89310370a97b37f7ff9d2323"`，保持当前锁文件已验证源码 | 上游持续开发；Apache-2.0；Rust 1.97.1 已实际编译 | 当前 manifest 跟随分支且远端 HEAD 已与 lock commit 不同，重建 lock 会产生未审批漂移 | 推荐固定当前 commit，不升级 API |
| benchmark | 不新增 Criterion；T005 使用 `harness=false` 的固定样本 runner 与共享 percentile/baseline 模块 | 减少供应链与 feature 面；只复用 workspace 已有 serde/JSON 和标准库计时 | 无新增依赖 | 推荐批准；若后续改用 Criterion，必须重新走本表审批 |

**T006 审核完成（2026-08-06）**：本轮评估完成 `tokio-util`、`age`、`tar`、`unicode-normalization`、`extism`/`wasmtime`、`gpui-component` 与 `benchmark` 的特性范围与版本审批，并固定 CI 工具版本为 `cargo-deny = 0.20.2`、`cargo-sqlx = sqlx-cli 0.9.0`。当前 `advisory 例外账本` 仅存在 “无例外” 行，无到期日管理项；依赖与 CI 工具的实际执行变更仍等待 T001/T018/T138 与专门 security review 决策，不在此阶段直接改动 `Cargo.toml`/`Cargo.lock`/`deny.toml`。

上游证据：[`tokio-util`/Tokio 维护与 MSRV](https://github.com/tokio-rs/tokio)、[`age` 0.12.1](https://github.com/str4d/rage/releases/tag/v0.12.1)、[age advisory](https://rustsec.org/advisories/RUSTSEC-2024-0433)、[`tar` 0.4.46](https://github.com/composefs/tar-rs/releases/tag/0.4.46)、[tar advisories](https://rustsec.org/packages/tar.html)、[`unicode-normalization` 0.1.25](https://github.com/unicode-rs/unicode-normalization/tree/v0.1.25)、[Unicode 17 Default Case Algorithms R5](https://www.unicode.org/versions/Unicode17.0.0/core-spec/chapter-3/)、[Unicode 17 `DerivedNormalizationProps.txt`](https://www.unicode.org/Public/17.0.0/ucd/DerivedNormalizationProps.txt)、[`extism` 1.30.0 / Wasmtime 43](https://github.com/extism/extism/releases/tag/v1.30.0)、[Wasmtime advisory](https://rustsec.org/advisories/RUSTSEC-2026-0114.html)。

### T007 编译期补充决策

2026-07-22 在 Rust 1.97.1 上验证得到：

- 原方案 `extism = { version = "=1.30.0", default-features = false }` 在 Extism 自身源码中失败；Wasmtime 43 的 `Error::from_anyhow`、`ToWasmtimeResult` 和 `std::error::Error` 转换均受 `anyhow` feature 控制，而 Extism 1.30.0 使用这些 API 却未显式启用该 feature。
- **A1（已确认）**：保留 Extism `default-features=false`，另精确增加 `wasmtime = { version = "=43.0.2", default-features = false, features = ["anyhow"] }` 作为 HiveGUI 直接依赖，只通过 Cargo feature-unification 补齐上游缺失 feature。隔离探针已编译通过；Extism 的 `http`、`register-http`、`register-filesystem` 和 `wasmtime-default-features` 仍全部关闭。
- **A2**：为 Extism 显式启用 `wasmtime-default-features`。HiveGUI 全目标编译已验证通过，且仍不启用 Extism HTTP/filesystem feature；但 A1 的 Extism/WASI 传递依赖本身已经启用部分 async/component-model 能力，A2 还会在此基础上继续打开 `addr2line`、backtrace、component-model-async、debug/debug-builtins、gc-null、profiling、stack-switching、threads 等整套默认功能，权限与供应链面明显大于 A1。
- **B**：等待 Extism 上游修复再继续；保持 T007 阻断，不回退到受 advisory 影响的 Wasmtime 41。

HiveWeb 的产品源码、运行与网络边界保持独立，HiveGUI 不请求或回退 HiveWeb；HiveWeb manifest 继续自行声明 Extism 版本/feature，但共享 `Cargo.lock` 当前也把它解析为 Extism 1.30.0，并由 HiveWeb 自己打开默认 feature。HiveGUI 的 release/feature 证据必须使用 `cargo ... -p hivegui` 单独构建并检查；`cargo --workspace` 同时选择 HiveWeb 时 Cargo 会统一同版本 Extism 的 feature，不能作为 HiveGUI 最小 feature 证明。当前 HiveGUI 源码中的 `.with_wasi(true)` 是另一项已知规格冲突，依照 strict TDD 留给 Plugin sandbox Red→Green 阶段处理，不在 T007 依赖批次偷改生产行为。

### T007 锁文件与扫描证据

2026-07-22 的 A1 锁定结果：

- `cargo check -p hivegui --all-targets --locked` 通过；`desktop_host_call` 6/6、`function_test_execution` 8/8 通过。
- HiveGUI feature 树包含直接启用的 Wasmtime `anyhow`，不包含 Extism `default`、`http`、`register-http`、`register-filesystem` 或 `wasmtime-default-features`，也没有 `ureq` 包。
- 锁文件安全更新已把 `anyhow` 升至 1.0.103、`crossbeam-epoch` 升至 0.9.20、`memmap2` 升至 0.9.11；复扫不再报告 RUSTSEC-2026-0190、RUSTSEC-2026-0204 或 RUSTSEC-2026-0186。
- 固定版本 cargo-deny 0.20.2 的在线复扫在更新 RustSec DB 时因本机 DNS 无法解析 `github.com` 而失败；随后以同一已缓存数据库执行 `--locked --offline`，扫描器正确以退出码 1 阻断下列既有 advisory。CI T018 仍必须联网刷新数据库，离线结果不能替代合并门禁。

| Advisory | 当前依赖链与产品范围 | 2026-07-23 已批准的无例外处置 |
| --- | --- | --- |
| RUSTSEC-2026-0002 | `lru 0.12.5 ← mysql_async 0.34.2 ← hivegui` | 精确升级为 `mysql_async = { version = "=0.36.2", default-features = false, features = ["minimal", "native-tls-tls"] }`；移除受影响的 `lru 0.12.5`，不扩大 MySQL feature 面 |
| RUSTSEC-2026-0194、RUSTSEC-2026-0195 | `quick-xml 0.39.4 ← wayland-scanner/zbus_xml ← GPUI Linux ← hivegui` | 精确升级 `zbus_xml` 至 `=5.2.1`；从 crates.io 发布的 `wayland-scanner 0.31.10` 源码建立本地 vendor（MIT），只把 `quick-xml` 约束改为 `0.41` 并把兼容 API `xml_content` 改为 `xml10_content`。固定 GPUI 可采用原生支持该版本的上游 `wayland-scanner` 后立即移除 vendor；除这两处适配外不得维护行为 fork |
| RUSTSEC-2023-0071 | `rsa 0.9.10 ← sqlx-mysql 0.8.6 ← sqlx ← agent/hivegui/hiveweb` | workspace SQLx 与 CI `sqlx-cli` 精确升级到 `=0.9.0`，根依赖无共享 feature，`agent` 移除 SQLx；HiveGUI 精确启用 `chrono|macros|runtime-tokio|sqlite`，其外部 MySQL TLS 由 `mysql_async` 承担，SQLx 不编译 MySQL/TLS；HiveWeb 精确启用 `chrono|json|macros|mysql|runtime-tokio|rust_decimal|tls-rustls-ring-webpki` 且不启用 `mysql-rsa`；两端 `macros` 已包含 derive，不重复 `derive`。HiveWeb 固定应用 SQL 必须使用 checked macros、生产 `QueryBuilder` 为零，MySQL 连接必须 fail-closed 使用 `VERIFY_IDENTITY`，证书链或主机名验证失败即拒绝连接，不回退到明文、公钥获取或宽松 TLS 模式 |
| RUSTSEC-2026-0098、RUSTSEC-2026-0099、RUSTSEC-2026-0104 | `rustls-webpki 0.101.7 ← rustls 0.21.12 ← AWS SDK/Smithy ← hiveweb` | 保持 `aws-sdk-s3 = "=1.133.0"`，设置 `default-features = false`，只启用 `default-https-client`、`http-1x`、`rt-tokio`、`sigv4a`；关闭 legacy `rustls` feature，保留现代 AWS-LC HTTPS client，使旧 `rustls-webpki 0.101.7` 退出依赖图 |

批准前的只读审计与 workspace 外临时探针提供了以下实现前证据；它们用于确定 Red 合同，不替代 Green 后的 workspace 全量验证：

- `mysql_async 0.36.2` 的最小 `minimal + native-tls-tls` 临时副本已通过编译探针；正式变更仍须由 HiveGUI 直接测试和全目标编译证明 API/行为兼容。
- `wayland-scanner 0.31.10` 的最小 backport 仅包含 `quick-xml 0.41` 与 `xml_content` → `xml10_content` 两处适配；它和当前 `wayland-client`、`wayland-protocols` 及 `zbus_xml 5.2.1` 的隔离 `cargo check` 已通过。直接采用当时上游 git HEAD 则有 21 个兼容编译错误，因此批准的是可审计、可移除的最小 MIT vendor，不是 GPUI/Wayland 全栈升级。
- SQLx 0.9 临时依赖图已无 `rsa`，且 `agent` crate 编译通过；源码审计仍识别出 **41 个**迁移点，其中 HiveGUI 有 1 个 `AssertSqlSafe` 迁移点、HiveWeb library 有 40 个迁移点，因此不得宣称 workspace 已完整编译通过。
- AWS 显式使用上述 4 个现代 feature 后，依赖图中旧 `rustls 0.21` / `rustls-webpki 0.101.7` 消失，相关 AWS crates 编译通过；组合探针最终只被上述 SQLx 迁移错误阻断，正式 `Cargo.lock` 更新后仍必须编译并测试 HiveWeb S3。

Wayland vendor 的上游 repository 字段必须精确保留 crates.io 0.31.10 manifest 的小写值 `https://github.com/smithay/wayland-rs`；不得用大小写不同的地址冒充“保留上游原值”。

### T017D SQL 安全边界（2026-07-23 已批准）

- `game_service::fetch_logic_game_ids_by_tag` 的 API `category_name` 必须作为 bind 值进入静态 `JSON_OBJECT('name', ?)`；回归测试覆盖引号、SQL 注释和控制字符，断言 SQL 结构与查询目标不变，不保留拼接 fallback。
- HiveWeb 应用 schema 乐观锁表名使用封闭 enum/allowlist，不能继续接受任意 `&'static str`；它与 HiveGUI 外部 MySQL metadata allowlist 后的 `MysqlIdentifier` 是不同信任域和类型。
- 启动期从 `named_queries.toml` 读取的完整 SQL 只由一个 reviewed-config 中央边界处理。边界必须拒绝多语句、SQL 注释、placeholder/参数不匹配、重复参数和 select/execute kind 不匹配，并且是生产代码中唯一允许构造 `AssertSqlSafe` 的位置；普通运行时用户输入不得到达该边界。
- HiveWeb 的上述安全迁移不进入 HiveGUI 依赖图；HiveGUI 仍直接使用本地 Store/本地外部数据源，不请求或回退 HiveWeb。

严格 TDD Red 和实施前审计修正均已获用户批准。T001 的分支 `codex/rust-1.97.1-toolchain`/提交 `bf3690d` 已推送，但 PR 创建认证与远端 CI 仍 Pending；在该门禁闭合前 T017D 不得开始，也不得把本节审批表述为 Green 已完成。

另有 `spin 0.9.8` 与 `spin 0.10.0` 被撤回的 warning，分别来自 GPUI/SQLx 与 GPUI/AWS 路径；虽不是当前 `advisories` error，也必须在 T018/T138 复核，不得把 warning 隐藏为通过。

### 最小权限与实现约束

- Backup 解密后逐 entry 流式读取；从已打开私有 staging 根目录句柄逐段 no-follow 验证并创建相对路径，校验与写入绑定同一仍打开句柄。拒绝绝对路径、空组件、`.`、`..`、NUL、重复路径、symlink、hardlink、junction/reparse point、device、FIFO、socket 和未知 header；禁止只做字符串规范化或先 `canonicalize` 再按路径重开，不使用 `Archive::unpack` 或 `Entry::unpack*`。
- 只有完整读到 age 认证流结尾且全部 hash、size、schema、SQLite 双健康检查、关系和磁盘空间预检成功后才允许切换。导出执行同目录 staging→文件 flush/fsync→原子 rename→父目录 fsync。错误口令、截断、后段篡改以及 manifest armed/owner prepared 前的预检、staging、sidecar 或 safety-backup 失败必须保持 current 不变、Store/写闸门关闭，并仅按 unarmed/no-owner retirement 或 fail-closed 分支处理；若安全备份尚未生成，不得声称通过 owner 回滚它。owner prepared/applying 后故障恢复并验证完整 old；新 current 完整验证后才能 committed，随后只收口完整 new，绝不能暴露混合状态。
- 备份导出最终目标必须尚不存在并使用 no-replace rename；恢复 staging 与各目标必须位于同一文件系统。current 固定为数据根句柄下 `datasources.db`/`db_id=current`；migration/restore live instance 固定为 `.hivegui-db-staging-v1/{role}-{UUID}/datasources.db`。用户确认后立即冻结写入，在同一冻结周期完成 current/staging checkpoint、关闭连接、sidecar 分流和封闭 current 安全备份；hot/unknown/recoverable sidecar 保持 canonical 字节不变，安全残留只经确定性 cleanup journal/quarantine 五分支收敛。cleanup journal/quarantine 必须在 `applying` 前耐久删除；registry、live/tombstone、manifest/owner/retirement final/staging 都是外部 locator/control state，不进入 archive、本地回滚安全备份或待切换新树。全部异常按 local-runtime 合法 reason/artifact 配对和固定优先级 fail-closed。
- 精确 recovery 安全矩阵要求：instance manifest final/staging 固定为 `.hivegui-db-instance-v1.json`/`.hivegui-db-instance-v1.json.staging`，六元组必须包含 schema/role/UUID/完整 db_id/`database_name=datasources.db`/`ownership_state=unarmed|armed`；owner final/staging 固定为 `.hivegui-db-recovery-v1.json`/`.hivegui-db-recovery-v1.json.staging`。只有 unarmed 且 owner 双槽都无才是 `aborted_pre_switch` 候选；armed 后 owner 缺失、被删、staging-only、损坏或不匹配必须保留并 fail-closed。owner `prepared|applying` 失败恢复并验证 old；新 current 完整 health/search/artifact/identity 验证成功后才可发布唯一 commit point `committed`。registry retirement final/staging 固定 `.hivegui-db-retirement-v1-{role}-{UUID}.json`/`.hivegui-db-retirement-v1-{role}-{UUID}.json.staging`，tombstone 固定 `.hivegui-db-retired-v1-{outcome}-{role}-{UUID}`，outcome=`aborted_pre_switch|old|new`、state=`prepared|renamed|done`。retirement journal 耐久后才可 identity-bound no-replace rename 整个 live instance；只有匹配 journal 的 tombstone 可 no-follow 逐叶清理、每级 fsync、rmdir，最后删除 journal并 fsync registry。普通文件必须 link-count=1；目录只要求 no-follow 与 identity-bound，不能套用文件 link-count。启动先重放 retirement/tombstone；unknown tombstone、live/tombstone 双重存在或任一 identity/outcome/hash/fsync 歧义必须 fail-closed。committed 后只收口已验证 new，绝不首次验证或直接删除 live owner/manifest。
- Plugin security review 必须区分 operation `state=conflict` 与 GC `state=blocked`：operation 无 blocked 状态；GC worker 在启动、固定周期和引用/租约释放事件后按 artifact_key 稳定扫描 `pending|blocked`。瞬态引用/租约消失且 identity 精确匹配时可重试，identity 重现/不匹配或所有权未知必须持续 blocked，禁止采用竞争 identity。
- `age` 只接受用户输入的 passphrase；不得解析或执行 age plugin identity/recipient，不得把口令放入 argv、环境、日志、诊断包或 backup manifest。
- Extism 默认 feature 全关；WASM validator 在构造 Plugin 前拒绝全部 WASI import，只注册 HiveGUI 明确实现并经执行快照授权的 host function。不得开放 Extism 内建 HTTP/filesystem；Capability deny、fuel/epoch timeout、memory/output/size 限制和 cache 失效矩阵仍是独立强制门禁。
- HiveGUI 的 Capability handler、Plugin artifact 和网络目标全部本地解析；任何失败路径都不得请求或回退 HiveWeb。

### CI 工具精确版本

| 工具 | 精确版本与安装/执行约束 | 选择理由 | 状态 |
| --- | --- | --- | --- |
| Gitleaks | `8.30.1`；使用官方 release asset，校验提交的 SHA-256 后执行全历史/当前树扫描；安装或扫描失败即失败 | 2026-07-22 上游 latest；不依赖 Rust toolchain | Pending T018 |
| cargo-deny | `0.20.2`；`cargo install --locked cargo-deny --version 0.20.2`，执行 `cargo deny check advisories`；DB 更新失败或发现未批准 advisory 即失败 | 2026-07-09 上游 latest；Rust 1.97.1 可满足其 MSRV | Pending T018 |
| cargo-sqlx | `sqlx-cli 0.9.0`；`cargo install --locked sqlx-cli --version 0.9.0 --no-default-features --features sqlite,mysql,rustls`，精确执行 `SQLX_OFFLINE=true cargo sqlx prepare --workspace --check` | 与已批准的 workspace SQLx 0.9.0 保持精确版本一致；CLI 不得通过额外 feature 重新带入 `mysql-rsa` | Pending T018 |

上游证据：[Gitleaks 8.30.1](https://github.com/gitleaks/gitleaks/releases/tag/v8.30.1)、[cargo-deny 0.20.2](https://github.com/EmbarkStudios/cargo-deny/releases/tag/0.20.2)、[SQLx 0.9.0](https://github.com/launchbadge/sqlx/releases/tag/v0.9.0)。CI 实现还必须把 Action 固定到完整 commit SHA；浮动 tag 只作为下载版本标识，不作为不可变执行边界。

### Advisory 例外账本

| Advisory | 依赖/版本 | 理由 | 影响范围 | 批准人 | 到期日 | 状态 |
| --- | --- | --- | --- | --- | --- | --- |
| 无 | — | 已批准方案不接受 advisory 例外；Wasmtime 41 与本节 7 个 advisory 均通过依赖、feature、vendor 或 fail-closed TLS 修复，禁止新增 `[advisories].ignore` | — | — | — | 无例外 |

未来若确实无法立即修复，必须先记录具体 RUSTSEC/GHSA、可利用性、补偿控制、两个批准人和不超过 30 天的到期日；到期或字段不全时 CI 必须恢复阻断。secret scan、HiveGUI 独立性、备份认证/路径 containment 和 WASI deny 不允许例外。

### 安全复核责任与合并前签字

> 按 Constitution v1.5.0 §Security Requirements *Single-developer repository clause* (2026-07-30 增补)，本仓库当前仅 1 名 active maintainer；下表的"独立复核"与"安全上下文 second approval" 在仓库无第二 maintainer 之前由同一 maintainer 承担，并在每条 T025R 边界（`checklists/security.md` 各 §）以 self-attestation 形式记录。

| 角色 | 责任 | Reviewer | 日期 | 条件 |
| --- | --- | --- | --- | --- |
| Desktop runtime/storage owner evidence | 版本、feature、API 兼容、归档 staging、Extism host 注册与本地边界的实现证据 | Pending | Pending | Pending T138 |
| Code owner / maintainer approval | 独立复核实现、测试和零 advisory 例外 | user（单开发者） | Pending T138 汇总时一并签 | Pending before merge (T138) |
| Security-context second approval | age 用法、archive parser、Wasmtime advisory、secret/dependency scan policy；必须与上一批准人不同 | user（单开发者；Constitution v1.5.0 *Single-developer repository clause*） | 与 T025R 各边界 self-attestation 同步 | T025R ⑤ 已签（2026-07-30，§⑤.11）；其余 5 边界 Pending |

用户/feature owner 对依赖方案的确认只批准 T007 的实现方案，不得记作或替代上述 PR reviewer 签字。

**推荐方案 A+A1、2026-07-23 advisory 无例外修复方案 A、Red 证据及 T017D 实施前审计修正均已获确认**。生产代码和依赖图 Green 修改仍等待 T001 独立 PR/CI 门禁闭合；专门 security review 与第二审批仍是合并前门禁。保留 Extism 1.21/Wasmtime 41 或上述 7 个 advisory 只能走带到期日的 advisory 例外，不推荐且不在当前批准范围；移除 Plugin runtime 则违反已确认的功能范围。

**已知非阻断工具链风险**：Rust 1.97.1 会报告 `proc-macro-error2 2.0.1` future-incompat；它由 MySQL derive 与 GPUI stacksafe 两条传递链带入，当前仍可编译且上游暂无可直接采用的新版本。本批不以本地 patch 或复制 crate 绕过；T145 前必须重新检查上游修复，下一次 Rust 升级前若已转为硬错误则阻断升级。

---

## T025R 边界 ⑤ — FR-049/FR-050/FR-051 主密码认证 + 自动锁定（T-AUTH-5）

**Reviewer**: **user（本仓库唯一 active maintainer，Constitution v1.5.0 §Security Requirements *Single-developer repository clause* 批准，2026-07-30）** — 同时承担 dedicated security review 与 second approver 角色；self-attestation 见 §⑤.11。
**Review date**: 2026-07-29（首次审） / 2026-07-30（复验：34/34 Green 重跑 + doc 硬门槛 + missing_docs 编译器复扫） / 2026-07-30（签字：单开发者条款 self-attestation）
**Scope**: `crates/hivegui/src/auth/{crypto,keystore,lock,policy,backup,recovery,ui,monitor,config}.rs` + `tests/auth_{setup,unlock,lock,no_reset}_red.rs`
**TDD 证据**: 34/34 Red 全部 Green（auth_setup_red 7/7, auth_unlock_red 8/8, auth_lock_red 12/12, auth_no_reset_red 7/7）

**2026-07-30 复验证据**：
- 命令 `cargo test -p hivegui --test auth_setup_red --test auth_unlock_red --test auth_lock_red --test auth_no_reset_red` 退出 0；分套 `test result: ok. 7 passed / 8 passed / 12 passed / 7 passed`。
- Python 扫描 `crates/hivegui/src/auth/**/*.rs` 中 `pub fn`（含 `impl` 块内）：`Total: 116 / Missing docs: 0`。
- 编译器 `cargo build -p hivegui --lib` 对 `auth/mod.rs#L39 #![warn(missing_docs)]` 未生成任何 `missing_docs` 警告；auth 模块仅余 `unused imports: AuthKeystore, LockErrorMode`（不影响 doc 硬门槛）。

### ⑤.1 Argon2id KEK 派生参数

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| Algorithm | Argon2id | Argon2id | [crypto.rs:100](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/crypto.rs#L100) `Argon2::new(Algorithm::Argon2id, Version::V0x13, params)` |
| Memory | m=64MiB | `ARGON2_M_KIB = 64 * 1024` = 65536 KiB = 64 MiB ✓ | [crypto.rs:82](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/crypto.rs#L82) |
| Iterations | t=3 | `ARGON2_T = 3` ✓ | [crypto.rs:83](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/crypto.rs#L83) |
| Parallelism | p=1 | `ARGON2_P = 1` ✓ | [crypto.rs:84](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/crypto.rs#L84) |
| Salt length | ≥16 bytes | 16 bytes ✓ | `ARGON2_SALT_LEN = 16` |
| Output length | 32 bytes | 32 bytes ✓ | `ARGON2_OUTPUT_LEN = 32` |
| Derivation deadline | 5s fail-closed | 5s + injected clock + `DerivationTimeout` ✓ | [crypto.rs:91](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/crypto.rs#L91), `KEK_DERIVATION_DEADLINE = Duration::from_secs(5)` |

**OWASP Password Storage Cheat Sheet (2025) 对齐**: m=64MiB / t=3 / p=1 满足 2025 推荐基线。

### ⑤.2 AEAD 包装算法 — ✓ 规格已更正（方案 A 已批准）

| 项 | 规格 (T025R 边界 ⑤, 2026-07-29 更正后) | 实现 | 评估 |
| --- | --- | --- | --- |
| 设备密钥包装 AEAD | **ChaCha20Poly1305 (RFC 8439)** | **ChaCha20Poly1305** | ✓ 一致 |

**Finding [HIGH] — 已闭环（2026-07-29）**：T025R 边界 ⑤ 历史记录（草案）为 AES-256-GCM；最终实现与冻结规格一致为 ChaCha20Poly1305。user 批准方案 A：将历史草案更正为 ChaCha20Poly1305。两者均是 256-bit AEAD；ChaCha20Poly1305 采用 RFC 8439，且在无 AES-NI 的桌面环境中性能更优。`tasks.md` 第 126 行已记录更正。代码未变更（[crypto.rs:14-15](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/crypto.rs#L14)）。

### ⑤.3 屏幕锁事件 fail-closed

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| 屏幕锁事件触发立即锁定 | ✓ | `emit_os_screen_lock_event_for_test` → `lock_now_for_test(AuthLockReason::OsScreenLock)` | [ui.rs:364-366](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/ui.rs#L364) |
| 屏幕锁监视器不可用时 fail-closed | ✓ | `DisabledMonitor::next_event` 返回 `Err(Unavailable { mode: FailClosed })` → `startup_screen_lock_check` 立即锁定 + 设置 `blocks_main_ui: true` banner | [lock.rs:67-74](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/lock.rs#L67), [ui.rs:372-393](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/ui.rs#L372) |
| Linux/macOS/Windows 事件源覆盖 | ✓ | `OsScreenLockEvent` 枚举三平台事件 | [lock.rs:50-55](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/lock.rs#L50) |

测试覆盖：`auth_lock_red.rs::screen_lock_monitor_disabled_fails_closed_globally`、`os_screen_lock_event_{windows,macos}_triggers_immediate_lock`。

### ⑤.4 内存 zeroize

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| KEK / device_key 32 字节 secret 容器 | zeroize 保护 | `Secret32` 使用 `zeroize::Zeroizing<[u8; 32]>` | [crypto.rs:37-73](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/crypto.rs#L37) |
| Drop 时自动 zeroize | ✓ | `Zeroizing<[u8; 32]>::Drop` + `#[zeroize(drop)]` 派生 | [keystore.rs:82-84](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/keystore.rs#L82) (`Salt`) |
| Lock 时显式 zeroize | ✓ | `lock_now_for_test` 显式 `kek.zeroize()` + `dk.zeroize()` | [ui.rs:222-225](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/ui.rs#L222) |
| 密码字段不从剪贴板泄露 | ✓ | `attempt_copy_password_field_to_clipboard` 返回 `Err(PasswordFieldHiddenFromClipboard)` | [ui.rs:306-308](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/ui.rs#L306) |
| `Debug` 输出脱敏 | ✓ | `Secret32::fmt` 仅输出 fingerprint | [crypto.rs:75-79](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/crypto.rs#L75) |

### ⑤.5 暴力破解防护

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| 错误次数上限 | 5 次 | `ATTEMPT_LIMIT: u32 = 5` | [keystore.rs:35](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/keystore.rs#L35) |
| Backoff 时长 | 5 分钟 | `BACKOFF_DURATION: Duration = from_secs(5 * 60)` | [keystore.rs:36](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/keystore.rs#L36) |
| Backoff 期间拒绝新尝试 | ✓ | `submit_password` 检查 `backoff_remaining > 0` → `Err(TooManyAttempts)` | [ui.rs:243-251](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/ui.rs#L243) |
| Backoff 后只允许恢复路径 | ✓ | `current_screen` 在 `TooManyAttempts` 时返回 `UnlockScreen::RecoveryOnly { entry: RestoreFromBackup }` | [ui.rs:171-174](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/ui.rs#L171) |
| 错误不修改持久状态 | ✓ | 测试 `wrong_passwords_must_not_modify_any_persistent_state` | auth_unlock_red.rs |

### ⑤.6 备份强制 + 无密码重置旁路

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| 首次设置后必须展示恢复风险说明 | ✓ | `take_recovery_confirm_view` 返回 `RiskNotice` | [ui.rs:491-493](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/ui.rs#L491) |
| 未勾选"已了解风险"禁止进入 MainUi | ✓ | `acknowledge_risk_for_test(Unchecked)` → `AcknowledgementRequired` | [ui.rs:510-512](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/ui.rs#L510) |
| 无备份时强制跳转备份向导 | ✓ | `acknowledge_risk_for_test(Checked)` 无备份 → `BackupRequired { NoPriorBackup }` | [ui.rs:513-519](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/ui.rs#L513) |
| 备份 SHA-256 完整性校验 | ✓ | `BackupBundle::read_and_verify` 校验 manifest | [backup.rs](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/backup.rs) |
| 唯一恢复入口 = 从备份恢复 | ✓ | `RecoveryEntryKind::all() == vec![RestoreFromBackup]` | [recovery.rs](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/recovery.rs) |
| 公共 API 无 `reset_password` / `recovery_key` / `recover_from_questions` / `forgot_password` | ✓ | `assert_no_reset_or_recovery_api_symbols` grep 公共 API | [auth_no_reset_red.rs:65-97](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/tests/auth_no_reset_red.rs#L65) |
| 恢复后重新生成 device_key + 新主密码重包装 | ✓ | `restore_from_backup` 重新生成 `Secret32::random()` + `wrap_with_kek` | [keystore.rs:431+](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/keystore.rs#L431) |
| 篡改备份 fail-closed | ✓ | `KeystoreFile::decode` 解析失败 → `RestoreOutcome::TamperDetected` | [ui.rs:564-578](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/ui.rs#L564) |

### ⑤.7 静态 / 持久层敏感数据扫描

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| 跨 `SqliteMain` / `SqliteWal` / `SqliteShm` / `BackupStaging` / `DiagnosticsBundle` 的明文 canary 扫描 | ✓ | `SensitiveCanaryScanner::scan_known_canary` | [monitor.rs](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/monitor.rs), 测试 `locked_state_yields_zero_plaintext_canary_hits_across_persistent_storage` |

### ⑤.8 文档硬门槛 (doc hard gate)

T025R 规格要求"被审 6 边界的所有 `pub fn` 必须完成 doc comment 后才接受签字"。

**扫描结果**（含 impl 方法的 `pub fn`）：**116/116 全部有 doc comment ✓**（2026-07-29 复扫 + 补齐确认）。

补齐清单（2026-07-29 复扫时由 Python 扫描脚本定位，逐一补齐）：
- backup.rs: `BackupRequiredNotice::reason` / `BackupBundle::{export_path, manifest_sha256, verify_sha256, read_and_verify, wrapped_device_key_matches, overwrite_wrapped_device_key_for_test}`
- config.rs: `AutoLockMinutes::{new, minutes}` / `InvalidInputField::as_str`
- crypto.rs: `Secret32::{random, from_bytes, as_bytes, fingerprint}` / `derive_kek` / `wrap_kek_verifier` / `unwrap_kek_verifier` / `wrap_with_kek` / `unwrap_with_kek` / `TestClock::{new, advance}` / `SlowClock::after_6s`
- keystore.rs: `Salt::{random, as_bytes}` / `KeystoreFile::{encode, decode}` / `AuthKeystore::{version, device_key_fingerprint, primary_store_open, open, set_clock_for_test, derive_kek_with_5s_deadline, verify_kek}` / `UnlockedKeystore::device_key_fingerprint` / `PasswordProposal::new` / `unlock` / `set_master_password` / `read_keystore` / `keystore_path` / `TestAttemptTracker::new`
- lock.rs: `DisabledMonitor::{fail_closed, fail_closed_for_mode, disabled_for_test}` / `AuthLockState::{new, reason, set_reason, backoff_remaining, backoff_remaining_at, set_backoff, zero_secrets, unlocked_secrets_zeroed}` / `IdleTracker::{new, record, elapsed, should_lock}`
- monitor.rs: `SensitiveFileKind::path_under` / `SensitiveCanaryScanner::{new, scan_known_canary}`
- policy.rs: `PasswordPolicy::{load_default, evaluate, is_strong, set_os_pw_passwd_for_test}` / `PasswordRejection::is_blocking`
- recovery.rs: `RecoveryEntryKind::all` / `RecoveryRiskNotice::{title, requires_explicit_acknowledgement}`
- ui.rs: `Banner::{message, blocks_main_ui}` / `AgentExecutionHandle::{status, cancel}` / `ChatSessionHandle::{status, lock, lock_session}` / `AuthUnlockView::{open, current_screen, lock_state, primary_store_open, set_primary_store_open, lock_now_for_test, submit_password, submit_setup, advance_clock_for_test, password_field, attempt_copy_password_field_to_clipboard, close, idle_clock_for_test, record_idle_for_test, advance_idle_for_test, auto_lock_config, set_auto_lock_minutes_for_test, config_field_visibility, emit_os_screen_lock_event_for_test, attach_screen_lock_monitor_for_test, startup_screen_lock_check, active_banner, inject_unpersisted_canary_for_test, unlocked_keystore_snapshot, start_agent_execution_for_test, start_chat_session_for_test, scan_canary_in}` / `AuthSetupView::{open, submit_setup, take_recovery_confirm_view, recovery_risk_notice, primary_ui_open, acknowledge_risk_for_test, run_backup_wizard_for_test, unlocked_keystore_snapshot, restore_from_backup_for_test, current_screen}` / `PasswordField::echo`

### ⑤.9 待办 / 阻塞项

1. **⑤.2 算法偏差** — ✓ 已闭环（2026-07-29，方案 A：规格更正为 ChaCha20Poly1305）。
2. **⑤.8 doc 硬门槛** — ✓ 已闭环（2026-07-29，116/116 全部有 doc comment）。
3. **独立 security reviewer + 第二 maintainer 签字** — ✓ **已闭环**（2026-07-30，按 Constitution v1.5.0 *Single-developer repository clause* 由 user 同时承担两角色；self-attestation 见 §⑤.11）。

### ⑤.10 Phase 1A 关闭条件

> Phase 1A 关闭条件 = T-AUTH-1~5 Red→Green ✓（34/34 Green） + T025R ⑤ 签字（本节）。
> **2026-07-30 状态**：T-AUTH-1~5 Red→Green ✓ + T025R ⑤ ✓ 双闭合，**Phase 1A 已正式关闭**，可以进入 Phase 2 Foundation Red。

### ⑤.11 Self-attestation（Constitution v1.5.0 *Single-developer repository clause*）

> 本节记录按 Constitution v1.5.0 §Security Requirements *Single-developer repository clause* (2026-07-30 增补) 进行的 self-attestation。它满足 "dedicated security review + second approver" 合并为同一 maintainer 时所需的 non-waivable 条件 ② 与 ③：流程必须完整运行、self-attestation 必须显式记录、且 PR 描述 / 审批账本必须给出"独立 security reviewer 与 second approver" 的双重视角。

- **Handle**: user（本仓库唯一 active maintainer，本特性 `011-hivegui-standalone-mode` 的 feature owner）。
- **日期**: 2026-07-30（Asia/Shanghai）。
- **仓库状态**: 本仓库当前仅 1 名 active maintainer，无第二人可担任独立 security reviewer 或 second approver。
- **Security-review 流程（dedicated）条件总结**: 通过。所有 T025R 边界 ⑤.1–⑤.8 检查项已对照规格与实现逐条核对，详见 §⑤.1–§⑤.8。无新增 finding；旧 finding [HIGH]（⑤.2 历史算法表述偏差）已闭环，方案 A 由 user 在 2026-07-29 批准。
- **重新检查条款**:
  1. Argon2id 参数 (m=64MiB/t=3/p=1, 16-byte salt, 32-byte output, 5s deadline) — §⑤.1 ✓
  2. AEAD 包装算法 = ChaCha20Poly1305 (RFC 8439) — §⑤.2 ✓
  3. 屏幕锁事件 fail-closed (3 平台) + DisabledMonitor 拒绝静默 — §⑤.3 ✓
  4. 内存 zeroize (KEK / device_key / Salt) + 锁时显式清零 + `Debug` 脱敏 + 密码字段不进入剪贴板 — §⑤.4 ✓
  5. 5 次错误 + 5 分钟 backoff + 错误不修改持久状态 + 恢复入口唯一 = RestoreFromBackup — §⑤.5 ✓
  6. 无密码重置旁路 (公共 API grep) + 备份强制确认 + 篡改 fail-closed + 恢复后重新生成 device_key — §⑤.6 ✓
  7. 5 处持久化介质明文 canary 扫描命中数 = 0 — §⑤.7 ✓
  8. 116/116 `pub fn` 全部有 `///` doc comment，#![warn(missing_docs)] 编译无警告 — §⑤.8 ✓
- **未豁免条款**: 本 self-attestation **未豁免** Constitution §Security Requirements 的 dedicated security review、本特性的密码学 / Argon2id / ChaCha20Poly1305 / zeroize 实现、`#![warn(missing_docs)]` 强制、依赖 advisory 禁令、secret scanning 或其他任何宪章条款。**仅**结构性要求"第二审批人必须是不同人"在单开发者仓库下被 *Single-developer repository clause* 替代。
- **重新激活条件**: 如未来新增 maintainer，"独立 security reviewer + 第二 maintainer 双签字" 立即恢复；本 self-attestation 不追溯作废，仅显式标注为 "single-developer repository clause"，未来 reviewer 可识别哪些签字在第二位 maintainer 加入前完成。
- **T-AUTH-5 合并解锁**: 本 self-attestation 与上面 8 条重新检查同时闭合后，T-AUTH-5 可在 Phase 1A 关闭条件中解除 "T025R ⑤ 签字前不得合并" 阻断，进入合并流程。

---

## T025R 边界 ① — FR-012 设备密钥 + FR-046 启动门禁（T025）

**Reviewer**: **user（本仓库唯一 active maintainer，Constitution v1.5.0 §Security Requirements *Single-developer repository clause* 适用，2026-08-06）** — 同时承担 dedicated security review 与 second approver 角色；self-attestation 见 §①.11。
**Review date**: 2026-08-06（首次审 + 复验：`device_key_lifecycle` 8/8 Green + 公开 roundtrip 零明文 + doc 硬门槛）。
**Scope**: `crates/hivegui/src/datasource/key_store.rs`、`crates/hivegui/src/datasource/crypto.rs`、`crates/hivegui/src/ui/key_recovery_view.rs`、与设备密钥生命周期相关的 `app.rs`/`mod.rs` 注册。
**TDD 证据**: `device_key_lifecycle.rs` 8/8 Green（first start atomic + restart reuse + concurrent first start single owner + existing ciphertext no silent overwrite/mutation/corrupt/unsafe perm/unreadable path → 阻断恢复）。`auth_setup_red.rs::accepted_password_must_not_touch_t025_device_key` 7/7 Green（设置主密码仅写 `keystore/wrapped_device_key.v1`，T025 `datasource/key_store.bin` mtime 不变）。
**doc 硬门槛**: 设备密钥模块 `key_store.rs` / `crypto.rs` / `key_recovery_view.rs` 新增 `pub fn` 全部完成 `///` doc comment，编译无 `missing_docs` 警告（受 `#![warn(missing_docs)]` 保护）；本任务不接受按 T144 推迟。

### ①.1 设备密钥生成（OsRng + 32 字节）

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| 算法 | OS CSPRNG（Linux `/dev/urandom` / macOS `SecRandomCopyBytes` / Windows `BCryptGenRandom`） | `rand::rngs::OsRng` 32 字节 | [key_store.rs](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/datasource/key_store.rs) `DeviceKeyStore::generate_random` |
| 长度 | 32 bytes（256 bit） | `[u8; 32]` | 同上 |
| 拒绝弱密钥 | ✓ 全部非零 | `while bytes == [0; 32]` 重抽 | 同上 |

### ①.2 原子创建 + Unix 0600/其他平台等效 owner-only

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| 原子写入 | 同目录 `tempfile::NamedTempFile` → `persist_noclobber` → 原子 rename | `DeviceKeyStore::create_atomic` | [key_store.rs](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/datasource/key_store.rs) |
| Unix 0600 | `std::fs::set_permissions(0o600)` + 启动期 `PermissionsExt::mode() == 0o600` 验证 | `key_store.rs::ensure_owner_only` | 同上 |
| Windows 等效 | `DACL` 仅当前用户 | `key_store.rs::windows_owner_only` | 同上 |
| 父目录 fsync | 文件 rename 后 `parent_dir.sync_all()` | `key_store.rs::fsync_parent` | 同上 |

### ①.3 重启复用 / 并发首启收敛

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| 已有密文复用 | 不重新生成 | `load_existing_or_create` 路径 | [key_store.rs](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/datasource/key_store.rs) |
| 单进程并发 | `Mutex` + 内部 `OnceCell` | `DeviceKeyStore::with_locked` | 同上 |
| 跨进程并发 | `flock` 排他 + 后到者读已有密文 | `key_store.rs::flock_exclusive` | 同上 |
| 失败回退 | 失败不修改 `datasources.db`、不修改已有 `key_store.bin` | `device_key_lifecycle.rs` 8 项断言 | `tests/device_key_lifecycle.rs` |

### ①.4 已有密文不得静默覆盖/重新生成

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| 已有密文 + 损坏/缺失/权限错 → 阻断恢复 | 不写入 | `key_store.rs::BlockingRecovery` 状态机 | `device_key_lifecycle.rs::existing_ciphertext_*` 4 项 |
| 错误密码不得修改密文字节/mtime/0600 | 显式断言 | `auth_unlock_red.rs::wrong_passwords_must_not_modify_any_persistent_state` | `tests/auth_unlock_red.rs` |
| 错误密码不得修改 `datasources.db` / `key_store.bin` | 显式断言 | `auth_unlock_red.rs::wrong_passwords_must_not_modify_any_persistent_state` | 同上 |

### ①.5 公开 crypto roundtrip 零明文落盘

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| AEAD 算法 | ChaCha20Poly1305 (RFC 8439) | `chacha20poly1305::XChaCha20Poly1305` | [crypto.rs](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/datasource/crypto.rs) |
| Nonce | 24 字节随机 OsRng / 单次 | `XChaCha20Poly1305::generate_nonce` | 同上 |
| 公开 roundtrip 边界 | `encrypt(plaintext) -> Vec<u8>` / `decrypt(ciphertext) -> Vec<u8>` | `crypto::seal/open` | 同上 |
| 临时缓冲 | zeroize 后丢弃 | `zeroize::Zeroizing<Vec<u8>>` | 同上 |
| 日志/诊断/备份 canary 命中数 = 0 | 5 处持久化介质扫描 | `sensitive_persistence_contract.rs::foundation_canary_*` 2 项 | `tests/sensitive_persistence_contract.rs` 7/7 Green |

### ①.6 阻断恢复界面

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| 缺失/损坏/权限错 UI | "重新配置 / 从备份恢复 / 退出" 三选项 | `KeyRecoveryView` | [key_recovery_view.rs](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/ui/key_recovery_view.rs) |
| 焦点陷阱 | 键盘可达 | `RecoveryTab` / `RecoveryTabPrev` actions | `ui/app.rs` / `ui/mod.rs` |
| 退出 → 应用关闭 | 唯一主 UI 不打开 | `HiveGuiAppState::assert_no_hiveweb_prerequisite` | [app.rs](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/ui/app.rs) |

### ①.7 启动门禁 FR-046

- 主 Store 不在设备密钥 `BlockingRecovery` 状态下打开（[store.rs::verify_sqlite_health](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/datasource/store.rs) + `key_store.rs::require_unlocked`）。
- 启动期 `PRAGMA integrity_check` / `foreign_key_check` 任一失败进入 `T023 IntegrityCorrupted` 分流（不进入本边界）。
- 任何 `Locked` → `Unlocked` 转换由 T-AUTH-5 边界控制（边界 ⑤），本边界不重复其逻辑。

### ①.8 已知非阻断工具链风险

- 同 §⑤.8 / `tasks.md` "已知非阻断工具链风险"：Rust 1.97.1 报 `proc-macro-error2 2.0.1` future-incompat；本边界不依赖其新 API，无额外暴露面。

### ①.9 待办 / 阻塞项

- **独立 security reviewer + 第二 maintainer 签字** — ✓ **已闭环**（2026-08-06，按 Constitution v1.5.0 *Single-developer repository clause* 由 user 同时承担两角色；self-attestation 见 §①.11）。

### ①.10 合并解锁范围

- 本签字解锁 **T025 合并门禁**：`device_key_lifecycle.rs` 8/8 Green + `auth_setup_red.rs` 7/7 Green + 公开 roundtrip 零明文 + doc 硬门槛 + 跨进程并发收敛 + 已有密文不静默覆盖/修改/破坏权限保护。
- 不替代 ⑤ 主密码认证（必须保持 `LockState` 由 T-AUTH-5 控制）。
- 不替代 ② sidecar cleanup（设备密钥文件不属于 SQLite sidecar，独立关闭）。
- 不替代 ③ Plugin sandbox（Plugin 加载不接触设备密钥明文；仅 `WrappedDeviceKey` 走公开 Store）。
- 不替代 ④ 备份 age 加密（备份 manifest exclude `.hivegui/keystore/`，由 T129/T130 闭合）。
- 不替代 ⑥ HiveGUI 远程 MySQL 公开边界。

### ①.11 Self-attestation（Constitution v1.5.0 *Single-developer repository clause*）

> 本节记录按 Constitution v1.5.0 §Security Requirements *Single-developer repository clause* (2026-07-30 增补) 进行的 self-attestation。它满足 "dedicated security review + second approver" 合并为同一 maintainer 时所需的 non-waivable 条件 ② 与 ③：流程必须完整运行、self-attestation 必须显式记录、且 PR 描述 / 审批账本必须给出"独立 security reviewer 与 second approver" 的双重视角。

- **Reviewer 独立视角检查**：
  1. 设备密钥生命周期（首次启动原子化、0600 ACL 等效、重启复用、并发首启收敛、缺失/损坏/权限错阻断恢复）— §①.1-§①.4 ✓
  2. 已有密文不得静默覆盖/修改/破坏权限保护 — §①.4 ✓
  3. 公开 crypto roundtrip 零明文落盘（5 处持久化介质 canary 命中数 = 0）— §①.5 ✓
  4. 阻断恢复界面（重新配置 / 从备份恢复 / 退出）— §①.6 ✓
  5. 启动门禁 FR-046（主 Store 不在 `BlockingRecovery` 状态下打开）— §①.7 ✓
- **测试命令与结果**：
  - `cargo test -p hivegui --test device_key_lifecycle` 退出 0，`test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s`。
  - `cargo test -p hivegui --test auth_setup_red` 退出 0，`test result: ok. 7 passed; 0 failed`（含 `accepted_password_must_not_touch_t025_device_key`）。
  - `cargo test -p hivegui --test auth_unlock_red` 退出 0，`test result: ok. 8 passed; 0 failed`（含 `wrong_passwords_must_not_modify_any_persistent_state`）。
  - `cargo test -p hivegui --test sensitive_persistence_contract` 退出 0，`test result: ok. 7 passed; 0 failed`（Foundation `device_key_canary` 行 2/2 + helper 5/5）。
- **Security-review 流程（dedicated）条件总结**: 通过。T025R 边界 ①.1-①.7 检查项已对照规格与实现逐条核对。无新增 finding；旧 finding "已有密文被静默覆盖" 已由 T016F/T025 Red 断言覆盖并通过。
- **Code-quality / doc 硬门槛**: `key_store.rs` / `crypto.rs` / `key_recovery_view.rs` 新增 `pub fn` 全部完成 doc comment；`#![warn(missing_docs)]` 编译 0 警告（其余模块遗留警告与本边界无关，且不阻断 doc 硬门槛）。
- **未豁免条款**: 本 self-attestation **未豁免** Constitution §Security Requirements 的 dedicated security review、设备密钥生命周期保证、AEAD 选型、零明文落盘、已有密文不可静默覆盖、secret scanning 或其他任何宪章条款。**仅**结构性要求"第二审批人必须是不同人"在单开发者仓库下被 *Single-developer repository clause* 替代。
- **重新激活条件**: 如未来新增 maintainer，"独立 security reviewer + 第二 maintainer 双签字" 立即恢复；本 self-attestation 不追溯作废，仅显式标注为 "single-developer repository clause"，未来 reviewer 可识别哪些签字在第二位 maintainer 加入前完成。
- **T025 合并解锁**: 本 self-attestation 与上面 7 条重新检查同时闭合后，T025 可解除 "T025R ① 签字前不得合并" 阻断，进入合并流程。

---

## T025R 边界 ② — SQLite sidecar cleanup 协议（T022）

**Reviewer**: **user（本仓库唯一 active maintainer，Constitution v1.5.0 §Security Requirements *Single-developer repository clause* 适用，2026-08-06）** — 同时承担 dedicated security review 与 second approver 角色；self-attestation 见 §②.11。
**Review date**: 2026-08-06（首次审 + 复验：`sqlite_health_contract` 9/9 Green + `store_resilience` 11/11 Green + 公开 sidecar cleanup 行为 + 合法 reason/artifact 优先级 + doc 硬门槛）。
**Scope**: `crates/hivegui/src/datasource/migrations.rs`（`sidecar_cleanup_journal` / `quarantine_identity` / `verify_sqlite_health` / `wal_checkpoint(TRUNCATE)`）+ `crates/hivegui/src/datasource/store.rs`（Open 路径 sidecar 重放） + `tests/sqlite_health_contract.rs`（9 项 sidecar 与 health 行为断言）。
**TDD 证据**: `sqlite_health_contract.rs` 9/9 Green（`legal_reason_and_artifact_priority_are_stable` + `struct_corruption_is_rejected_at_open_without_schema_apply` + `identity_bound_no_replace_quarantine` + `sqlite_zero_byte_is_rejected_with_same_kind` + `sqlite_valid_but_with_failed_integrity_check_is_rejected` + `frozen_write_marker_is_required_before_every_commit` + `write_without_committed_high_watermark_is_refused` + `sidecar_classification_is_canonical` + `orphan_foreign_key_is_rejected_at_open_without_schema_apply`）。`store_resilience.rs` 11/11 Green（1s/2s/4s 精确退避 + corruption 阻断 + 单实例写锁 + 缺库即创建 v4 + 不静默重建）。
**doc 硬门槛**: `migrations.rs` / `store.rs` / `sidecar_cleanup_journal` 涉及 `pub fn` 全部完成 `///` doc comment，编译无 `missing_docs` 警告（受 `#![warn(missing_docs)]` 保护）；本任务不接受按 T144 推迟。

### ②.1 Sidecar canonical 分类（hot / unknown / recoverable）

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| Hot WAL/SHM/rollback | 仍含未 checkpoint 帧或事务未提交 | `SidecarKind::Hot` + `wal_checkpoint_remaining() > 0` 探测 | [migrations.rs](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/datasource/migrations.rs) `sidecar_classification` |
| Unknown owner | 不匹配任何 instance manifest / identity | `SidecarKind::Unknown` + 不在 registry 中 | 同上 |
| Recoverable | 已 checkpoint 但仍有 frame | `SidecarKind::Recoverable` | 同上 |
| canonical 字节保留 | Hot/Unknown/Recoverable **永不**进入 cleanup journal | `sidecar_cleanup_journal::append` 拒绝上述 kind | `migrations.rs::sidecar_cleanup_journal` |
| 仅安全残留可记录 | 仅 `checkpoint_done` 且非 hot/unknown/recoverable 的 sidecar 可进入 journal | `journal::append(artifact)` 条件 | 同上 |

### ②.2 `prepared → quarantined → done` 五分支重放

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| `prepared` | 写入 journal + parent fsync，不动 live | `migrations.rs::sidecar_cleanup_journal::prepared` | 同上 |
| `quarantined` | identity-bound no-replace rename → fsync(quarantine) | `migrations.rs::quarantine_identity` | 同上 |
| `done` | unlink quarantine + fsync(quarantine parent) + 持久化 `done` + unlink journal + fsync(journal parent) | `migrations.rs::sidecar_cleanup_journal::done` | 同上 |
| 启动重放 5 分支 | (1) journal + final + staging 均无 → 跳过；(2) journal + final 无 + staging 有 → 孤立 staging cleanup；(3) journal + final 有 + `done` → 删 journal；(4) journal + final 有 + `prepared/quarantined` → identity-bound cleanup + 推进 `done`；(5) journal 损坏/重复 → fail-closed | `migrations.rs::replay_sidecar_cleanup_journal` | 同上 |
| `done` 收尾 | journal 删除 + parent fsync 后才能快照/发布/开放 Store | `store.rs::open_local` 在 `replay_sidecar_cleanup_journal` 完成后才进入 schema apply | `store.rs` |

### ②.3 Identity-bound no-replace quarantine

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| Token 派生 | `SHA-256("hivegui-sidecar-cleanup-v1" \0 db_id)`, 取 64 位小写 hex（前 32 hex 字符） | `migrations.rs::quarantine_token` | `sqlite_health_contract.rs::identity_bound_no_replace_quarantine` ✓ |
| basename 固定 | `.hivegui-sidecar-cleanup-v1-{token}-{artifact}.json` (含 `.staging`) | `migrations.rs::sidecar_basename` | 同上 |
| Rename 协议 | root-handle-relative + no-follow + atomic | `std::fs::rename` + 父目录 fsync + identity 复核 | 同上 |
| 同名追加不覆盖 | 旧 file 仍保留，新 quarantine 用 `{token}-{artifact}-{seq}` 派生 | `migrations.rs::quarantine_seq` | `sqlite_health_contract.rs::identity_bound_no_replace_quarantine` ✓ |

### ②.4 合法 `storage_recovery_blocked { reason, artifact }` 配对 + 固定总优先级

| reason | 合法 artifact 集合 | 固定总优先级（数字越小越先报） |
| --- | --- | --- |
| `checkpoint_failed` | wal | 1 |
| `checkpoint_busy` | wal | 2 |
| `connections_open` | wal | 3 |
| `sidecar_reappeared` | wal / rollback_journal / shm | 4 |
| `sidecar_hot` | wal / rollback_journal / shm | 5 |
| `sidecar_recoverable` | wal / rollback_journal / shm | 6 |
| `sidecar_unknown_owner` | wal / rollback_journal / shm | 7 |
| `sidecar_cleanup_failed` | wal / rollback_journal / shm | 8 |

证据：[migrations.rs::legal_reason_artifact_priority](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/datasource/migrations.rs) + `sqlite_health_contract.rs::legal_reason_and_artifact_priority_are_stable` ✓（sum stable + reason/artifact 矩阵）。

### ②.5 完整性双检查 `PRAGMA integrity_check` + `PRAGMA foreign_key_check`

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| `integrity_check == "ok"` | 任意打开 / 新建 / 迁移提交前 | `migrations.rs::verify_sqlite_health` | `sqlite_health_contract.rs::sqlite_valid_but_with_failed_integrity_check_is_rejected` ✓ |
| `foreign_key_check` 零行 | 任意打开 / 新建 / 迁移提交前 | 同上 | `sqlite_health_contract.rs::orphan_foreign_key_is_rejected_at_open_without_schema_apply` ✓ |
| 失败回滚/阻断 | 不得 `foreign_keys=ON` 替代 | `migrations.rs::open_local` 失败返回 `StoreErrorKind::StoreCorrupt` 或 `OrphanForeignKey`，无 schema apply，无 sidecar 新增 | 同上 |
| 失败不写 sidecar | 任何 `StoreCorrupt` / `OrphanForeignKey` 路径不增加 WAL/SHM/journal | 同上 | 同上 |

### ②.6 Frozen marker + `wal_checkpoint(TRUNCATE)`

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| Frozen marker | 仅写事务存在时落地，`.frozen` 标记当前已 commit WAL | `migrations.rs::write_frozen_marker` | `sqlite_health_contract.rs::frozen_write_marker_is_required_before_every_commit` ✓ |
| `wal_checkpoint(TRUNCATE)` | 非 busy 时完整并入主文件 | `migrations.rs::wal_checkpoint_truncate` | 同上 |
| 关闭全部连接 | 完整并入后 `Connection::close` + 证明全部已提交帧仍可读 | `migrations.rs::close_and_reopen_read` | 同上 |
| Committed high-watermark | 在 `CommittedHighWatermarkMissing` 时 fail-closed | `migrations.rs::committed_high_watermark_check` | `sqlite_health_contract.rs::write_without_committed_high_watermark_is_refused` ✓ |

### ②.7 Staging instance / manifest / locator

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| Staging instance 路径 | `.hivegui-db-staging-v1/{role}-{db_instance_operation_id}/datasources.db` | `migrations.rs::staging_instance_path` | `migrations.rs` |
| v1 instance manifest | 6 元组 `schema_version=1` + role + UUID + db_id + `database_name=datasources.db` + `ownership_state=unarmed` | `migrations.rs::write_instance_manifest_staging` | 同上 |
| 启动发现 | ASCII 字节序 no-follow 枚举 + 拒绝 link/特殊文件/缺失/损坏/重复 manifest/UUID 重复 | `migrations.rs::replay_instance_registry` | 同上 |
| 状态机 | 仅 `unarmed → armed` | `migrations.rs::arm_instance` | 同上 |
| 备份/新树排除 | registry / live / tombstone / manifest / owner / retirement / cleanup 不进入 archive/安全备份/待切换新树 | `migrations.rs::exclude_paths_for_archive` | 同上 |

### ②.8 Migration owner + retirement

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| Migration staging | `unarmed migration instance`，T022 唯一拥有 | `migrations.rs::staging_instance_path(role="migration")` | 同上 |
| v2→v3→v4 | 仅在 `unarmed` migration instance 执行 | `migrations.rs::migrate_v2_to_v3` / `migrate_v3_to_v4` | `migration_compatibility.rs` 9/9 Green |
| 事务完整性 | SQLite commit ≠ 系统 commit | `migrations.rs::publish_after_commit` | 同上 |
| Restore instance | T129 仅构建 `unarmed restore instance`，故意不 arm | 由 T129 实现（仍 Pending） | 范围外 |
| Retirement | 旧/new/aborted_pre_switch 三 outcome + `prepared/renamed/done` 状态机 | `migrations.rs::retirement_journal` | 同上 |
| Live 下 | 不得单独删除 manifest/owner；aborted/old/new 仅由 registry retirement 接管 | `migrations.rs::live_under_retirement` | 同上 |

### ②.9 Crash matrix

- 在 `prepared → quarantined → done` 任一阶段注入崩溃后重启，断言只丢弃活动段末尾不完整记录、旧段或压缩后新段至少一个完整可恢复、已到期记录不会因回拨复活。
- 任何 high-watermark 或容量操作失败时诊断读取/导出 fail-closed；诊断读取永不观察半写记录。
- canonical 新 identity 必须为 `sidecar_reappeared`；`done` 后 quarantine 重现、canonical/quarantine 同时存在、任一 identity 不匹配、journal 损坏/重复或状态无法证明必须为 `sidecar_unknown_owner`；只有 identity-bound cleanup、journal 删除或耐久化操作明确失败才为 `sidecar_cleanup_failed`。
- 全部候选按固定 reason/artifact 配对和总优先级选择，绝不盲删或与新主文件组合。
- journal 耐久删除前不会快照/发布、不丢失已提交帧、不组合 canonical 旧 sidecar 与新主文件。

### ②.10 已知非阻断工具链风险

- 同 §⑤.8 / `tasks.md` "已知非阻断工具链风险"：本边界在 Rust 1.97.1 上无 future-incompat 警告。

### ②.11 Self-attestation（Constitution v1.5.0 *Single-developer repository clause*）

> 本节记录按 Constitution v1.5.0 §Security Requirements *Single-developer repository clause* (2026-07-30 增补) 进行的 self-attestation。它满足 "dedicated security review + second approver" 合并为同一 maintainer 时所需的 non-waivable 条件 ② 与 ③：流程必须完整运行、self-attestation 必须显式记录、且 PR 描述 / 审批账本必须给出"独立 security reviewer 与 second approver" 的双重视角。

- **Reviewer 独立视角检查**：
  1. Sidecar canonical 分类与"hot/unknown/recoverable 永不进入 journal" — §②.1 ✓
  2. `prepared → quarantined → done` 五分支启动重放（含 `done` 收尾） — §②.2 ✓
  3. Identity-bound no-replace quarantine（精确 token / 精确 basename / 原子 rename） — §②.3 ✓
  4. 合法 reason/artifact 配对 + 固定总优先级 — §②.4 ✓
  5. `PRAGMA integrity_check` + `PRAGMA foreign_key_check` 双检查 + `foreign_keys=ON` 不得替代 — §②.5 ✓
  6. Frozen marker + `wal_checkpoint(TRUNCATE)` + 关闭全部连接 + 已提交帧可读 — §②.6 ✓
  7. Staging instance / manifest / locator 终态生命周期 / 备份新树排除 — §②.7 ✓
  8. Migration owner + retirement 状态机 + aborted/old/new 边界 — §②.8 ✓
  9. Crash matrix（sidecar_reappeared / unknown_owner / cleanup_failed 严格映射） — §②.9 ✓
- **测试命令与结果**：
  - `cargo test -p hivegui --test sqlite_health_contract` 退出 0，`test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.15s`。
  - `cargo test -p hivegui --test store_resilience` 退出 0，`test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.74s`（含 1s/2s/4s 精确退避 + corruption 阻断 + 单实例写锁）。
  - `cargo test -p hivegui --test migration_compatibility` 退出 0，`test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 6.90s`（1 ignored fixture regen，非产品 surface）。
  - `cargo test -p hivegui --test plugin_artifact_schema_contract` 退出 0，`test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.32s`。
- **Security-review 流程（dedicated）条件总结**: 通过。T025R 边界 ②.1-②.9 检查项已对照规格与实现逐条核对。无新增 finding；旧 finding "sidecar 盲删与新主文件组合" 已由 identity-bound no-replace rename + journal 五分支重放覆盖。
- **Code-quality / doc 硬门槛**: `migrations.rs` / `store.rs` / `sidecar_cleanup_journal` 涉及 `pub fn` 全部完成 doc comment；`#![warn(missing_docs)]` 编译 0 警告（其余模块遗留警告与本边界无关）。
- **未豁免条款**: 本 self-attestation **未豁免** Constitution §Security Requirements 的 dedicated security review、sidecar cleanup 协议保证、identity-bound quarantine 保证、reason/artifact 固定总优先级、canonical/quarantine 字节不变、绝不与新主文件组合，或其他任何宪章条款。**仅**结构性要求"第二审批人必须是不同人"在单开发者仓库下被 *Single-developer repository clause* 替代。
- **重新激活条件**: 如未来新增 maintainer，"独立 security reviewer + 第二 maintainer 双签字" 立即恢复；本 self-attestation 不追溯作废，仅显式标注为 "single-developer repository clause"，未来 reviewer 可识别哪些签字在第二位 maintainer 加入前完成。
- **T022 合并解锁**: 本 self-attestation 与上面 9 条重新检查同时闭合后，T022 可解除 "T025R ② 签字前不得合并" 阻断，进入合并流程。

---

## T025R 边界 ③ — Plugin sandbox（T073-T079 + T082）

**Reviewer**: **user（本仓库唯一 active maintainer，Constitution v1.5.0 §Security Requirements *Single-developer repository clause* 适用，2026-08-06 签字）** — 同时承担 dedicated security review 与 second approver 角色；self-attestation 见 §③.11。
**Review date**: 2026-08-06（首次审 + 复验：`.with_wasi(true) → .with_wasi(false)` 修复 + `plugin_sandbox_red` 7/7 Green + `plugin_artifact_schema_contract` 10/10 Green + `desktop_host_call` 6/6 Green + doc 硬门槛）。
**Scope**: `crates/hivegui/src/runtime/plugin_executor.rs`（Wasmtime/Extism 桥 + 资源限制 + 实例池 + `with_wasi(false)` 单点）+ `crates/hivegui/src/datasource/plugin_artifacts.rs`（no-replace / 不可变键 / 旧句柄 / 租约）+ `crates/hivegui/src/datasource/migrations.rs`（`plugin_artifact_operations` / `plugin_artifact_gc` ledger）+ `crates/hive-runtime-core/src/wasm.rs`（`WasmSandboxConfig::deny_all_wasi()`）+ 根 workspace `Cargo.toml`（`extism = { version = "=1.30.0", default-features = false }`）+ `tests/plugin_artifacts.rs` + `tests/plugin_compatibility.rs` + `tests/plugin_limits.rs` + `tests/plugin_artifact_schema_contract.rs` + `tests/desktop_host_call.rs` + `tests/plugin_sandbox_red.rs`。
**TDD 证据（2026-08-06，签字复验）**:
- `plugin_artifacts` 4/4 Green（`install_plugin_records_byte_stable_fingerprint` + `existing_plugin_cannot_be_replaced_silently` + `soft_delete_keeps_artifact_on_disk` + `plugin_lease_is_scoped_to_a_runtime_session`）
- `plugin_compatibility` 5/5 Green（`empty_artifact_is_rejected_before_persisting_any_state` + `invalid_identifier_or_version_is_rejected` + `no_replace_rejects_duplicate_identifier_and_version_silently` + `install_error_kinds_have_stable_string_codes` + `install_with_custom_root_creates_artifact_directory`）
- `plugin_limits` 6/6 Green（`default_limits_match_spec` + `hard_caps_match_spec` + `plugin_limits_reject_out_of_range_values` + `plugin_limits_accept_in_range_values` + `plugin_executor_constructs_with_default_limits` + `plugin_executor_pool_capacity_is_bounded`）
- `plugin_artifact_schema_contract` 10/10 Green（`runtime_store_has_no_plugin_ledger_ddl` + `operations_state_check_enforces_identity_preconditions` + `v3_to_v4_backfills_row_revision_and_creates_ledger_transactionally` 等 ledger 所有权约束）
- `desktop_host_call` 6/6 Green（`plugin_importing_host_call_can_be_built_by_the_desktop_executor` + `host_call_uses_standard_permission_and_unknown_capability_errors` + `network_http_capability_forwards_the_request_and_response` 等 lockdown 后合法性 + capability 拒绝码 4030/4045）
- **`plugin_sandbox_red` 7/7 Green**（`plugin_importing_wasi_fd_write_is_rejected` + `plugin_importing_wasi_path_open_is_rejected` + `plugin_importing_wasi_proc_exit_is_rejected` + `plugin_executor_disables_wasi_in_source` + `plugin_executor_keeps_only_host_call_as_a_host_import` + `hivegui_cargo_manifest_keeps_extism_auto_registration_disabled` + `plugin_with_only_host_call_can_still_be_built_after_wasi_lockdown`）— T025R ③ 重新激活硬门禁
- 累计 **38/38 Green**（相对 2026-07-31 首次 15/15 Green，扩 23/23 = `plugin_artifact_schema_contract` 10 + `desktop_host_call` 6 + `plugin_sandbox_red` 7）
- T018 端 `wasm.rs` `WasmSandboxConfig::deny_all_wasi()` 与 `WasmExecutionFailure` 6 类已就位
**doc 硬门槛**: plugin_executor / plugin_artifacts / wasm 新增 `pub fn` 全部完成 `///` doc comment；`#![warn(missing_docs)]` 编译 0 警告（其余模块遗留警告与本边界无关）。

### ③.1 资源限制（timeout / memory / output）

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| Timeout | `[1, 120]` 秒，Extism `.with_timeout(Duration)` + 外部 `tokio::time::timeout` 双层 | `plugin_executor.rs::execute_with_timeout` + `DEFAULT_TIMEOUT_SECS=30` + `HARD_MAX_TIMEOUT_SECS=120` | [plugin_executor.rs](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/runtime/plugin_executor.rs) |
| Memory | `[1, 512]` MiB（per-instance） | `DEFAULT_MEMORY_MB=128` + `HARD_MAX_MEMORY_MB=512` + `PluginLimits::new` 严格范围校验 | 同上 |
| Output | `[1, 50]` MiB（per-call） | `DEFAULT_OUTPUT_BYTES=10 MiB` + `HARD_MAX_OUTPUT_BYTES=50 MiB` | 同上 |
| 越界 | 任一字段越界即 `PluginLimitError`（3 变体）+ 拒绝构造 | `PluginLimits::new` 三段独立 `if !(1..=HARD).contains(&x)` | `plugin_limits` 6/6 Green |

### ③.2 实例池（cache key 完整性 + LRU 容量）

| 项 | 规格 | 实现 | 证据 |
| --- | --- | --- | --- |
| 全局池容量 | ≤ 8 个空闲实例 | `POOL_CAPACITY: usize = 8` | `plugin_executor.rs::POOL_CAPACITY` |
| 每 cache-key LRU | 最多 1 个 | `pool_capacity` + `BTreeMap<cache_key, ...>` LRU 维护 | 同上 + `plugin_limits::plugin_executor_pool_capacity_is_bounded` Green |
| 缓存键 | SHA-256(artifact_bytes) + ABI version + runtime version + fuel + max_memory + cache key 完整性 | 注释 + 单元测试 | `plugin_executor.rs:147-184` `PluginExecutor` 文档 + `plugin_artifacts::install_plugin_records_byte_stable_fingerprint` Green |

### ③.3 No-replace / 不可变键 / 旧句柄 / 租约（与 T022 ledger 联动）

- `plugin_artifact_operations` 与 `plugin_artifact_gc` 由 T022 `migrations.rs` 一次性创建（DDL 单一 owner），运行时 Store 不得继续散落 DDL —— `plugin_artifact_schema_contract::runtime_store_has_no_plugin_ledger_ddl` Green
- `operation_id` → `staging_name` 派生 + UNIQUE 约束；`operations` 状态 CHECK 锁定 5 类（`prepared|staged|published|referenced|done|conflict`）；identity precondition（`staged` 仅 staging identity 非空 / `published|referenced` 两项 identity 均非空 / `done` 满足 `new_identity IS NULL OR staging_identity IS NOT NULL`） —— `operations_state_check_enforces_identity_preconditions` Green
- 软删除保留 artifact bytes（`deleted_at` 字段不删文件），新建 `(identifier, version)` 唯一约束拒绝静默覆盖 —— `soft_delete_keeps_artifact_on_disk` + `no_replace_rejects_duplicate_identifier_and_version_silently` Green
- 租约 `plugin_lease(plugin_id, session_id)` 限定到 `session_id` —— `plugin_lease_is_scoped_to_a_runtime_session` Green

### ③.4 ABI / 校验（与 T018 联动）

- `WasmSandboxConfig::deny_all_wasi()` + `WasmModuleShape` 6 类 + `WasmValidationError` 完整覆盖 —— T018 6/6 Green
- `StableErrorKind` 11 变体 + `WasmExecutionFailure::stable_error_kind()` 一一对应 —— 同上
- 不注册 Extism HTTP / filesystem —— HiveGUI 端 `extism = { version = "=1.30.0", default-features = false }`（T007 A1 决策）阻断；feature 树 `cargo tree -p hivegui` 不含 `ureq` / Extism `http` / `register-http` / `register-filesystem`

### ③.5 Root-handle-relative no-follow

- 当前 `PluginExecutor::execute_with_capabilities` 接受 `wasm_path: &Path` 直接读取字节（`tokio::fs::read(wasm_path)`）并送入 Extism；未在 `runtime/plugin_executor.rs` 内对 `wasm_path` 自身做 root-handle 解析或 no-follow 验证。该路径在生产 UI 中由 `plugin_view` / `plugin_artifacts` 提供受控根（`PluginArtifactStore::artifact_path()` 在 `plugin_artifacts.rs` 内构造并 `fs::canonicalize` + 校验不越界），调用方契约保证不接受外部路径。**这是当前已知的契约外延风险**：若未来引入外部 `wasm_path` 入口，必须在 T082 收尾前补 root-handle no-follow + symlink/junction/reparse 拒绝单元测试，否则此条目不得计入 T025R ③ 重新激活范围。

### ③.6 已知未闭合项（Pending finding）

- ~~`plugin_executor.rs:245` 当前仍调用 `.with_wasi(true)`，与本边界规格 "WASI off" 直接冲突。`tasks.md` Phase 2 注释明确："HiveGUI 源码中的 `.with_wasi(true)` 是另一项已知规格冲突，依照 strict TDD 留给 Plugin sandbox Red→Green 阶段处理，不在 T007 依赖批次偷改生产行为"。~~ ✅ **已修复（2026-08-06）**：
  1. ✅ 编写 `crates/hivegui/tests/plugin_sandbox_red.rs`（7 项断言：`plugin_importing_wasi_fd_write_is_rejected` + `plugin_importing_wasi_path_open_is_rejected` + `plugin_importing_wasi_proc_exit_is_rejected` + `plugin_executor_disables_wasi_in_source` + `plugin_executor_keeps_only_host_call_as_a_host_import` + `hivegui_cargo_manifest_keeps_extism_auto_registration_disabled` + `plugin_with_only_host_call_can_still_be_built_after_wasi_lockdown`）实际观察到 Red → Green 闭环。
  2. ✅ `plugin_executor.rs:249` 改为 `.with_wasi(false)` + 显式 `with_function("host_call", ...)` 注册，并加入 T025R ③ 注释说明。
  3. ✅ 复跑 `plugin_artifacts` 4/4 + `plugin_compatibility` 5/5 + `plugin_limits` 6/6 + `plugin_artifact_schema_contract` 10/10 + `desktop_host_call` 6/6 + 新增 `plugin_sandbox_red` 7/7 = 38/38 Green。
  4. ✅ T076/T082 self-attest（本节 §③.11）显式列出"`.with_wasi(true) → .with_wasi(false)` + WASI import 拒绝断言"作为 Green 范围扩展。
  5. ✅ 重跑 T025R ③ 重新激活，状态从 Pending 改为 Signed（§③.8）。

### ③.7 已知非阻断工具链风险

- 同 §⑤.8 / `tasks.md` "已知非阻断工具链风险"：本边界在 Rust 1.97.1 上无 future-incompat 警告。

### ③.8 状态

- **Signed**（2026-08-06，user，按 Constitution v1.5.0 *Single-developer repository clause*）：T073-T079 + T082 故事层实现 + `.with_wasi(true) → .with_wasi(false)` 修改 + 公开 `plugin_sandbox_red.rs` Red 编写并实际观察 Red → Green + 38/38 Green 全复跑后，本边界签字。`self-attestation` 见 §③.11。
- **Green 范围扩展（2026-08-06 相对 2026-07-31 15/15 Green）**：`+plugin_sandbox_red` 7/7 = WASI import 拒绝三例（`fd_write` / `path_open` / `proc_exit`）+ 源码层 `with_wasi(false)` 单点 + 源码层仅 `host_call` 一个 host import + workspace `Cargo.toml` 保持 `extism = { default-features = false, ... }` 不开 `http` / `register-http` / `register-filesystem` + lockdown 后合法 `host_call` 仍可构建。
- T138 跨介质汇总不受本 partial closure 影响。

### ③.9 合并解锁范围

- 本签字解锁 **T073-T079 + T082 合并门禁**：`plugin_artifacts` 4/4 + `plugin_compatibility` 5/5 + `plugin_limits` 6/6 + `plugin_artifact_schema_contract` 10/10 + `desktop_host_call` 6/6 + `plugin_sandbox_red` 7/7 = 38/38 Green + `with_wasi(false)` 修复 + WASI import 三例拒绝 + 仅 `host_call` 单一 host import + Extism `default-features = false` 阻断 `http` / `register-http` / `register-filesystem`。
- 不替代 ① 设备密钥（Plugin 加载不接触设备密钥明文；仅 `WrappedDeviceKey` 走公开 Store）。
- 不替代 ② sidecar cleanup（Plugin 制品 store 与 SQLite sidecar 互不重叠）。
- 不替代 ④ 备份 age 加密（备份 manifest exclude plugin artifacts，由 T129/T130 闭合）。
- 不替代 ⑤ 主密码认证（Plugin 执行调用必须经过 `LockState` 由 T-AUTH-5 控制）。
- 不替代 ⑥ HiveGUI 远程 MySQL 公开边界。

### ③.11 Self-attestation（Constitution v1.5.0 *Single-developer repository clause*）

> 本节记录按 Constitution v1.5.0 §Security Requirements *Single-developer repository clause* (2026-07-30 增补) 进行的 self-attestation。它满足 "dedicated security review + second approver" 合并为同一 maintainer 时所需的 non-waivable 条件 ② 与 ③：流程必须完整运行、self-attestation 必须显式记录、且 PR 描述 / 审批账本必须给出"独立 security reviewer 与 second approver" 的双重视角。

- **Reviewer 独立视角检查**：
  1. 资源限制（timeout / memory / output）— §③.1 ✓
  2. 实例池（cache key 完整性 + 全局 ≤8 / 每 key ≤1 LRU）— §③.2 ✓
  3. No-replace / 不可变键 / 旧句柄 / 租约 + T022 ledger 联动 — §③.3 ✓
  4. ABI / 校验（与 T018 联动）+ StableErrorKind 11 变体 — §③.4 ✓
  5. Root-handle-relative no-follow（已知契约外延风险，UI 受控根兜底；条目本任务不重写）— §③.5 ✓
  6. **WASI off + WASI import 拒绝**（`plugin_sandbox_red` Red→Green + `.with_wasi(false)` 修复 + `extism` `default-features = false` 不开 `http` / `register-http` / `register-filesystem`）— §③.6 ✓
  7. 已知非阻断工具链风险（无 future-incompat 警告）— §③.7 ✓
- **测试命令与结果**：
  - `cargo test -p hivegui --test plugin_sandbox_red` 退出 0，`test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s`：3 个 WASI 拒绝 + 2 个源码层 + 1 个 Cargo 清单 + 1 个 lockdown 后合法 `host_call` 仍可构建。
  - `cargo test -p hivegui --test plugin_artifacts` 退出 0，`test result: ok. 4 passed; 0 failed`：no-replace 旧句柄 / 租约 / 字节稳定 / 不可变键。
  - `cargo test -p hivegui --test plugin_compatibility` 退出 0，`test result: ok. 5 passed; 0 failed`：T072 等价 envelope。
  - `cargo test -p hivegui --test plugin_limits` 退出 0，`test result: ok. 6 passed; 0 failed`：默认 30s/128MiB/10MiB + 硬上限 120s/512MiB/50MiB。
  - `cargo test -p hivegui --test plugin_artifact_schema_contract` 退出 0，`test result: ok. 10 passed; 0 failed`：v4 ledger + state check + identity precondition + DDL 唯一 owner。
  - `cargo test -p hivegui --test desktop_host_call` 退出 0，`test result: ok. 6 passed; 0 failed`：lockdown 后合法 `host_call` 仍可构建 + capability 拒绝码 4030/4045 + `network.http` 转发。
- **Security-review 流程（dedicated）条件总结**: 通过。T025R 边界 ③.1-③.8 检查项已对照规格与实现逐条核对。新增 7 项 Green（含 3 项 WASI import 拒绝回归 + 1 项 lockdown 后合法性 + 3 项源码/Cargo 静态闸门），把本边界从 2026-07-31 的 15/15 Green 扩到 2026-08-06 的 38/38 Green。无新增 finding；唯一 Pending finding `.with_wasi(true) → .with_wasi(false)` 已在 §③.6 全 5 步闭环。
- **Code-quality / doc 硬门槛**: `plugin_executor.rs` 新增 `pub fn` 全部完成 doc comment；`#![warn(missing_docs)]` 编译 0 警告（其余模块遗留警告与本边界无关，且不阻断 doc 硬门槛）。
- **未豁免条款**: 本 self-attestation **未豁免** Constitution §Security Requirements 的 dedicated security review、WASI off 保证、仅 `host_call` 单一 host import 约束、Extism `default-features = false` 阻断 `http` / `register-http` / `register-filesystem`、resource limit 严格范围校验、cache key 完整性、no-replace / 不可变键 / 旧句柄 / 租约约束、secret scanning 或其他任何宪章条款。**仅**结构性要求"第二审批人必须是不同人"在单开发者仓库下被 *Single-developer repository clause* 替代。
- **重新激活条件**: 如未来新增 maintainer，"独立 security reviewer + 第二 maintainer 双签字" 立即恢复；本 self-attestation 不追溯作废，仅显式标注为 "single-developer repository clause"，未来 reviewer 可识别哪些签字在第二位 maintainer 加入前完成。
- **T073-T079 + T082 合并解锁**: 本 self-attestation 与上面 8 条重新检查同时闭合后，T073-T079 + T082 五个被审实现任务可解除 "T025R ③ 签字前不得合并" 阻断，进入合并流程。T138 跨介质汇总须待 US8 全部 story-owned canary 行由各 story reviewer 激活后再汇总。

---

## T025R 边界 ④ — FR-026 备份 age 加密（T119 + T129/T130 + T123）

**Reviewer**: **user（本仓库唯一 active maintainer，Constitution v1.5.0 §Security Requirements *Single-developer repository clause* 适用，2026-08-06）** — 同时承担 dedicated security review 与 second approver 角色；self-attestation 见 §④.11。
**Review date**: 2026-08-06（首次审 + 复验：`backup_restore` 6/6 Green + age 流认证 + 6 元组 manifest + staging 隔离 + 路径拒绝清单 + doc 硬门槛）。
**Scope**: `crates/hivegui/src/datasource/backup.rs`（`BackupExporter` / `BackupImporter` / `BackupManifest` / `ExportError` / `ImportError`）+ `crates/hivegui/Cargo.toml`（`age = "=0.12.1"` + `tar = "=0.4.46"` + `flate2` + `secrecy`）+ `tests/backup_restore.rs`（6 项边界断言）+ `tests/sensitive_persistence_contract.rs`（跨介质 canary）。
**TDD 证据（2026-08-06）**:
- `cargo test -p hivegui --test backup_restore` 退出 0，`test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`：6 项边界
  1. `export_refuses_overwrite_of_existing_target`（no-replace rename）
  2. `export_refuses_symlinked_source`（symlink 拒绝）
  3. `manifest_carries_six_tuple_unarmed_default`（6 元组 `schema_version=1/role/UUID/restore_db_id/database_name=datasources.db/ownership_state=unarmed`）
  4. `import_with_wrong_passphrase_is_rejected`（age 流认证失败）
  5. `export_then_import_round_trip_recovers_seed_database`（passphrase 匹配的 round-trip）
  6. `format_2_archive_is_accepted_and_normalised`（format 1/2/3 升级到 format 1）
- `cargo test -p hivegui --test sensitive_persistence_contract` 退出 0，Foundation 7/7 Green（`backup_*` canary 跨 SQLite 主/WAL/SHM/journal/临时目录/脱敏错误 0 命中）
- 跨设备重加密由 `import_blocking` 写到 `<final_target>/datasources.db`（独立 staging 树，**不**直接覆盖旧主文件；T129 收尾时由 `T129::commit_after_owner` 二次写闸门兜底，本任务仅闭合"恢复到 staging 路径 + 完整字节保留"边界）

**doc 硬门槛**: `backup.rs` 新增 `pub fn` 全部完成 `///` doc comment；`#![warn(missing_docs)]` 编译 0 警告。

### ④.1 口令认证流（age passphrase identity）

- `Encryptor::with_user_passphrase(SecretString)` + `age::scrypt::Identity::new(SecretString)` —— passphrase 始终位于 `secrecy::SecretString` 零化包装内，**不得** 进入 argv / env / log / 诊断包 / 备份 manifest
- 错误口令立即返回 `ImportError::AuthenticationFailed`，无任何明文回显或 fallback
- 证据：[backup.rs](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/datasource/backup.rs) `decrypt_age_streaming` + `import_with_wrong_passphrase_is_rejected` Green

### ④.2 敏感值仅在有界内存中转换

- 加密/解密全程走 `Vec<u8>` in-memory buffer；数据库 bytes 不落任何中间明文文件（`export_blocking` 直接 `Vec::extend_from_slice` → 加密 → `OpenOptions::create_new(true)` 写目标）
- passphrase 走 `SecretString` 包装；导出目标 atomic create_new + parent fsync
- 诊断包 / 日志 / 备份 manifest 不得包含 passphrase / 数据库明文（`Sanitize::central_sanitizer` + T027 logging 持久边界共同保证）

### ④.3 跨设备立即重新加密

- 导入侧 `import_blocking` 把解密后 bytes 立即写入 `final_target/datasources.db`，**不** 复用加密形式（避免跨设备密文搬运）；T129 收尾由 `commit_after_owner` 二次闸门负责写盘前的 health check
- 跨设备搬运在 spec 层级始终是"解密 → staging → 目标路径原子重命名"；T129 备份恢复 + US13 全套 canary 共同覆盖

### ④.4 staging 数据库隔离

- 导入侧固定 staging 根：`<staging_root>/.hivegui-db-staging-v1/restore-<UUID>/datasources.db`
- import 期间 `manifest.json` 仅写入 staging 树；外部 caller 持有 `final_database` 路径后才执行 `fs::rename(staging → final_target)`
- staging 树不在 T119/T129/T130 主线 write-ahead 路径上（`plugin_artifact_operations.staging_name` 与本边界互不重叠）

### ④.5 归档路径拒绝清单（symlink / hardlink / junction / reparse / device / FIFO / socket / 未知 header）

- **导出侧** `export_blocking`：
  - `fs::symlink_metadata(source)` + `!file_type.is_file() || file_type.is_symlink() → UnsafeSource`
  - `metadata.len() == 0 → UnsafeSource`
  - `#[cfg(unix)]` `metadata.nlink() > 1 → UnsafeSource`（hardlink 拒绝）
  - 设备 / FIFO / socket 由 `is_file() = false` 覆盖
- **导入侧** `decompress_and_parse`：
  - `entry_type.is_symlink() || entry_type.is_hard_link() → UnsafeArchiveEntry`
  - `!entry_type.is_file() → UnsafeArchiveEntry`（覆盖 char/block device / FIFO / socket / 未知 header）
  - 入口 `tar::Archive::entries()` 逐 entry 校验，**禁用** `Archive::unpack` 或 `Entry::unpack*`（源码 contract 强制 + `tar = 0.4.46` `default-features = false` 不开 xattr）
- 证据：[backup.rs](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/datasource/backup.rs) + `export_refuses_symlinked_source` Green

### ④.6 No-replace / 原子 rename / 父目录 fsync

- 导出：`OpenOptions::new().create_new(true)` + `sync_all()` + `parent.sync_all()`
- 导入：`fs::rename(staging → final_target)` + `parent.sync_all()`；`final_database.exists()` 显式拒绝
- 错误口令、截断、后段篡改、staging 失败、安全备份未生成 → `final_target` 不变，导入返回 `ImportError`

### ④.7 Manifest / 格式升级

- 6 元组 `(schema_version=1, role="primary", UUID, restore_db_id, database_name="datasources.db", ownership_state="unarmed")` —— `manifest_carries_six_tuple_unarmed_default` Green
- `ALLOWED_FORMATS: &[u32] = &[1, 2, 3]` + `upgrade_to_format1(manifest)` 把 format 2/3 透明升级为 format 1 —— `format_2_archive_is_accepted_and_normalised` Green
- `manifest.database_name != "datasources.db"` 拒绝（防止 manifest 路径逃逸）

### ④.8 Path containment（不依赖 canonicalize 后重开）

- 导入侧无 `Entry::unpack` / `Entry::unpack_in`（源码 contract 强制 + import 走 `entry.read_to_end(&mut buf)` 逐 entry 读取）
- 拒绝绝对路径 / 空组件 / `.` / `..` / NUL / 重复路径（`header.path()` 解析失败 → `ImportError::Io`）
- 入口路径白名单只接受 `MANIFEST_FILENAME` 与 `DATABASE_FILENAME` 两个精确字符串（`decompress_and_parse` 中的 `if path_str == "manifest.json" { ... } else if path_str == "datasources.db" { ... }`），其他全部丢弃

### ④.9 已知非阻断工具链风险

- 同 §⑤.8 / `tasks.md` "已知非阻断工具链风险"：本边界在 Rust 1.97.1 上无 future-incompat 警告。
- `age = 0.12.1` `default-features = false`（不启用 `plugin` / SSH / pinentry），规避 RUSTSEC-2024-0433 中外部 age plugin 执行触发面
- `tar = 0.4.46` `default-features = false`（不启用 xattr），规避 PAX desync 类问题；RUSTSEC-2026-0067/0068 修复线已满足

### ④.10 测试命令与结果

- `cargo test -p hivegui --test backup_restore` 退出 0，`test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.12s`。
- `cargo test -p hivegui --test sensitive_persistence_contract` 退出 0，`test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s`（Foundation 7/7，含 backup_* 跨介质 canary）。
- `cargo doc --no-deps -p hivegui` 0 警告（`backup.rs::#![warn(missing_docs)]` 已开启）。

### ④.11 Self-attestation（Constitution v1.5.0 *Single-developer repository clause*）

> 本节记录按 Constitution v1.5.0 §Security Requirements *Single-developer repository clause* (2026-07-30 增补) 进行的 self-attestation。它满足 "dedicated security review + second approver" 合并为同一 maintainer 时所需的 non-waivable 条件 ② 与 ③：流程必须完整运行、self-attestation 必须显式记录、且 PR 描述 / 审批账本必须给出"独立 security reviewer 与 second approver" 的双重视角。

- **Reviewer 独立视角检查**：
  1. 口令认证流（age passphrase identity + 错误口令无 fallback） — §④.1 ✓
  2. 敏感值仅在有界内存中转换（无中间明文落盘 + `SecretString`） — §④.2 ✓
  3. 跨设备立即重新加密（解密 → staging → 目标原子 rename） — §④.3 ✓
  4. Staging 数据库隔离（`.hivegui-db-staging-v1/restore-{UUID}/`） — §④.4 ✓
  5. 归档路径拒绝清单（symlink / hardlink / device / FIFO / socket / 未知 header + nlink>1） — §④.5 ✓
  6. No-replace / 原子 rename / 父目录 fsync — §④.6 ✓
  7. 6 元组 manifest + format 1/2/3 升级 + `database_name` 严格校验 — §④.7 ✓
  8. Path containment（仅接受 `manifest.json` / `datasources.db` 两个精确 entry，禁用 `Archive::unpack`） — §④.8 ✓
  9. 已知非阻断工具链风险（age 0.12.1 / tar 0.4.46 / Rust 1.97.1 无 future-incompat 警告） — §④.9 ✓
  10. 测试命令与结果 — §④.10 ✓
- **Security-review 流程（dedicated）条件总结**: 通过。T025R 边界 ④.1-④.9 检查项已对照规格与实现逐条核对。无新增 finding；age passphrase 旁路 / 跨设备密文搬运 / symlink 入口 / 父目录 fsync 缺失 / manifest 路径逃逸 5 类常见攻击面均被现有实现阻断。
- **Code-quality / doc 硬门槛**: `backup.rs` 涉及 `pub fn` 全部完成 doc comment；`#![warn(missing_docs)]` 编译 0 警告（其余模块遗留警告与本边界无关）。
- **未豁免条款**: 本 self-attestation **未豁免** Constitution §Security Requirements 的 dedicated security review、age passphrase 旁路阻断、跨设备重加密保证、staging 隔离保证、归档路径拒绝清单保证、no-replace rename 保证、parent fsync 保证，或其他任何宪章条款。**仅**结构性要求"第二审批人必须是不同人"在单开发者仓库下被 *Single-developer repository clause* 替代。
- **重新激活条件**: 如未来新增 maintainer，"独立 security reviewer + 第二 maintainer 双签字" 立即恢复；本 self-attestation 不追溯作废，仅显式标注为 "single-developer repository clause"，未来 reviewer 可识别哪些签字在第二位 maintainer 加入前完成。
- **T119 + T129/T130 合并解锁**: 本 self-attestation 与上面 10 条重新检查同时闭合后，T119 / T129 / T130 三个被审实现任务可解除 "T025R ④ 签字前不得合并" 阻断，进入合并流程。T123 故事层 Green 复跑不受本签字影响，但 T138 跨介质汇总须待 US13 全部 story-owned canary 行由各 story reviewer 激活后再汇总。

---

## T025R 边界 ⑥ — HiveGUI 远程 MySQL 公开边界 FR-048（T034-T040 + T048）

**Reviewer**: **user（本仓库唯一 active maintainer，Constitution v1.5.0 §Security Requirements *Single-developer repository clause* 适用，2026-08-06）** — 同时承担 dedicated security review 与 second approver 角色；self-attestation 见 §⑥.11。
**Review date**: 2026-08-06（首次审 + 复验：`datasource_connection` 5/5 Green + `datasource_store` 8/8 Green + `datasource_ui_contract` 12/12 Green + 公开 Store 校验 + 单一 `MysqlIdentifier` 类型 + 跨介质 canary 0 命中 + doc 硬门槛）。
**Scope**: `crates/hivegui/src/datasource/mysql_client.rs`（`MysqlClient` / `MysqlMetadata` / `MysqlIdentifier` / `IdentifierCatalog` / `IdentifierContext` / `MysqlConnectionError`）+ `crates/hivegui/src/datasource/data_source_store.rs`（`DataSourceStore` 公开 Store 校验边界）+ `crates/hivegui/src/datasource/crypto.rs`（`DataSourcePassword` ChaCha20Poly1305 密文）+ `tests/datasource_connection.rs`（5 项 Red 边界断言）+ `tests/datasource_store.rs`（8 项 store 断言）+ `tests/sensitive_persistence_contract.rs`（Foundation `DataSourcePassword` 跨介质 canary）。
**TDD 证据（2026-08-06）**:
- `cargo test -p hivegui --test datasource_connection` 退出 0（独立 HiveGUI CI job 提供 MySQL 8.0+ service container 与专用 `HIVEGUI_TEST_MYSQL_URL`，动态创建/销毁隔离 schema + 账号）
- `cargo test -p hivegui --test datasource_store` 退出 0：`create_persists_record_with_encrypted_password` + `duplicate_name_returns_conflict_with_reason_name` + `empty_password_on_update_keeps_existing_ciphertext` + `restart_recovery_preserves_records_and_ciphertexts` + `list_and_search_use_indexed_plans_only`（`USING INDEX data_sources_name_idx`）+ `one_hundred_crud_operations_p95_under_one_second` + **`encrypted_password_canary_leaves_zero_residue_across_all_mediums`**（T016F `DataSourcePassword` 跨 SQLite 主/WAL/SHM/journal/临时目录/脱敏错误 全介质 0 命中）+ `sanitized_error_does_not_leak_canary_plaintext`
- `cargo test -p hivegui --test datasource_ui_contract` 退出 0，12/12 Green（含 theme + `Input::new` 可编辑 input + T016E scroll tag + 键盘焦点 + 错误摘要焦点 + 20 条/页 + 6×40ms tab 总耗时 < 800ms + 45 条分页 + `DataSourceViewMode` enum + 禁止 `WindowHandle<Root>` + 禁止 `forbid(dead_code)`）
- `cargo test -p hivegui --test sensitive_persistence_contract` 退出 0，Foundation 7/7 Green（含 `DataSourcePassword` 跨介质 canary）

**doc 硬门槛**: `mysql_client.rs` / `data_source_store.rs` / `crypto.rs` 新增 `pub fn` 全部完成 `///` doc comment；`#![warn(missing_docs)]` 编译 0 警告。

### ⑥.1 公开 Store 校验（`invalid_input` / `conflict` envelope）

- `DataSourceStore::create` / `update` / `delete` 走 `datasource::validation::FieldCatalog` 通用 `validate` 路径
- 普通失败返回 `InvalidInput { field, reason }`（无 value、无 SQL 错误）
- SQLite UNIQUE 映射到 `Conflict { field, value }` 仅含安全值（不暴露 SQL 错误）
- 引用或状态冲突 `Conflict { field, reason, references }`（无 value）
- 密码 / token 字段仅返回字段名 + 脱敏 reason；UI 不得作为唯一校验层（由 `datasource_view` 双层 `validate_and_submit` 单通道保证）

### ⑥.2 `MysqlIdentifier` 单一 source of truth

- 唯一公开类型 `MysqlIdentifier { kind, name, ctx }`，由 `MysqlMetadata::from_server_metadata` + `IdentifierCatalog::database/table/column` 三个工厂方法构造
- 任意外部 `&'static str` / `format!` / `escape` / 原始 `where_clause` / `order_by` / `conn.query(&` 路径被源码 contract 显式拒绝（`datasource_connection.rs` 集成）
- 序列化目标由 `IdentifierContext { Kind, Database, Table, Column }` 强类型区分；`MysqlIdentifier::to_sql` 拒绝跨上下文混用（`MysqlIdentifier must be rendered in the same context it was minted for`）

### ⑥.3 Metadata allowlist（精确 match）

- `IdentifierCatalog` 严格精确匹配 server 预加载 `MysqlMetadata`（database / table / column 三层白名单）
- 任何反引号 / SQL 注释 / 控制字符 / 大小写差异 / 点号 / DROP 注入 / 未知 / 恶意输入 → 拒绝（`datasource_connection.rs` 5 项 Red 全部覆盖）
- 不允许从外部输入构造 `MysqlIdentifier`（必须经 `MysqlMetadata::from_server_metadata` 预加载）

### ⑥.4 跨设备重放 + HiveWeb URL 0 命中

- `MysqlClient::test_connection` + `MysqlClient::query_metadata` + `MysqlClient::fetch_*` 全部走 `mysql_async` prepared values，identifier 经 `MysqlIdentifier` 单一类型化序列化
- `crates/hivegui/Cargo.toml` 不含 `hiveweb` 依赖（`hiveweb_independence::hivegui_manifest_has_no_hiveweb_dependency` Green）
- `CapturedHttpServer` 集成测试 0 命中（`datasource_connection::no_hiveweb_url_appears_in_test_connection`）
- 错误脱敏：timeout / 不可达 / 错误凭据 / 取消统一返回脱敏 envelope，无明文 host / port / database_name 泄漏

### ⑥.5 唯一 prepared statement 边界

- 所有 `MysqlClient` 查询走 `mysql_async::Queryable::query` + 预编译 + bind values
- 无 blanket `AssertSqlSafe` 或 raw 用户派生 SQL（生产 `QueryBuilder` 调用数 0，T028 inventory 强制）
- identifier 不得 bind（DBI 不支持），唯一允许路径 = `MysqlIdentifier` 单一类型化序列化（源码 contract 强制）

### ⑥.6 5 秒预算 + 可控时钟 + 取消

- `MysqlClient::test_connection` 5 秒总预算，`tokio::select!` 100ms 可放弃
- RFC 5737 不可达强制超时（`192.0.2.0/24` / `198.51.100.0/24` / `203.0.113.0/24` 测试源）
- `CancellationToken` 短路长任务（与 T019 `ExecutionContext::cancellation_token` 同源）

### ⑥.7 加密 canary 跨介质 0 命中

- `DataSourcePassword` 唯一明文 canary（process-unique UUID）写入 → 立即扫描 SQLite 主 / WAL / SHM / journal / 临时目录 / 脱敏错误 / 备份 staging / 备份最终密文包 / 普通临时目录 / 诊断包 11 介质 → 0 命中
- 加密算法 `chacha20poly1305::XChaCha20Poly1305`（RFC 8439）+ 设备密钥 32B OsRng（与 T025 设备密钥生命周期同源）
- 重新生成时 `DataSourcePassword::new` 强制设备密钥可用，缺设备密钥时 Store 返回 `BlockingRecovery`（与 T025 边界一致）

### ⑥.8 仓库 `.env` 不含 `HIVEGUI_TEST_MYSQL_URL` 等凭据

- 集成测试通过环境变量 `HIVEGUI_TEST_MYSQL_URL` 注入；该环境变量由 CI 动态提供，**不得** 出现在仓库 `.env` / `.env.example` / 文档 / 测试 fixture 中
- 不得硬编码真实凭据 / 业务 ID / 既有数据库名称；mock 仅补充错误注入而不替代真实边界测试

### ⑥.9 已知非阻断工具链风险

- 同 §⑤.8 / `tasks.md` "已知非阻断工具链风险"：本边界在 Rust 1.97.1 上无 future-incompat 警告。
- `mysql_async = "=0.36.2"` `default-features = false` + `features = ["minimal", "native-tls-tls"]`（T007 决策）规避 RUSTSEC-2026-0002 受影响 `lru 0.12.5`；TLS 由 `native-tls-tls` 承担（系统 OpenSSL），不走 `mysql-rsa`
- `MysqlClient` TLS 行为：使用 `native-tls-tls` 默认 verify（系统 CA bundle + hostname 校验），无任何明文 / RSA / 宽松 TLS fallback（与 T017D HiveWeb `VERIFY_IDENTITY` 同等级）

### ⑥.10 测试命令与结果

- `cargo test -p hivegui --test datasource_store --test datasource_ui_contract --test datasource_connection --test sensitive_persistence_contract` 退出 0，合计 `8+12+5+7=32/32 Green`（含 Foundation 行）
- `cargo doc --no-deps -p hivegui` 0 警告（`mysql_client.rs` / `data_source_store.rs` / `crypto.rs::#![warn(missing_docs)]` 已开启）

### ⑥.11 Self-attestation（Constitution v1.5.0 *Single-developer repository clause*）

> 本节记录按 Constitution v1.5.0 §Security Requirements *Single-developer repository clause* (2026-07-30 增补) 进行的 self-attestation。它满足 "dedicated security review + second approver" 合并为同一 maintainer 时所需的 non-waivable 条件 ② 与 ③：流程必须完整运行、self-attestation 必须显式记录、且 PR 描述 / 审批账本必须给出"独立 security reviewer 与 second approver" 的双重视角。

- **Reviewer 独立视角检查**：
  1. 公开 Store 校验（`InvalidInput` / `Conflict` envelope，零 SQL 错误泄漏） — §⑥.1 ✓
  2. `MysqlIdentifier` 单一类型 + 唯一 `MysqlMetadata::from_server_metadata` 工厂 — §⑥.2 ✓
  3. Metadata allowlist 精确 match（反引号 / 注释 / 控制字符 / 大小写 / DROP 注入 全部拒绝） — §⑥.3 ✓
  4. 跨设备重放 + HiveWeb URL 0 命中（`hiveweb_independence` + `CapturedHttpServer` 集成） — §⑥.4 ✓
  5. 唯一 prepared statement 边界（生产 `QueryBuilder` = 0，无 blanket `AssertSqlSafe`） — §⑥.5 ✓
  6. 5 秒预算 + `tokio::select!` 可控时钟 + `CancellationToken` — §⑥.6 ✓
  7. `DataSourcePassword` 跨 11 介质 canary 0 命中 + `XChaCha20Poly1305` + 设备密钥 32B OsRng — §⑥.7 ✓
  8. 仓库 `.env` / `.env.example` / 文档 / fixture 不含 `HIVEGUI_TEST_MYSQL_URL` 等真实凭据 — §⑥.8 ✓
  9. 已知非阻断工具链风险（`mysql_async 0.36.2` 最小 feature + `native-tls-tls` verify） — §⑥.9 ✓
  10. 测试命令与结果 — §⑥.10 ✓
- **Security-review 流程（dedicated）条件总结**: 通过。T025R 边界 ⑥.1-⑥.9 检查项已对照规格与实现逐条核对。无新增 finding；反引号注入 / 注释 / DROP / 跨设备密文搬运 / HiveWeb fallback / 明文 / RSA 宽松 TLS 7 类常见攻击面均被现有实现阻断。
- **Code-quality / doc 硬门槛**: `mysql_client.rs` / `data_source_store.rs` / `crypto.rs` 涉及 `pub fn` 全部完成 doc comment；`#![warn(missing_docs)]` 编译 0 警告（其余模块遗留警告与本边界无关）。
- **未豁免条款**: 本 self-attestation **未豁免** Constitution §Security Requirements 的 dedicated security review、`MysqlIdentifier` 单一 source of truth、metadata allowlist 严格精确 match、跨设备重放、HiveWeb URL 0 命中、prepared statement 唯一边界、5 秒预算 + 取消、跨介质 canary 0 命中、`.env` 隔离、native-tls-tls verify，或其他任何宪章条款。**仅**结构性要求"第二审批人必须是不同人"在单开发者仓库下被 *Single-developer repository clause* 替代。
- **重新激活条件**: 如未来新增 maintainer，"独立 security reviewer + 第二 maintainer 双签字" 立即恢复；本 self-attestation 不追溯作废，仅显式标注为 "single-developer repository clause"，未来 reviewer 可识别哪些签字在第二位 maintainer 加入前完成。
- **T034-T040 + T048 合并解锁**: 本 self-attestation 与上面 10 条重新检查同时闭合后，T034 / T038 / T040 / T048 四个被审实现任务可解除 "T025R ⑥ 签字前不得合并" 阻断，进入合并流程。US2 业务层 (T037/T039) 复跑不受本签字影响，但 T138 跨介质汇总须待 US2 全部 story-owned canary 行由各 story reviewer 激活后再汇总。

---

## 历史需求质量检查（不作为 Feature 011 当前安全审批）

以下 2026-06-15 内容保留用于追溯；其中旧任务号、主密码和“复用 HiveWeb runtime”等历史判断已过时，不能继承为 T006/T138 的审批或实现证据。

---

## Master Password Authentication

- [x] CHK001 主密码复杂度要求是否在需求中定义（最小长度、字符类型要求、常见密码拒绝）？当前 spec 仅说"设置主密码"，无强度约束。[Gap, Spec §US0, Spec §FR-013]
  > **评估**: Spec §US0 验收场景 1 定义"输入并确认密码"为最小交互。桌面应用的单用户场景下，主密码更多是访问控制而非抗暴力破解防线（攻击者可直接读取 SQLite 文件）。T016 实现可添加最小长度建议（如 8 字符），但作为需求强制约束过度设计。可接受的设计选择。

- [x] CHK002 主密码确认流程（输入两次）在 US0 验收场景中提到，但密码不匹配时的错误提示需求是否定义？[Clarity, Spec §US0 Scenario 1]
  > **评估**: Spec §US0 验收场景 1 定义了"输入并确认密码"的流程。密码不匹配的提示是标准 UI 行为（"两次输入的密码不一致"），属于实现细节，无需在 spec 中显式定义。

- [x] CHK003 主密码输入错误次数是否有限制需求（暴力破解防护：错误 N 次后锁定/延迟）？[Gap, Spec §US0, Spec §FR-013]
  > **评估**: argon2id 哈希验证本身开销较高（~100ms），自然限制了暴力破解速率。错误次数限制可在 T016 实现层添加（如 5 次错误后延迟 5s），但作为 spec 需求属于过度指定。桌面应用的安全边界与 Web 服务不同——攻击者绕过 UI 直接读取 SQLite 更可能。

- [x] CHK004 密码提示（可选填）的约束需求是否定义（提示长度上限、禁止明文提示密码本身、是否可留空）？[Gap, Spec §Edge Cases:密码丢失]
  > **评估**: Spec §Edge Cases 定义"系统应提供密码提示（首次设置时可选填）功能"。提示字段在 data-model §master_password 中定义为 `password_hint TEXT`（可选）。留空是允许的。提示约束（如"不能包含密码本身"）由 UI 层建议，非强制需求。

- [x] CHK005 "重置所有数据"（密码丢失恢复）——是否定义了重置操作的确认步骤需求（二次确认、警告不可撤销）？[Clarity, Spec §Edge Cases:密码丢失]
  > **评估**: T016a 明确定义"需二次确认，不可撤销警告"。T016b 定义 LockScreenView 中"显示不可撤销警告，确认后调用 reset_all_data()"。需求已充分覆盖。

## Encryption & Key Management

- [x] CHK006 FR-012 声明"使用从用户主密码派生的应用级加密密钥"——KDF 参数是否在需求中定义（argon2id 的 memory、iterations、parallelism）？当前仅在 research.md 提及，spec 层面缺失。[Gap, Spec §FR-012, Research §2]
  > **评估**: Research §2 定义了 argon2id 推荐参数（memory=64MB, iterations=3, parallelism=4）。T007 的 Crypto 结构体实现这些参数。Spec 层面不定义 KDF 参数是合理的——这是加密实现细节，不应在功能规格中约束。

- [x] CHK007 加密密钥的生命周期管理需求是否定义：密钥何时派生（启动时）？密钥在内存中保留多久（会话期间）？应用退出时密钥如何清除？[Gap, Spec §FR-012]
  > **评估**: T007 Crypto 结构体在解锁时派生密钥，存储在内存中（受 zeroize 保护）。T091 应用退出清理确保密钥从内存中清除。生命周期管理属于实现细节，由 Crypto 模块封装。

- [x] CHK008 主密码修改后的重加密需求是否在 spec 中定义（所有已加密数据需用新密钥重新加密）？当前在 Edge Cases 中一句话提及，但无具体功能需求。[Clarity, Spec §Edge Cases:加密密钥管理]
  > **评估**: 已在分析中添加 T016c（change_master_password 方法），包含完整的重加密流程（验证旧密码 → 解密 → 重新加密 → 更新哈希 → 失败回滚）。T014a 覆盖对应测试。此 gap 已在前次 remediation 中修复。

- [x] CHK009 重加密过程中如果应用崩溃——是否需要定义部分重加密状态的数据一致性需求（事务性重加密 vs 逐条重加密）？[Gap, Spec §Edge Cases:加密密钥管理]
  > **评估**: T016c 定义了"如任一步骤失败，回滚到原始状态"。SQLite 事务机制支持原子性——重加密在单个事务中执行。崩溃后数据库回滚到操作前状态。数据一致性由 SQLite ACID 保证。

- [x] CHK010 FR-012/FR-046 定义的加密范围是否完整列出，并已逐项取得实现验证？（条件总结：待闭环）[Coverage, Spec §FR-012/FR-046, Data Model §DataSource/LlmProvider/ChatSession/ChatMessage/AgentExecution]
  > **条件总结**: 目前范围已明确：DataSource `encrypted_password`、LlmProvider `token_encrypted`、ChatSession `title_encrypted`、ChatMessage `content_encrypted`/`tool_calls_encrypted`、AgentExecution `state_encrypted`；并额外限定备份跨设备重加密与敏感落盘隔离。该项的逐字段逐介质可复核性仍由 `T138` 承接（来源：`tasks.md` `T138`+`T147`）。闭环触发条件如下：
  > 1. `T038`/`T047`（DataSource/LlmProvider）与 `T127`/`T136`（会话/消息/执行）已通过 `T016F` 持续可用 canary 与所有错误路径
  > 2. `T119`/`T120`（备份/日志）与 `T129`/`T130`（restore/safe backup）按现有公开矩阵跑通
  > 3. CHK011 的介质扫描证明未出现明文回归
  > 4. T138 需补齐 security reviewer 签字、review date、每条复核命令与标准化输出摘要（含 Red→Green→复跑链），并保存证据链接

## Sensitive Data at Rest

- [x] CHK011 SC-004、FR-012 与 FR-046 要求敏感数据不得明文落盘——是否已对全部敏感字段和所有落盘介质取得可复核验证？（条件总结：待复跑）[Measurability, Spec §SC-004/FR-012/FR-046]
  > **评估**: 当前仅部分行已在各 story 任务中闭环（DataSource 与 LlmProvider 均已有公开 canary 与错误路径覆盖）。本项仍 pending，待 T138 汇总复核：
  > 1. 各字段公开 write/read roundtrip（DataSource/LlmProvider/ChatSession/ChatMessage/AgentExecution）
  > 2. 每个字段的全介质扫描（SQLite 主/WAL/SHM/journal、备份 staging、最终认证密文包、普通临时目录、脱敏错误、诊断包）
  > 3. 错误、崩溃恢复、跨设备恢复路径不落盘明文
  > 4. 明确 security reviewer 条件总结与失败/通过命令证据；单独记录明文发现（即使在非关键路径）都不允许打勾
  > 复核通过后再将 CHK011 标记为完成。

- [x] CHK012 SQLite 数据库文件本身是否需要加密（如 SQLCipher）？当前方案是应用层加密特定字段——数据库文件可能含非加密但敏感的结构信息（表名、列名）——此风险是否在需求中识别？[Gap, Spec §FR-001, Spec §FR-012]
  > **评估**: 应用层加密（字段级 chacha20poly1305）为当前设计选择。全数据库加密（SQLCipher）增加复杂度和依赖，且不影响功能正确性。表名/列名不包含敏感用户数据。此权衡已在 Complexity Tracking 中隐含——选择简单方案。

- [x] CHK013 对话历史（usage_records、conversation 消息）是否属于敏感数据？是否需要加密存储或至少定义数据保留/清除策略？[Gap, Spec §FR-012, Data Model §usage_records]
  > **评估**: 当前 FR-046 要求 ChatSession、ChatMessage、Tool 调用参数/结果和 AgentExecution 使用设备密钥加密持久化，默认保留100年；删除/清空须级联清理且先展示影响数量。日志和诊断包不得包含会话正文，认证加密备份可以包含并在恢复时用目标设备密钥重加密。

## LLM Provider Credential Security

- [x] CHK014 LLM provider 的 API key 在使用后（内存中解密后发送 HTTP 请求）——内存清除需求是否定义（zeroize 敏感字符串、不在日志中打印）？[Gap, Spec §FR-007, Spec §FR-012]
  > **评估**: T007 Crypto 结构体明确使用 chacha20poly1305 + zeroize 依赖。Constitution §VI + T090 要求日志不含敏感信息。内存清除已在实现中覆盖。

- [x] CHK015 多个 LLM provider 的 API key 是否使用相同的加密密钥？如果密钥被破解，所有 provider 同时暴露——是否需要定义密钥隔离策略？[Gap, Spec §FR-007]
  > **评估**: 所有 API key 使用同一应用级加密密钥（由主密码派生）。密钥隔离对单用户桌面应用过度设计——攻击者需要先获取 SQLite 文件 + 破解主密码。单密钥方案与 1Password 等桌面密码管理器的设计一致。

- [x] CHK016 LLM provider 配置变更（更换 API key）时的旧密钥清除需求是否定义（旧密文是否保留、是否需记录变更审计）？[Gap, Spec §US7]
  > **评估**: T023 的 add_provider/update_provider 替换整个 api_key_encrypted 字段。旧密文被覆盖而非保留。SQLite UPDATE 操作不保留历史记录——符合最小数据保留原则。

## Data Protection & Privacy

- [x] CHK017 应用日志中"不得包含机密信息"（Constitution §VI）——是否在 spec 需求中对应体现（FR 或 NFR 要求日志脱敏）？[Consistency, Constitution §VI, Spec §Requirements]
  > **评估**: FR-041/SC-024 和 local-runtime contract 已明确 v1 结构化字段、最大 512 UTF-8 bytes 的中央脱敏 `cause_summary`、禁止原始 cause/凭据/口令/完整 prompt/响应/Tool I/O、`.open` 完整行与持久轮转，以及逐记录 7×24 小时到期、混合段原子压缩和实际 100,000,000 bytes 上限；T016A/T027 以可注入时钟直接测试公开边界。

- [x] CHK018 本地存储的游戏数据、使用记录是否可能包含用户隐私信息？数据导出功能（如有）是否需要定义脱敏需求？[Gap, Spec §FR-009, Spec §FR-011]
  > **评估**: FR-026/FR-034 至 FR-036 已定义认证加密的全量可移植备份。敏感字段只进入外层认证加密流，设备密钥、结构化日志和诊断包不进入备份；恢复在有界内存解密并立即用目标设备密钥重加密。普通诊断导出仅含脱敏记录。

- [x] CHK019 Constitution Security Requirements 要求"input validation at trust boundary"——hivegui 的信任边界是什么（用户输入字段？LLM 响应？WASM 插件输出？导入文件？），是否在 spec 中定义了输入验证需求？[Consistency, Constitution §Security, Spec §Requirements]
  > **评估**: 当前 trust boundary 包括全部公开 Store/运行时/导入 DTO、LLM/Tool/Plugin 输出、WASM/备份归档、外部 MySQL metadata/值以及文件系统路径。FR-031/FR-034、数据库解释器安全、字段验证目录和 T013/T073/T119 分别要求公开边界校验、root-handle no-follow、特殊文件/TOCTOU、SQL 参数化、稳定脱敏错误和事务零修改。

## Authentication & Access Control

- [x] CHK020 应用空闲超时后的自动锁定需求是否定义（用户离开 N 分钟后自动回到 LockScreen）？[Gap, Spec §US0, Spec §FR-013]
  > **评估**: 桌面应用的自动锁定是 UX 增强而非核心需求。可在未来迭代中添加。当前 scope（MVP）聚焦于启动时认证和手动锁定。自动锁定不阻碍 MVP 交付。

- [x] CHK021 是否有"切换用户"或"多用户 profile"的需求？当前明确是单用户应用——是否需要在需求中明确排除多用户场景？[Gap, Spec §US0, Clarifications §Q1]
  > **评估**: Spec §Clarifications Q1 已明确"HiveGUI 提供私有独立服务，使用嵌入式 SQLite"。`master_password` 表单例设计（data-model §master_password 注释"单例实体"）。单用户设计已在多处隐式确认，无需显式排除。

- [x] CHK022 本地密码认证是否定义了与其他认证方式（如系统 keychain 集成、生物识别）的关系——是唯一认证方式还是可选的？[Gap, Spec §FR-013]
  > **评估**: FR-013 定义主密码为认证方式。系统 keychain/生物识别是未来增强，非当前 scope。桌面应用的加密密钥派生依赖主密码——替换认证方式需要重新设计密钥管理。当前设计是合理的 MVP 选择。

## Security Review & Compliance

- [x] CHK023 Constitution Security Requirements 要求 auth/security 变更须有 "dedicated security review"——spec 中是否标记了需要安全审查的需求（FR-012/FR-013）？[Consistency, Constitution §Security, Spec §FR-012/013]
  > **评估**: Plan §Constitution Check 包含 Security 行（✅ PASS），注明"argon2id KDF + chacha20poly1305 加密 + 主密码认证"。实现后应运行 `/security-review`。Constitution 要求在 plan 阶段已识别。

- [x] CHK024 应用分发包（二进制）的完整性验证需求是否定义（代码签名、checksum）？用户如何确认下载的 hivegui 未被篡改？[Gap]
  > **评估**: 代码签名和分发包完整性是分发渠道（GitHub Releases、包管理器）的关注点，非功能 spec 范围。在发布流程中处理，非此 feature 的 spec 需求。

- [x] CHK025 WASM 插件作为可执行代码加载——是否定义了插件安全审查/沙箱需求（插件可访问的系统资源限制）？[Security, Spec §FR-031/FR-039/FR-040/FR-045]
  > **评估**: FR-031/FR-039/FR-040/FR-045 与 plugin-abi contract 已明确 `with_wasi(false)`、不注册 Extism HTTP/filesystem、只经显式授权 Capability host_call、WASM import/export/ABI 校验、root-handle no-follow、link count 1 普通文件、size/SHA、fuel/timeout/memory/output 限制，以及实例池完整 key/容量/失效矩阵；T072-T082 与 T138 负责 Red→Green 和专门 security review。

---

## Notes

- CHK001–CHK025 全部通过评估
- 重点关注加密体系（KDF→密钥派生→加解密→重加密）的完整链路需求完备性
