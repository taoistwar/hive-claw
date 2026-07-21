# 环境变量

## 插件系统总开关

| 变量名 | 说明 | 默认值 |
|--------|------|--------|
| `PLUGIN_SYSTEM_ENABLED` | 插件系统总开关。`true`（默认）时 S3 必填、Plugin 上传/下载/调用及 `s3.*` capability 可用；`false` 时跳过 S3 客户端初始化、相关接口返回 503（业务码 5031）。合法值：`true`/`false`/`1`/`0`/`yes`/`no`/`off`（不区分大小写），空值等同 `true`。 | `true` |

## 后端环境变量

| 变量名 | 说明 | 默认值 |
|--------|------|--------|
| `HIVEWEB_HOST` | 服务器地址 | 127.0.0.1 |
| `HIVEWEB_PORT` | 服务器端口 | 3300 |
| `DATABASE_URL` | MySQL 连接字符串 | (必填) |
| `REDIS_MODE` | Redis 连接模式：`direct` 或 `sentinel` | direct |
| `REDIS_URL` | 直连模式的 Redis 连接字符串；Sentinel 模式忽略 | redis://127.0.0.1:6379 |
| `REDIS_SENTINEL_MASTER` | Sentinel 监控的 master 名称 | Sentinel 模式必填 |
| `REDIS_SENTINEL_NODES` | Sentinel 节点，使用逗号分隔的 `host:port` 列表 | Sentinel 模式必填 |
| `REDIS_DATABASE` | Sentinel 返回的数据节点数据库索引 | 0 |
| `REDIS_SENTINEL_REFRESH_MS` | Sentinel master 地址刷新间隔（毫秒） | 1000 |
| `REDIS_USERNAME` | Sentinel 返回的数据节点 ACL 用户名 | (可选) |
| `REDIS_PASSWORD` | Sentinel 返回的数据节点密码 | (可选) |
| `REDIS_SENTINEL_USERNAME` | Sentinel 服务自身的 ACL 用户名 | (可选) |
| `REDIS_SENTINEL_PASSWORD` | Sentinel 服务自身的密码 | (可选) |
| `REDIS_CONNECT_TIMEOUT_MS` | Redis 主节点发现和建连超时（毫秒） | 5000 |
| `AGENT_CACHE_TTL_SECS` | Agent 运行时内容 Redis 缓存 TTL（秒） | 300 |
| `JWT_SECRET` | JWT 密钥 | (必填，至少 32 字符) |
| `JWT_EXPIRATION_HOURS` | Token 有效期（小时） | 24 |
| `AWS_REGION` | S3 region | us-east-1（仅 `PLUGIN_SYSTEM_ENABLED=true` 时必填） |
| `AWS_ENDPOINT_URL` | S3 端点 | (可选；仅 `PLUGIN_SYSTEM_ENABLED=true` 时必填) |
| `AWS_ACCESS_KEY_ID` | S3 访问密钥 | (可选；仅 `PLUGIN_SYSTEM_ENABLED=true` 时必填) |
| `AWS_SECRET_ACCESS_KEY` | S3 密钥 | (可选；仅 `PLUGIN_SYSTEM_ENABLED=true` 时必填) |
| `S3_BUCKET` | S3 桶名 | (可选；仅 `PLUGIN_SYSTEM_ENABLED=true` 时必填) |

Sentinel 模式下，`REDIS_PASSWORD` 用于 Sentinel 返回的数据主节点，Sentinel
自身的认证信息必须使用 `REDIS_SENTINEL_USERNAME` 和
`REDIS_SENTINEL_PASSWORD`。hiveweb 缓存当前 master，并按
`REDIS_SENTINEL_REFRESH_MS` 定期刷新；刷新期间其他请求继续使用最近一次成功
发现的 master。单次运行期刷新最多等待 `REDIS_CONNECT_TIMEOUT_MS` 的一半，且
上限为 1 秒；超时后继续使用最近一次成功发现的 master。Sentinel 返回的地址
必须能从 hiveweb 所在主机或容器访问。
当前构建的 Direct 和 Sentinel 模式都只支持普通 TCP：Direct 使用
`redis://`；Sentinel 节点使用 `host:port` 或 `redis://`，不接受 `rediss://`。

## 前端环境变量

| 变量名 | 说明 | 默认值 |
|--------|------|--------|
| `VITE_API_URL` | 后端 API 地址 | http://localhost:3300 |
| `VITE_APP_TITLE` | 应用标题 | 管理中心 |
