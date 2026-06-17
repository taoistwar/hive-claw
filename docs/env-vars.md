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
| `REDIS_URL` | Redis 连接字符串 | redis://127.0.0.1:6379 |
| `JWT_SECRET` | JWT 密钥 | (必填，至少 32 字符) |
| `JWT_EXPIRATION_HOURS` | Token 有效期（小时） | 24 |
| `AWS_REGION` | S3 region | us-east-1（仅 `PLUGIN_SYSTEM_ENABLED=true` 时必填） |
| `AWS_ENDPOINT_URL` | S3 端点 | (可选；仅 `PLUGIN_SYSTEM_ENABLED=true` 时必填) |
| `AWS_ACCESS_KEY_ID` | S3 访问密钥 | (可选；仅 `PLUGIN_SYSTEM_ENABLED=true` 时必填) |
| `AWS_SECRET_ACCESS_KEY` | S3 密钥 | (可选；仅 `PLUGIN_SYSTEM_ENABLED=true` 时必填) |
| `S3_BUCKET` | S3 桶名 | (可选；仅 `PLUGIN_SYSTEM_ENABLED=true` 时必填) |

## 前端环境变量

| 变量名 | 说明 | 默认值 |
|--------|------|--------|
| `VITE_API_URL` | 后端 API 地址 | http://localhost:3300 |
| `VITE_APP_TITLE` | 应用标题 | 管理中心 |
