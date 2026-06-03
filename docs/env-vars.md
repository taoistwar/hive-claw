# 环境变量

## 后端环境变量

| 变量名 | 说明 | 默认值 |
|--------|------|--------|
| `HIVEWEB_HOST` | 服务器地址 | 127.0.0.1 |
| `HIVEWEB_PORT` | 服务器端口 | 3300 |
| `DATABASE_URL` | MySQL 连接字符串 | (必填) |
| `REDIS_URL` | Redis 连接字符串 | redis://127.0.0.1:6379 |
| `JWT_SECRET` | JWT 密钥 | (必填，至少 32 字符) |
| `JWT_EXPIRATION_HOURS` | Token 有效期（小时） | 24 |
| `S3_ENDPOINT` | S3 端点 | (可选) |
| `S3_ACCESS_KEY_ID` | S3 访问密钥 | (可选) |
| `S3_SECRET_ACCESS_KEY` | S3 密钥 | (可选) |
| `S3_BUCKET` | S3 桶名 | (可选) |

## 前端环境变量

| 变量名 | 说明 | 默认值 |
|--------|------|--------|
| `VITE_API_URL` | 后端 API 地址 | http://localhost:3300 |
| `VITE_APP_TITLE` | 应用标题 | 管理中心 |
