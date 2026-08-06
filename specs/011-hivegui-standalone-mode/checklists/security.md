# Security Review Checklist: HiveGUI 独立本地 Agent

**Purpose**: Feature 011 的依赖、备份加密、归档解析、Plugin sandbox 与 CI 安全门禁
**Created**: 2026-06-15
**Current review**: 2026-07-28
**Feature**: [spec.md](../spec.md)

## T006 与补充 T017G 当前候选依赖评估（方案目标已确认，实施审批仍 Pending）

**工具链基线**: Rust 1.97.1。版本信息来自 2026-07-22 的 registry metadata 和上游发布记录；实际合并仍由固定版本 `cargo deny check advisories` 重新扫描提交后的完整 `Cargo.lock`。

**方案确认状态**: `Approved for T007`。用户/feature owner 于 2026-07-22 先选择推荐选项 A，批准下表精确版本、最小 feature、Extism 安全升级和 GPUI revision 固定；在发现 Extism 1.30.0 缺失 Wasmtime `anyhow` feature 后，又确认下方补充选项 A1。该确认仅授权本地 T007 实现；宪章要求的专门 security review 和第二批准仍是合并前门禁，由 T138 及最终 PR 审批闭合，不伪装成已经取得的 reviewer 签字。

**Advisory 修复确认状态**: `Historical Red approved; supplemental T017G/T017H pending; Green pending T001 PR/CI`。用户/feature owner 于 2026-07-23 选择推荐方案 A，批准下方 7 个 advisory 的无例外修复目标，随后批准 T017A-T017B Red 证据，并再次批准实施前审计提出的 SQLx feature、Wayland provenance、`game_service` 绑定、named-query 中央审计边界和乐观锁封闭枚举修正。2026-07-28 用户又批准 checked-static-query 推荐 A，使 HiveWeb 最终 SQLx feature 由历史 `derive`-only 目标升级为 `macros`、生产 `QueryBuilder` 为零；同日“全 A”批准 Unicode 17 `NFKC_CF`+NFC 的规范目标与候选依赖方向。上述目标必须先由 T017G 编写、实际观察 Red，并由 T017H 的 Unicode/data、dependency/security 与 SQL reviewer 审批；方案批准不代表 T017H、Green、专门 security review 或第二批准已经完成。

### 依赖决策表

| 依赖 | 精确候选与最小 feature | 维护、许可证与 MSRV | 已知 advisory / 处置 | 结论 |
| --- | --- | --- | --- | --- |
| `tokio-util` | `=0.7.18`，`default-features=false`，仅 `rt`（`CancellationToken`） | Tokio 项目持续维护；MIT；声明 Rust 1.71 | 2026-07-22 未检索到 `tokio-util` 包 advisory；完整锁文件仍必须由 cargo-deny 阻断扫描 | 推荐批准 |
| `age` | `=0.12.1`，`default-features=false`；只用 library passphrase streaming API，不启用 `plugin`、SSH、CLI/pinentry | rage/age 上游 2026 年发布；MIT OR Apache-2.0；声明 Rust 1.74 | RUSTSEC-2024-0433 的修复范围包含 `>=0.11.1`；同时禁用触发外部 age plugin 执行的 feature | 推荐批准 |
| `tar` | `=0.4.46`，`default-features=false`（不启用 xattr） | composefs/tar-rs 仍维护并发布安全修复；MIT OR Apache-2.0；声明 Rust 1.63 | 0.4.46 包含最新 PAX desync 修复，并高于 RUSTSEC-2026-0067/0068 的 `>=0.4.45` 修复线；实现仍禁止对不受信包调用 `unpack`/`unpack_in` | 推荐批准，但必须执行下方归档约束 |
| `unicode-normalization` + Unicode 17 生成表 | `unicode-normalization = { version = "=0.1.25", default-features=false, features=["std"] }` 只负责 NFC；`NFKC_CF` 从官方 Unicode 17.0.0 `DerivedNormalizationProps.txt` 确定性生成 | unicode-rs 维护；MIT OR Apache-2.0；声明 Rust 1.36；crate 内置 `UNICODE_VERSION=(17,0,0)` | 当前 lock 已传递解析 0.1.25，但直接依赖、官方 UCD 使用条款、源 SHA-256、生成器/输出 checksum 与数据更新流程仍须 T017G Red、T017H 独立 reviewer 和 T138 最终复核；依赖或数据漂移必须 fail-closed 并分配新 normalization ID | 文档方案 A 已确认；实际直接依赖和生成数据保持 Pending，不得在 T017H 前实施 |
| `extism`（现有依赖升级） | 从 `1.21.0` 改为 `=1.30.0`，`default-features=false`；禁止 Extism 自动 HTTP/filesystem 注册 | Extism 上游最新 1.30.0；BSD-3-Clause；使用 Wasmtime 43 | 当前 1.21.0 带入 Wasmtime 41.0.4，受 RUSTSEC-2026-0114 影响且 41.x 无修复线；1.30.0 升到 Wasmtime 43，当前解析 43.0.2，满足 advisory 的 `>=43.0.2` 修复线 | **阻断式推荐升级**；不得为 41.x 建立无到期例外 |
| `wasmtime`（Extism 兼容 feature） | `=43.0.2`，`default-features=false`；HiveGUI 直接声明只新增 `anyhow` 并用于 Cargo feature-unification，有效图仍包含 Extism/wasi-common 的必要 feature | Bytecode Alliance 持续维护；Apache-2.0 WITH LLVM-exception；声明 Rust 1.91 | 43.0.2 位于 RUSTSEC-2026-0114 修复线；不用 `wasmtime-default-features` 进一步扩大功能面 | A1 已确认；必须验证 Extism HTTP/filesystem/default feature 仍关闭 |
| `gpui-component` / assets（现有 git 依赖固定） | 两项都增加 `rev="49f4b4fb57553daa89310370a97b37f7ff9d2323"`，保持当前锁文件已验证源码 | 上游持续开发；Apache-2.0；Rust 1.97.1 已实际编译 | 当前 manifest 跟随分支且远端 HEAD 已与 lock commit 不同，重建 lock 会产生未审批漂移 | 推荐固定当前 commit，不升级 API |
| benchmark | 不新增 Criterion；T005 使用 `harness=false` 的固定样本 runner 与共享 percentile/baseline 模块 | 减少供应链与 feature 面；只复用 workspace 已有 serde/JSON 和标准库计时 | 无新增依赖 | 推荐批准；若后续改用 Criterion，必须重新走本表审批 |

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

| 角色 | 责任 | Reviewer | 日期 | 结论 |
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

**Finding [HIGH] — 已闭环（2026-07-29）**：原 T025R 边界 ⑤ 规格写 "AES-256-GCM"，实现用 ChaCha20Poly1305，user 批准方案 A：将规格更正为 ChaCha20Poly1305。理由：两者均为 256-bit AEAD / RFC 标准化（RFC 8439）/ 恒定时间实现抗侧信道；ChaCha20Poly1305 在无 AES-NI 的桌面环境下性能更优。`tasks.md` 第 126 行已记录更正。代码未变更（[crypto.rs:14-15](file:///home/developer/agent/gpui-claw/hive-claw-worktree/crates/hivegui/src/auth/crypto.rs#L14)）。

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
- **Security-review 流程（dedicated）结论**: 通过。所有 T025R 边界 ⑤.1–⑤.8 检查项已对照规格与实现逐条核对，详见 §⑤.1–§⑤.8。无新增 finding；旧 finding [HIGH] (⑤.2 AES-256-GCM vs. ChaCha20Poly1305) 已闭环，方案 A 由 user 在 2026-07-29 批准。
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

## 历史需求质量检查（不作为 Feature 011 当前安全审批）

以下 2026-06-15 内容保留用于追溯；其中旧任务号、主密码和“复用 HiveWeb runtime”等结论已过时，不能继承为 T006/T138 的审批或实现证据。

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

- [ ] CHK010 FR-012/FR-046 定义的加密范围是否完整列出，并已逐项取得实现验证？[Coverage, Spec §FR-012/FR-046, Data Model §DataSource/LlmProvider/ChatSession/ChatMessage/AgentExecution]
  > **评估**: 当前规范范围至少包括 DataSource `encrypted_password`、LlmProvider `token_encrypted`、ChatSession `title_encrypted`、ChatMessage `content_encrypted`/`tool_calls_encrypted` 和 AgentExecution `state_encrypted`；未来 OAuth 或其他认证令牌只有先进入公开敏感字段目录和加密契约后才可持久化。字段范围已经澄清，但 T038、T047、T117、T119/T120 与 T138 的逐字段密文、备份/日志排除和专门安全审查证据尚未闭合，因此本项保持未完成，不得再把 API Key 视为唯一加密字段。

## Sensitive Data at Rest

- [ ] CHK011 SC-004、FR-012 与 FR-046 要求敏感数据不得明文落盘——是否已对全部敏感字段和所有落盘介质取得可复核验证？[Measurability, Spec §SC-004/FR-012/FR-046]
  > **评估**: 要求已定义，但实现证据 Pending。关闭本项前必须为 CHK010 的每个字段直接验证公开写入/读取 roundtrip，使用唯一明文 canary 扫描 SQLite 主文件、WAL/SHM/journal、备份 staging/最终密文包、普通临时目录、结构化日志和诊断包，并证明错误、崩溃恢复和跨设备恢复路径也不泄漏明文；还须记录精确测试命令、退出状态和 T138 security reviewer 结论。仅 API Key 单元测试或代码审查不足以关闭本项。

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
