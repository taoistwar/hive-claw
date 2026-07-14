# 生产环境安装与上线

本文描述 `hiveweb` 后端和 `web-admin` 管理端的生产部署流程。构建方式另见
[deployment.md](../deployment.md)。

## 1. 部署拓扑

- `hiveweb`：HTTP API 服务并托管 `web-admin` 静态页面，仅监听 `127.0.0.1:3300`，
  由 Nginx 反向代理。
- `web-admin`：构建产物放在 `hiveweb` 二进制同级的 `dist/`，仅允许公司网络访问。
- MySQL 8.0+：业务元数据和管理端数据。
- Redis：登录锁定、限流及运行时缓存。
- Rustfs/S3：插件系统已关闭不需要。插件系统启用时必需；不使用插件时可关闭。
- 外部只读 MySQL：Assistant API 查询云电脑用户、会员和资产数据时必需。

不要将 MySQL、Redis、Rustfs/S3 或 `hiveweb:3300` 直接暴露到公网。

## 2. 上线前检查

目标服务器需要满足：

- Linux x86_64，系统架构和构建产物一致。
- 可访问 MySQL、Redis、LLM 服务；按功能需要访问外部只读数据库。
- 已安装 Nginx；仅在服务器本地构建时需要 Rust、Node.js 和 npm。
- 已准备独立的生产数据库账号、Redis和外部数据库只读账号。
- 已完成数据库备份，并记录当前应用版本和数据库 migration 版本。

生产密码、JWT 密钥、API Key 和数据库连接串不得写入 Git、构建日志或部署文档。

## 3. 构建产物

建议在 CI 或与生产环境兼容的 Linux 构建机上构建：

```bash
cargo build -p hiveweb --release \
  --bin hiveweb \
  --bin migrate \
  --bin create-super-admin

cd web-admin
npm ci
VITE_API_BASE_URL=/api npm run build
cd ..
```

需要部署的文件：

```text
target/release/hiveweb
target/release/migrate
target/release/create-super-admin
web-admin/dist/
crates/hiveweb/.env.example
crates/hiveweb/llm_presets.toml.example
```

如需使用 musl 静态构建，按照 [deployment.md](../deployment.md) 构建并充分验证
Extism/Wasmtime、TLS 和目标系统兼容性。

## 4. 目录与运行用户

```bash
sudo useradd --system --home /opt/hive-claw --shell /usr/sbin/nologin hiveclaw
sudo install -d -o hiveclaw -g hiveclaw /opt/hive-claw/bin
sudo install -d -o hiveclaw -g hiveclaw /opt/hive-claw/bin/dist
sudo install -d -o hiveclaw -g hiveclaw /opt/hive-claw/config
sudo install -d -o hiveclaw -g hiveclaw /opt/hive-claw/logs

sudo install -m 0755 target/release/hiveweb /opt/hive-claw/bin/
sudo install -m 0755 target/release/migrate /opt/hive-claw/bin/
sudo install -m 0755 target/release/create-super-admin /opt/hive-claw/bin/
sudo cp -a web-admin/dist/. /opt/hive-claw/bin/dist/
```

部署新版本时先上传到临时文件，再原子替换二进制，避免进程读取到不完整文件。

## 5. 环境配置

以 `crates/hiveweb/.env.example` 为基线创建 `/opt/hive-claw/.env`：

```bash
sudo install -o root -g hiveclaw -m 0640 \
  crates/hiveweb/.env.example /opt/hive-claw/.env
sudo editor /opt/hive-claw/.env
```

至少检查以下配置：

```dotenv
APP_ENV=production
HIVEWEB_HOST=127.0.0.1
HIVEWEB_PORT=3300
LOG_DIR=/opt/hive-claw/logs

DATABASE_URL=mysql://<user>:<password>@<host>:3306/hiveweb
REDIS_MODE=direct
REDIS_URL=redis://:<password>@<host>:6379
REDIS_CONNECT_TIMEOUT_MS=5000

JWT_SECRET=<至少32字节的随机值>
ASSISTANT_SECRET=<独立的随机值>
JWT_EXPIRES_SEC=86400
RATE_LIMIT_MAX=180
RATE_LIMIT_WINDOW_SECS=60

CORS_ALLOWED_ORIGINS=https://<管理端域名>
LLM_PRESETS_PATH=/opt/hive-claw/config/llm_presets.toml
```

可使用 `openssl rand -hex 32` 生成独立随机密钥。`JWT_SECRET` 与
`ASSISTANT_SECRET` 不应复用。

### Redis Sentinel

生产 Redis 由 Sentinel 管理时，将上面的直连配置替换为：

```dotenv
REDIS_MODE=sentinel
REDIS_SENTINEL_MASTER=mymaster
REDIS_SENTINEL_NODES=<sentinel-1>:26379,<sentinel-2>:26379,<sentinel-3>:26379
REDIS_DATABASE=0
REDIS_SENTINEL_REFRESH_MS=1000
REDIS_USERNAME=<data-node-acl-user>
REDIS_PASSWORD=<data-node-password>
REDIS_CONNECT_TIMEOUT_MS=5000

# 仅当 Sentinel 服务自身也启用了认证时配置
REDIS_SENTINEL_USERNAME=<sentinel-acl-user>
REDIS_SENTINEL_PASSWORD=<sentinel-password>
```

Sentinel 模式忽略 `REDIS_URL`。`REDIS_USERNAME` 和 `REDIS_PASSWORD` 用于
Sentinel 返回的数据主节点；Sentinel 自身的认证信息使用
`REDIS_SENTINEL_USERNAME` 和 `REDIS_SENTINEL_PASSWORD`。建议列出所有
Sentinel 节点，避免单个 Sentinel 不可用导致启动失败。

hiveweb 启动时会发现当前 master、建立连接并执行 `PING`；运行期间缓存当前
master，并按 `REDIS_SENTINEL_REFRESH_MS` 定期刷新。刷新期间其他请求继续使用
最近一次成功发现的 master，不会排队等待 Sentinel。单次运行期刷新最多等待
`REDIS_CONNECT_TIMEOUT_MS` 的一半，且上限为 1 秒；如果部署环境中 Sentinel
发现与 Redis `ROLE` 校验通常超过 1 秒，应先排查网络延迟。请确认 Sentinel
返回的 master 地址能从 hiveweb 所在主机或容器解析并访问。若 Redis 位于容器
或 NAT 后，应同时检查 Redis/Sentinel 的 announce 地址和端口配置。
当前 Sentinel 模式仅支持普通 TCP，不接受 `rediss://` 地址。

### 插件系统

启用插件系统时配置：

```dotenv
PLUGIN_SYSTEM_ENABLED=true
AWS_REGION=us-east-1
AWS_ENDPOINT_URL=https://<rustfs-or-s3-endpoint>
AWS_ACCESS_KEY_ID=<access-key>
AWS_SECRET_ACCESS_KEY=<secret-key>
S3_BUCKET=hiveclaw
```

不使用插件系统时显式设置：

```dotenv
PLUGIN_SYSTEM_ENABLED=false
```

关闭后插件上传、下载、调用以及 `s3.*` capability 将不可用，但无需配置 S3。

### 外部 Assistant 数据库

需要 Assistant API 时配置 `EXTERNAL_DB_URL`。该账号必须只有业务所需的只读权限。
外部库的 `DATETIME` 字段按东八区保存时，应明确设置 SQLx 会话时区：

```dotenv
EXTERNAL_DB_URL=mysql://<readonly-user>:<password>@<host>:3306/cloud_computer?timezone=%2B08:00
```

URL 中的特殊字符必须进行百分号编码。不配置外部数据库时，依赖它的 Assistant
接口不可用。

## 6. LLM 配置

```bash
sudo install -o root -g hiveclaw -m 0640 \
  crates/hiveweb/llm_presets.toml.example \
  /opt/hive-claw/config/llm_presets.toml
sudo editor /opt/hive-claw/config/llm_presets.toml
```

要求：

- 必须有且仅有一个 `default = true` 的 preset。
- `kind`、`base_url`、`model` 必须与实际供应商一致。
- API Key 的环境变量名由每个 provider 的 `api_key_env` 指定，并非固定为
  `LLM_API_KEY`。例如 `api_key_env = "OPENAI_API_KEY"` 时，需要在 `.env` 中配置
  `OPENAI_API_KEY=...`。
- 配置文件只引用环境变量名，不要直接写 API Key。

## 7. 数据库迁移

`hiveweb` 主进程不会自动建表或执行 migration。生产上线必须先备份数据库，再运行
随同当前版本构建的 `migrate` 二进制。不要以手工导入单个 `db.sql` 替代 migration。

`docs/prod/hiveweb.sql` 是环境导出文件，可能包含管理员密码哈希、业务配置和已有
migration 记录，不是生产建库的权威来源。含真实数据的导出文件不得提交到 Git；如需
用作受控备份，应加密存放并限制访问。

```bash
cd /opt/hive-claw
sudo -u hiveclaw ./bin/migrate
```

迁移记录保存在 `schema_migrations` 表，命令可重复运行，已执行版本会跳过。迁移失败时
不要启动新版本；先保留日志并恢复数据库或修复问题。不要手工删除 migration 记录。

## 8. 创建首个超级管理员

首次部署时执行：

```bash
cd /opt/hive-claw
sudo -u hiveclaw ./bin/create-super-admin \
  --phone '<11位手机号>' \
  --password '<生产专用强密码>' \
  --nickname '<管理员昵称>'
```

密码会以 bcrypt 哈希写入 `admins.password_hash`。不要复制测试环境的默认管理员密码。
命令参数可能出现在 shell 历史或短暂出现在进程列表中，执行后应清理历史记录，并在
首次登录后通过修改密码接口轮换。重复手机号只更新昵称，不会重置已有密码。

## 9. systemd 服务

创建 `/etc/systemd/system/hiveweb.service`：

```ini
[Unit]
Description=HiveClaw Web API
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=hiveclaw
Group=hiveclaw
WorkingDirectory=/opt/hive-claw
ExecStart=/opt/hive-claw/bin/hiveweb --mode production
Restart=on-failure
RestartSec=5
LimitNOFILE=65535
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

程序会从 `WorkingDirectory` 加载 `.env`；这里不再使用 systemd `EnvironmentFile`，避免
systemd 与 dotenv 对引号、转义及行内注释的解析差异。

启动并查看状态：

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now hiveweb
sudo systemctl status hiveweb --no-pager
sudo journalctl -u hiveweb -n 200 --no-pager
sudo tail -n 200 /opt/hive-claw/logs/hiveweb.log.*
```

服务启动失败时优先检查数据库、Redis、LLM 配置文件、S3 配置和日志目录权限。

管理端仅挂载在 `/web-admin`，根路径不会返回管理页面。该路径本身不替代访问控制，
仍需通过 Nginx 和防火墙限制管理端来源网络。

## 10. Nginx 与内网管理端

管理端应使用独立的内网域名，并在 Nginx 和防火墙两层限制公司网段。示例中的网段、
域名和证书路径必须替换：

```nginx
server {
    listen 443 ssl http2;
    server_name hive-admin.example.internal;

    ssl_certificate     /etc/nginx/tls/hive-admin.crt;
    ssl_certificate_key /etc/nginx/tls/hive-admin.key;

    allow 10.0.0.0/8;
    deny all;

    location / {
        proxy_pass http://127.0.0.1:3300;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 60s;
        proxy_buffering off;
    }
}
```

```bash
sudo nginx -t
sudo systemctl reload nginx
```

如需对公网开放 Assistant API，应使用独立域名和单独的 Nginx `server`，只放行明确
需要的公开路由；不要把管理端静态页面和全部 `/api/` 管理接口一起暴露。

## 11. 上线验收

```bash
# 后端端口只应监听本机
ss -lntp | grep ':3300'

# Liveness：只检查进程和 HTTP 事件循环
curl --fail --max-time 3 https://hive-admin.example.internal/health/live

# Readiness：检查主 MySQL 和 Redis；依赖异常时返回 503
curl --fail --max-time 5 https://hive-admin.example.internal/health/ready

# 检查最近错误
sudo journalctl -u hiveweb --since '10 minutes ago' --no-pager
grep -iE 'error|panic' /opt/hive-claw/logs/hiveweb.log.* | tail -n 100
```

还应人工验证：

1. 超级管理员登录、退出和修改密码。
2. 管理端列表页面和需要的 CRUD 操作。
3. LLM 默认 preset 和 fallback 链。
4. Assistant API 签名、外部用户、会员及资产查询。
5. 启用插件系统时的上传、调用和 S3 访问。
6. CORS、内网访问控制、限流和登录锁定策略。
7. 日志不包含密码、Token、API Key 或完整请求正文。

## 12. 更新与回滚

更新顺序：

1. 备份数据库和当前二进制、配置文件。
2. 上传新二进制和前端静态文件。
3. 运行新版本 `migrate`。
4. 重启 `hiveweb`，再原子切换前端目录或 Nginx 配置。
5. 完成上线验收后再清理旧产物。

应用回滚可以恢复旧二进制和静态文件；数据库 migration 默认按前向兼容处理，不要在
没有专项回滚脚本和备份的情况下反向执行 DDL。若新版本依赖不可逆 schema 变更，应在
上线前单独制定数据库回滚方案。
