# 部署指南

## 环境要求

### 基础环境
- **操作系统**: Linux (Ubuntu 20.04+ / CentOS 7+)
- **Rust**: 1.95+
- **内存**: 最低 512MB，推荐 2GB+
- **磁盘**: 最低 100MB，推荐 1GB+（含缓存）

### 系统依赖

```bash
# Ubuntu/Debian
apt-get update
apt-get install -y \
    libssl-dev \
    pkg-config \
    libsqlite3-dev \
    libgit2-dev \
    libmysqlclient-dev \
    cmake

# CentOS/RHEL
yum install -y \
    openssl-devel \
    pkgconfig \
    sqlite-devel \
    libgit2-devel \
    mysql-devel \
    cmake
```

## 编译安装

### 1. 克隆/下载

```bash
cd /workspace/offline-analysis-agent
```

### 2. 编译

```bash
# Debug 版本（开发用，编译快）
cargo build

# Release 版本（生产用，优化好）
cargo build --release

# 查看二进制大小
ls -lh target/release/offline-analysis-agent
```

### 3. 配置

```bash
# 复制配置模板
cp config.template.toml config.toml

# 编辑配置（推荐使用 sedit 或手动编辑）
vim config.toml
```

### 4. 测试连接

```bash
# 测试数据库和 API 连接
./target/release/offline-analysis-agent --test-connection
```

### 5. 运行

```bash
# TUI 模式（默认）
./target/release/offline-analysis-agent

# 或者指定 GUI 类型
./target/release/offline-analysis-agent --gui tui
```

## 配置说明

### 必填配置

```toml
# Hive Metastore MySQL 连接
[hive_metastore]
host = "mysql.example.com"      # MySQL 服务器地址
port = 3306                      # MySQL 端口
database = "hive"               # Hive Metastore 数据库名
username = "hive"               # MySQL 用户名
password = "your-password"      # MySQL 密码

# Azkaban 连接
[azkaban]
host = "http://azkaban.example.com"  # Azkaban Web 地址
username = "your-username"           # Azkaban 用户名
password = "your-password"           # Azkaban 密码

# AI 服务配置
[ai]
provider = "openai"            # AI 提供商：openai, anthropic, azure
model = "gpt-4o"               # 模型名称
api_key = "sk-xxx"             # API Key
```

### 可选配置

```toml
# Git 仓库（用于任务版本管理）
[git]
remote = "git@github.com:data-team/azkaban-jobs.git"
branch_prefix = "task/"
username = "your-username"

# 数据质量告警
[quality]
alert_email = "data-team@example.com"
default_threshold = 0.05
```

## 测试数据库连接

### 方法 1：使用内置测试工具

```bash
./target/release/offline-analysis-agent --test-connection
```

### 方法 2：手动测试 MySQL 连接

```bash
# 安装 MySQL 客户端
apt-get install mysql-client

# 测试连接
mysql -h mysql.example.com -u hive -p hive
```

### 方法 3：使用测试 SQL

```bash
# 连接并执行测试 SQL
mysql -h mysql.example.com -u hive -p hive < examples/test-hive-metastore.sql
```

## 常见问题

### Q1: 编译时提示找不到 libmysqlclient

**解决方案**：
```bash
# Ubuntu/Debian
apt-get install libmysqlclient-dev

# CentOS/RHEL
yum install mysql-devel

# 如果仍然失败，设置 pkg-config 路径
export PKG_CONFIG_PATH=/usr/lib/mysql/pkgconfig
```

### Q2: 连接 Hive Metastore 失败

**排查步骤**：
1. 确认 MySQL 服务运行：`systemctl status mysql`
2. 检查防火墙：`telnet mysql.example.com 3306`
3. 验证用户名密码：`mysql -h mysql.example.com -u hive -p`
4. 确认数据库存在：`SHOW DATABASES;`

### Q3: Azkaban 登录失败

**排查步骤**：
1. 确认 Azkaban Web 服务运行
2. 检查 URL 是否正确（包含 http/https）
3. 验证用户名密码
4. 检查网络连通性：`curl -I http://azkaban.example.com`

### Q4: TUI 显示乱码

**解决方案**：
```bash
# 设置正确的终端编码
export LANG=zh_CN.UTF-8
export LC_ALL=zh_CN.UTF-8

# 或使用支持 UTF-8 的终端
```

## 生产部署

### Systemd 服务（可选）

```ini
# /etc/systemd/system/offline-analysis-agent.service
[Unit]
Description=Offline Analysis AI Agent
After=network.target

[Service]
Type=simple
User=data
WorkingDirectory=/opt/offline-analysis-agent
ExecStart=/opt/offline-analysis-agent/offline-analysis-agent --gui tui
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```bash
# 启用服务
systemctl daemon-reload
systemctl enable offline-analysis-agent
systemctl start offline-analysis-agent
systemctl status offline-analysis-agent
```

### Docker 部署（计划中）

```dockerfile
FROM rust:1.95 AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/offline-analysis-agent /usr/local/bin/
CMD ["offline-analysis-agent", "--gui", "tui"]
```

## 升级

```bash
# 停止服务（如果使用 systemd）
systemctl stop offline-analysis-agent

# 备份配置
cp config.toml config.toml.bak

# 重新编译
cargo build --release

# 恢复配置
cp config.toml.bak config.toml

# 启动服务
systemctl start offline-analysis-agent
```

## 日志

```bash
# 查看实时日志（使用 systemd）
journalctl -u offline-analysis-agent -f

# 查看最近的错误
journalctl -u offline-analysis-agent -p err -n 50
```

## 性能调优

### 数据库连接池

在 `config.toml` 中调整：

```toml
[cache]
# 元数据缓存 TTL（秒）
metastore_ttl_seconds = 3600  # 默认 1 小时，可根据实际情况调整
```

### 并发优化

设置环境变量：

```bash
# 增加 Tokio 工作线程
export TOKIO_WORKER_THREADS=4

# 设置日志级别
export RUST_LOG=info
```
