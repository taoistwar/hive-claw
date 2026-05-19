# Research: 数据源管理

## Decision 1: MySQL 客户端库选型

**Decision**: `mysql_async`

**Rationale**: `mysql_async` 是 Rust 生态中最成熟的异步 MySQL 客户端，原生支持 Tokio 运行时，与 `axum` 技术栈一致。支持连接池、预处理语句、SSL 连接，能够满足连接测试、元数据查询和数据预览的需求。

**Alternatives considered**:
- `mysql` (同步版本)：不适合与 Tokio 异步运行时配合，会阻塞事件循环
- `sqlx` + MySQL：功能强大但更重量级，且我们的场景不需要 ORM 层的抽象，直接执行元数据查询更简单
- `r2d2-mysql` + `mysql` 连接池方案：增加复杂度，对于单用户桌面应用来说过重

---

## Decision 2: SQLite 持久化方案

**Decision**: `sqlx` with SQLite (async)

**Rationale**: `sqlx` 提供编译期 SQL 校验（可通过 offline mode 使用），支持异步操作，与 Tokio 运行时配合良好。相比 `rusqlite`（同步），`sqlx` 的 async 支持更适合我们的异步架构。数据源配置是结构化关系数据，符合宪法指定的 SQLite 用途。

**Alternatives considered**:
- `rusqlite`：同步 API，在异步上下文中需要使用 `spawn_blocking`，增加复杂度
- `sled`：KV 存储，不适合需要查询的关系数据（如按数据源名称搜索、按创建时间排序）
- `tokio-rusqlite`：封装层，不如直接用 `sqlx` 统一

---

## Decision 3: 密码加密方案

**Decision**: `chacha20poly1305` + 用户级密钥派生

**Rationale**: 密码等敏感信息需要加密存储。使用 `chacha20poly1305`（AEAD 加密算法）提供认证加密，密钥从用户登录凭证或设备指纹派生。对于本地桌面应用，也可以考虑使用操作系统的密钥链（Linux: libsecret），但 `chacha20poly1305` 更轻量且跨平台一致。

**Alternatives considered**:
- `libsodium` (via `sodiumoxide`)：功能更全但依赖系统库，增加部署复杂度
- OS 密钥链 (Linux libsecret / macOS Keychain)：更安全但平台差异大，v1 优先简单方案
- 简单 AES-256-GCM：`chacha20poly1305` 在软件实现中性能更好

---

## Decision 4: 架构归属 — 全部在 HiveGUI 中实现

**Decision**: 数据源管理的所有功能（存储、MySQL 连接、查询、UI）在 `hivegui` crate 内实现，不涉及 `hiveclaw` 后端 API

**Rationale**: 数据源管理是桌面应用本地功能，用户直接在 GUI 中操作本地 SQLite 存储和直连 MySQL 服务器。不需要经过后端服务层，简化了架构，减少了 HTTP 通信的复杂度和延迟。这符合单用户桌面应用的典型模式。

**Alternatives considered**:
- HiveGUI 通过 HTTP 调用 HiveClaw API：增加不必要的网络层，对于本地桌面场景过于复杂
- 纯后端实现：不符合桌面应用的交互模式

---

## Decision 5: MySQL 元数据查询策略

**Decision**: 使用 `information_schema` 查询数据库、表、列元数据

**Rationale**: `information_schema` 是 MySQL 标准元数据查询方式，兼容所有 MySQL 5.7+ 版本。通过 `information_schema.SCHEMATA`、`TABLES`、`COLUMNS` 等系统视图获取元数据，避免使用 MySQL 特有的 `SHOW` 语句（返回格式不统一，难以序列化）。

**Alternatives considered**:
- `SHOW DATABASES` / `SHOW TABLES` / `DESCRIBE`：返回非结构化结果，解析复杂
- `mysql` CLI 工具调用：需要额外进程管理，不适合嵌入式场景

---

## Decision 6: 连接池 vs 按需连接

**Decision**: 按需创建连接，不维护长连接池

**Rationale**: 单用户桌面应用，并发量低。元数据查询频率不高（用户手动展开树节点时触发），按需创建/关闭连接足够。这简化了实现，避免了连接池管理的复杂性。后续如需要可升级为 `mysql_async` 的连接池。

**Alternatives considered**:
- `mysql_async::Pool`：适合高并发场景，v1 场景收益低
- 长连接保持：需要心跳、重连逻辑，增加复杂度

---

## Decision 7: 表数据 WHERE/ORDER BY 注入安全

**Decision**: 用户输入的 WHERE/ORDER BY 直接拼接为 SQL 片段，在 UI 层明确提示用户这是"高级功能"，仅面向有数据库知识的用户

**Rationale**: 用户是具备数据库知识的目标用户（spec 假设）。WHERE 条件和 ORDER BY 由用户自行编写，系统仅做基本的 SQL 注入防护（如限制多语句）。这提供了最大灵活性。

**Alternatives considered**:
- 参数化 WHERE：需要构建查询构建器，增加复杂度，灵活性受限
- 完全禁止自定义查询：不符合 spec 中"支持自定义 WHERE 条件"的需求
