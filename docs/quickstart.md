# 快速开始：HiveClaw 全系统启动指南

**目标读者**：新加入项目的工程师，在一台全新的 Linux 或 macOS 机器上起步。

**目标**：从干净检出代码开始，在 **15 分钟内**（SC-001）完成编译启动，发送一条消息并查看日志。

## 1. 前置条件（一次性设置，约 5 分钟）

### 基础环境

- 操作系统：Linux x86_64（主要）或 macOS arm64（次要），Windows 暂不在 v1 范围内。
- `git` ≥ 2.30。
- Rust 工具链：仓库根目录的 `rust-toolchain.toml` 已锁定版本，安装 [rustup](https://rustup.rs/) 后首次运行 `cargo` 时会自动安装对应版本。
- Node.js 18+（前端项目需要）。
- MySQL 8.0+（管理中心后端需要）。
- Redis 7+（管理中心后端缓存需要）。
- （可选）MinIO 或 S3 兼容存储（文件上传需要）。

### Linux 额外依赖

`gpui` 需要以下 GUI 开发库，通过系统包管理器安装：

```bash
sudo apt update
sudo apt install -y libxkbcommon-dev libwayland-dev libvulkan-dev pkg-config build-essential
```

### macOS 额外依赖

安装 Xcode Command Line Tools：

```bash
xcode-select --install
```

## 2. 克隆并构建（暖缓存约 3 分钟，冷构建约 8 分钟）

```bash
git clone <repo-url> hive-claw
cd hive-claw
cargo build --workspace
```

首次构建会解析工作空间、下载依赖，并在 `target/debug/` 下生成二进制文件：

- `target/debug/hiveclaw` — Agent 运行时
- `target/debug/hivegui` — GUI 客户端

此步骤不会修改任何受源代码控制的文件（SC-005）。

## 3. 启动 Agent 服务 — HiveClaw（终端 A）

```bash
# 可选：修改监听地址（默认 127.0.0.1:8686）
# export HIVECLAW_BIND_ADDR=127.0.0.1:8686

cargo run -p hiveclaw
```

启动成功后，stderr 会输出类似以下格式的结构化日志：

```json
{"timestamp":"2026-05-14T08:01:23.456Z","level":"INFO","fields":{"message":"HiveClaw listening","bind_addr":"127.0.0.1:8686","version":"0.1.0"},"target":"hiveclaw"}
```

**验证接口**，在第三个终端执行：

```bash
curl -sS -X POST http://127.0.0.1:8686/v1/responses \
  -H 'Content-Type: application/json' \
  -d '{"model":"openclaw:hiveclaw-placeholder-v1","input":"hello"}' | jq
```

响应格式参见 `contracts/openresponses-v1.md` 中的 `Response — synchronous` 部分。

## 4. 启动 GUI 客户端 — HiveGUI（终端 B）

```bash
# 可选：指定 HiveClaw 地址
# export HIVECLAW_URL=http://127.0.0.1:8686

cargo run -p hivegui
```

原生窗口打开后将看到：

- 主面板上的对话区域。
- 两个可导航的区块：**Day+1 工具** 和 **Hour+1 工具**，每个区块显示中文空状态提示（"暂无工具"，参见 `crates/hivegui/src/ui/strings_zh.rs` 中的正式文案）。

在对话区域输入消息并发送，应看到：

1. 你的消息出现在对话线程中，标记为本人发送。
2. 进行中的加载指示器（FR-008）。
3. HiveClaw 的占位回复在约 3 秒内出现在下方（SC-002 预算）。
4. 只有在收到回复后，发送按钮才会重新启用（FR-008a）。

## 5. 日志位置

- **HiveClaw**：仅输出到 stderr（终端 A）。
- **HiveGUI**：同时输出到 stderr（终端 B）和平台用户应用数据目录下的滚动 JSONL 日志文件：
  - Linux：`$XDG_DATA_HOME/hivegui/logs/`（默认 `~/.local/share/hivegui/logs/`）
  - macOS：`~/Library/Application Support/hivegui/logs/`

  日志按天滚动。每行包含一个 JSON 对象，字段包括：`timestamp`、`level`、`target`、`conversation_id`、`request_id`、`operation`、`outcome`、`duration_ms`（遵循 FR-012b 和宪法原则 VI）。

## 6. 测试失败路径

在 HiveGUI 运行时，停止 HiveClaw（在终端 A 按 Ctrl-C）。在 HiveGUI 中发送另一条消息，应看到：

- 该轮对话显示清晰的中文错误提示（"HiveClaw 不可达，请检查服务是否运行"，参见 `strings_zh.rs` 的最终文案）。
- 失败对话上出现可见的**重试**操作按钮（规范 Edge Cases / FR-008a）。
- 无自动重试，需手动点击。

重启 HiveClaw，点击失败的对话上的**重试**按钮，占位回复应正常返回。

---

## 7. 管理中心 & 用户中心（HiveWeb）

以下步骤用于启动 HiveWeb 管理中心后端及前后端。

### 7.1 设置数据库

```bash
# 创建 MySQL 数据库
mysql -u root -p -e "CREATE DATABASE hiveweb CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;"

# 启动 Redis
redis-server
```

### 7.2 后端启动

```bash
cd crates/hiveweb

# 复制环境变量文件
cp .env.example .env
# 编辑 .env 文件，配置数据库连接及相关参数

# 运行数据库迁移
cargo run --bin migrate

# 创建初始超级管理员
cargo run --bin create-super-admin -- \
  --phone "18810154696" \
  --password "admin123" \
  --nickname "Super Admin"

# 启动后端服务器
cargo run
# 访问 http://localhost:3300
```

### 7.3 管理中心前端（web-admin）

```bash
cd web-admin

# 安装依赖
npm install

# 复制环境变量文件
cp .env.example .env

# 启动开发服务器
npm run dev
# 访问 http://localhost:5173
```

### 7.4 用户中心前端（web-user）

```bash
cd web-user

# 安装依赖
npm install

# 复制环境变量文件
cp .env.example .env

# 启动开发服务器
npm run dev
# 访问 http://localhost:5174
```

---

## 8. 运行测试

在仓库根目录执行：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

三条命令必须在干净检出上全部通过（宪法工作流质量门禁）。`crates/hiveclaw/tests/contract_responses_*.rs` 中的合约测试会验证 `contracts/openresponses-v1.md` 中的线缆格式。

后端额外测试：

```bash
cd crates/hiveweb
cargo test
cargo fmt
cargo clippy -- -D warnings
```

前端测试：

```bash
# web-admin
cd web-admin
npm test
npm run format
npm run lint

# web-user
cd web-user
npm test
npm run format
npm run lint
```

---

## 9. 环境变量参考

### Agent 运行时环境变量

| 变量名 | 说明 | 默认值 |
|--------|------|--------|
| `HIVECLAW_BIND_ADDR` | 监听地址 | 127.0.0.1:8686 |
| `HIVECLAW_URL` | HiveGUI 连接地址 | http://127.0.0.1:8686 |

### HiveWeb 后端环境变量

| 变量名 | 说明 | 默认值 |
|--------|------|--------|
| `HIVEWEB_HOST` | 服务器地址 | 127.0.0.1 |
| `HIVEWEB_PORT` | 服务器端口 | 3300 |
| `DATABASE_URL` | MySQL 连接字符串 | （必填） |
| `REDIS_MODE` | Redis 模式：`direct` 或 `sentinel` | direct |
| `REDIS_URL` | 直连模式的 Redis 连接字符串 | redis://127.0.0.1:6379 |
| `REDIS_SENTINEL_MASTER` | Sentinel master 名称 | Sentinel 模式必填 |
| `REDIS_SENTINEL_NODES` | 逗号分隔的 Sentinel `host:port` 列表 | Sentinel 模式必填 |
| `REDIS_DATABASE` | Sentinel 数据节点数据库索引 | 0 |
| `REDIS_SENTINEL_REFRESH_MS` | Sentinel master 刷新间隔（毫秒） | 1000 |
| `REDIS_USERNAME` / `REDIS_PASSWORD` | Sentinel 数据节点认证 | （可选） |
| `REDIS_SENTINEL_USERNAME` / `REDIS_SENTINEL_PASSWORD` | Sentinel 服务自身认证 | （可选） |
| `REDIS_CONNECT_TIMEOUT_MS` | Redis 发现和建连超时（毫秒） | 5000 |
| `JWT_SECRET` | JWT 密钥 | （必填，至少 32 字符） |
| `JWT_EXPIRATION_HOURS` | Token 有效期（小时） | 24 |
| `S3_ENDPOINT` | S3 端点 | （可选） |
| `S3_ACCESS_KEY_ID` | S3 访问密钥 | （可选） |
| `S3_SECRET_ACCESS_KEY` | S3 密钥 | （可选） |
| `S3_BUCKET` | S3 桶名 | （可选） |

完整说明及 Sentinel 配置注意事项见 [环境变量](env-vars.md)。

### 前端环境变量

| 变量名 | 说明 | 默认值 |
|--------|------|--------|
| `VITE_API_URL` | 后端 API 地址 | http://localhost:3300 |
| `VITE_APP_TITLE` | 应用标题 | 管理中心 |

---

## 10. Dev 环境容器配置

### Redis

```bash
docker run -d --restart=always --name agent_redis_16379 -p 16379:6379 redis redis-server --requirepass "ai123456"
```

### Rustfs（S3 存储）

```bash
git clone https://ghfast.top/https://github.com/rustfs/rustfs
cd rustfs
sudo mkdir -p /opt1/rustfs /opt2/rustfs /opt3/rustfs /opt4/rustfs
sudo chown -R 10001:10001 /opt1/rustfs /opt2/rustfs /opt3/rustfs /opt4/rustfs
sudo chmod -R 755 /opt1/rustfs /opt2/rustfs /opt3/rustfs /opt4/rustfs
docker compose --profile observability up -d
```

### MySQL

```bash
docker run -d --restart=always --name agent_mysql_stack -p 33060:3306 -e MYSQL_ROOT_PASSWORD='ai123456' mysql:5.7.41
```

---

## 11. 常见问题

### Linux 上 `cargo run -p hivegui` 无法打开窗口

通常是缺少系统库（`libxkbcommon-dev`、`libvulkan-dev`），请重新检查第 1 节的 Linux 额外依赖。

### HiveGUI 立即显示"HiveClaw 不可达"

HiveClaw 未运行，或 `HIVECLAW_URL` 指向错误地址。使用第 3 节的 `curl` 命令验证。

### 日志目录为空

设置 `HIVEGUI_LOG_LEVEL=debug`，并确认该目录对当前用户可写。

### 数据库连接失败

确保 MySQL 在运行且数据库已创建：

```bash
mysql -u root -p -e "SHOW DATABASES LIKE 'hiveweb';"
```

### Redis 连接失败

确保 Redis 在运行：

```bash
redis-cli ping
# 应返回 PONG
```

### 端口被占用

修改端口：

```bash
export HIVEWEB_PORT=3001
cargo run
```

---

## 12. 生产环境构建

```bash
# 后端
cd crates/hiveweb
cargo build --release

# 前端
cd web-admin
npm run build
```

### Docker 部署（可选）

```bash
# 构建镜像
docker build -t hiveweb .

# 运行容器
docker run -p 3300:3300 hiveweb
```

---

## 13. v1 范围说明

以下功能**不在** v1 范围内：

- 不持久化对话历史（关闭 HiveGUI 即丢失对话，此为设计预期）。
- Day+1 和 Hour+1 工具系列均为空（设计预期）。
- 无认证、无多用户、无共享部署（FR-015）。
- 无 Windows 构建（v1 范围外）。
- 管理中心的完整功能参见 `specs/003-admin-center/`。

这些功能将作为后续特性通过各自的规范文档推进。

## 14. 功能特性一览

- ✅ Agent 运行时（HiveClaw）与 GUI 客户端（HiveGUI）
- ✅ 管理员登录（JWT 认证）
- ✅ 管理员账号管理（增删改查）
- ✅ 角色权限控制（普通/系统/超级管理员）
- ✅ 仪表盘概览
- ✅ 登录失败锁定（Redis）
- ✅ 会话管理（24 小时有效期）
- ✅ 文件上传（S3 存储）

## 15. 许可证

MIT
