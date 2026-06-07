# Research: 敏感词过滤

**Feature**: 010-sensitive-word-filter | **Date**: 2026-06-07

## Decision 1: 多模式匹配算法

**Decision**: 精确匹配使用 Aho-Corasick 算法，正则匹配使用预编译 `regex::Regex` 缓存。

**Rationale**:
- Aho-Corasick 是公认最优的多模式精确匹配算法，O(n+m) 时间复杂度（n=文本长度, m=匹配数），远优于逐模式 O(k*n) 扫描。`aho-corasick` crate 是 Rust 生态标准实现，零依赖，已广泛应用于安全工具。
- 正则匹配天然需要逐个模式遍历（不同模式语法不可合并），但通过预编译 `Regex` 实例缓存避免每次编译开销。ReDoS 防护通过 `regex::RegexBuilder::size_limit()` 限制 DFA 大小实现。

**Alternatives considered**:
- 纯逐模式迭代：简单但 O(k*n)，10,000 条时不可接受
- hyperscan (C FFI)：性能极致但引入 native 依赖，违背宪法 V (Simplicity)
- `fancy-regex`：支持 lookahead 但性能开销大且易受 ReDoS

## Decision 2: 缓存刷新策略

**Decision**: 启动时全量加载到 `Arc<RwLock<Vec<SensitivePattern>>>`，变更时通过管理 API 主动触发缓存刷新。

**Rationale**:
- `Arc<RwLock>` 模式：读取路径无锁竞争（RwLock::read 低成本），写入仅在 admin CRUD 时触发（RwLock::write 低频），符合读取远多于写入的访问模式。与现有 `AgentContext` 的 `CategoryLock<T>` 设计一致。
- API 主动刷新：管理后台 CRUD 操作完成后，调用 `POST /api/sensitive-words/cache-refresh` 或直接内联刷新，简单可靠，无需 Redis Pub/Sub 或 DB LISTEN/NOTIFY。

**Alternatives considered**:
- 定时轮询 DB：延迟 1-N 秒，不符合 SC-004 (1 秒内生效)
- Redis Pub/Sub：增加依赖，10,000 条规模不值得引入额外的通知通道
- ArcSwap：无锁读性能更好，但需要整体替换 Vec，对于 10,000 条规模 RwLock 已足够

## Decision 3: ReDoS 防护

**Decision**: 使用 `regex::RegexBuilder::size_limit(1024 * 1024)` 限制 DFA 大小 + 使用 `regex::RegexBuilder::dfa_size_limit(256 * 1024)` 二级限制。

**Rationale**:
- `size_limit` 限制编译后 DFA 的内存占用，超过限制则 regex crate 自动 fallback 到更慢但安全的 NFA 引擎（不会拒绝编译）。
- 结合 FR-005 的"保存时校验合法性"，管理员无法提交语法错误的 pattern。
- 不需要超时机制：Rust regex crate 自身是 ReDoS-safe（不使用回溯），限制 DFA 大小足够。

**Alternatives considered**:
- `regex::bytes::Regex` with timeout: Rust regex 本身不提供 timeout API，需要外部 `tokio::time::timeout` 包装，复杂度增加但收益有限
- `fancy-regex`: 支持更多特性但性能差且有 ReDoS 风险

## Decision 4: 数据库表设计

**Decision**: 使用内部 MySQL 表 `sensitive_words`，字段简洁：id, word, match_mode, enabled, created_at, updated_at。

**Rationale**:
- 与现有 `functions`, `tools`, `workflows` 等内部配置表设计风格一致（单一职责表 + soft delete via enabled flag + created_at/updated_at）。
- 不使用外部数据库：敏感词是运营配置数据，非业务数据，属于管理后台范畴。
- `match_mode` 枚举：`exact` 或 `regex`，字符串存储便于管理后台直接展示。

## Decision 5: 前端复用策略

**Decision**: 管理后台页面复用现有 `AdminTable` 通用表格组件 + 新建 `SensitiveWordPage.tsx`。

**Rationale**:
- 管理后台有成熟的 `AdminTable` 组件支持搜索、分页、增删改操作，直接复用减少代码量。
- 匹配模式选择器复用 `Select` 组件，表单验证复用 Ant Design `Form` 校验规则。
- 遵循现有 web-admin 的单页文件结构（`pages/XxxPage.tsx` + `services/xxx.ts`）。

## Decision 6: 预置词库 Seed 策略

**Decision**: 首次部署时通过 `seed_sensitive_words` bin 脚本导入预置词库。词库来源于 funNLP + houbb/sensitive-word 等多个开源仓库的合并去重结果，存储为 seed JSON 文件。bin 脚本读取 JSON 文件逐条 INSERT（含重复检查），不覆盖管理员已修改/删除的词条。

**Rationale**:
- 复用现有 `seed` bin 模式（`cargo run -p hiveweb --bin seed`），与其他种子数据一致。
- `INSERT IGNORE` + 首次运行标记：仅首次部署时执行，后续 `migrate` 不会重复导入，允许管理员后续修改。
- JSON 格式便于版本控制和更新（直接替换 seed 文件即可更新预置词库）。

**Alternatives considered**:
- SQL migration 直接 INSERT：但 migration 每次部署都执行，会覆盖管理员修改
- 单独 API 导入：增加复杂度，seed 模式更简单
