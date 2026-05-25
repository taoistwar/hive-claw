# Quick Start: 管理中心

**Created**: 2026-05-25  
**Feature**: 管理中心 (003-admin-center)

## Prerequisites

- Rust 1.75+ (使用 `rustup install stable`)
- Node.js 18+ (使用 `nvm install 18`)
- npm 或 yarn
- Git

## Installation

### 1. 克隆仓库

```bash
git clone <repository-url>
cd hive-claw
git checkout 003-admin-center
```

### 2. 后端设置 (hiveweb)

```bash
# 进入后端目录
cd crates/hiveweb

# 安装依赖
cargo build

# 配置环境变量（可选，使用默认配置可跳过）
cp .env.example .env
# 编辑 .env 文件，配置数据库路径、JWT 密钥等

# 运行数据库迁移
cargo run --bin migrate

# 创建初始超级管理员
cargo run --bin create-super-admin -- \
  --phone "13800138000" \
  --password "admin123" \
  --nickname "Super Admin"

# 启动后端服务器
cargo run
# 默认运行在 http://localhost:3000
```

### 3. 前端设置 (web)

```bash
# 进入前端目录
cd web

# 安装依赖
npm install

# 配置环境变量（可选）
cp .env.example .env
# 编辑 .env 文件，配置 API 地址

# 启动开发服务器
npm run dev
# 默认运行在 http://localhost:5173
```

### 4. 访问应用

打开浏览器访问：http://localhost:5173

使用初始超级管理员账号登录：
- 手机号：13800138000
- 密码：admin123

## Project Structure

```
hive-claw/
├── crates/hiveweb/     # 后端 Rust 项目
│   ├── src/
│   │   ├── main.rs     # 入口文件
│   │   ├── models/     # 数据模型
│   │   ├── services/   # 业务逻辑
│   │   ├── api/        # API 路由
│   │   └── middleware/ # 中间件
│   └── migrations/     # 数据库迁移
└── web/                # 前端 React 项目
    ├── src/
    │   ├── components/ # React 组件
    │   ├── pages/      # 页面组件
    │   ├── services/   # API 调用
    │   └── hooks/      # 自定义 Hooks
    └── package.json
```

## Development

### 后端开发

```bash
# 运行测试
cargo test

# 代码格式化
cargo fmt

# 代码检查
cargo clippy -- -D warnings

# 查看日志
RUST_LOG=debug cargo run
```

### 前端开发

```bash
# 运行测试
npm test

# 代码格式化
npm run lint

# 类型检查
npm run type-check

# 生产构建
npm run build
```

## API Documentation

### 认证 API

#### POST /api/auth/login
登录

**Request**:
```json
{
  "phone": "13800138000",
  "password": "admin123"
}
```

**Response**:
```json
{
  "code": 0,
  "data": {
    "token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
    "refresh_token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
    "admin": {
      "id": 1,
      "phone": "13800138000",
      "nickname": "Super Admin",
      "role": 3,
      "status": 1
    }
  },
  "message": "登录成功"
}
```

#### GET /api/auth/me
获取当前登录管理员信息

**Headers**:
```
Authorization: Bearer <token>
```

**Response**:
```json
{
  "code": 0,
  "data": {
    "id": 1,
    "phone": "13800138000",
    "nickname": "Super Admin",
    "role": 3,
    "status": 1
  }
}
```

### 管理员管理 API

#### GET /api/admins
获取管理员列表

**Query Parameters**:
- `page`: 页码（默认 1）
- `page_size`: 每页数量（默认 10）

**Response**:
```json
{
  "code": 0,
  "data": {
    "total": 5,
    "admins": [
      {
        "id": 1,
        "phone": "13800138000",
        "nickname": "Super Admin",
        "role": 3,
        "status": 1,
        "created_at": "2026-05-25T10:00:00Z",
        "last_login_at": "2026-05-25T12:00:00Z"
      }
    ]
  }
}
```

#### POST /api/admins
添加管理员

**Request**:
```json
{
  "phone": "13900139000",
  "nickname": "New Admin",
  "password": "password123",
  "role": 1
}
```

#### PUT /api/admins/:id
修改管理员

**Request**:
```json
{
  "nickname": "Updated Nickname",
  "phone": "13900139000",
  "status": 1
}
```

#### DELETE /api/admins/:id
删除管理员

#### PATCH /api/admins/:id/status
禁用/启用管理员

**Request**:
```json
{
  "status": 0  // 0=禁用，1=启用
}
```

### 仪表盘 API

#### GET /api/dashboard/stats
获取统计数据

**Response**:
```json
{
  "code": 0,
  "data": {
    "total_admins": 5,
    "online_admins": 2,
    "today_logins": 15
  }
}
```

#### GET /api/dashboard/recent-logins
获取最近登录记录

**Response**:
```json
{
  "code": 0,
  "data": [
    {
      "id": 1,
      "admin_id": 1,
      "admin_nickname": "Super Admin",
      "login_at": "2026-05-25T12:00:00Z",
      "ip_address": "192.168.1.100",
      "success": true
    }
  ]
}
```

## Error Codes

- `1001`: 登录失败（密码错误）
- `1002`: 账户已被禁用
- `1003`: 账户已被锁定
- `1004`: Token 无效或过期
- `2001`: 权限不足
- `3001`: 管理员不存在
- `3002`: 手机号已存在
- `3003`: 不能删除超级管理员
- `3004`: 不能禁用最后一个超级管理员

## Troubleshooting

### 后端启动失败

**问题**: 数据库迁移失败
```
Error: database "admins" does not exist
```

**解决**:
```bash
# 确保数据库文件存在
mkdir -p ~/.local/share/hiveweb
cargo run --bin migrate
```

**问题**: 端口被占用
```
Error: Address already in use
```

**解决**:
```bash
# 修改端口
export HIVWEB_PORT=3001
cargo run
```

### 前端启动失败

**问题**: 依赖安装失败
```
Error: npm install failed
```

**解决**:
```bash
# 清理缓存
npm cache clean --force
rm -rf node_modules package-lock.json
npm install
```

**问题**: API 请求失败
```
Network Error: Unable to connect to backend
```

**解决**:
```bash
# 检查后端是否启动
curl http://localhost:3000/api/health

# 检查前端配置
cat web/.env
# 确保 VITE_API_URL=http://localhost:3000
```

## Environment Variables

### 后端 (.env)

```env
# 服务器配置
HIVWEB_HOST=127.0.0.1
HIVWEB_PORT=3000

# 数据库配置
DATABASE_URL=sqlite://~/.local/share/hiveweb/admins.db

# JWT 配置
JWT_SECRET=your-secret-key-change-in-production
JWT_EXPIRATION_HOURS=24
REFRESH_TOKEN_EXPIRATION_DAYS=7

# 日志配置
RUST_LOG=info,hiveweb=debug
```

### 前端 (.env)

```env
# API 地址
VITE_API_URL=http://localhost:3000

# 应用配置
VITE_APP_TITLE=管理中心
```

## Production Deployment

### 后端构建

```bash
# 生产环境构建
cargo build --release

# 运行迁移
./target/release/migrate

# 启动服务
./target/release/hiveweb
```

### 前端构建

```bash
# 生产环境构建
npm run build

# 部署 dist 目录到 Web 服务器
cp -r dist/* /var/www/admin-center/
```

### Nginx 配置示例

```nginx
server {
    listen 80;
    server_name admin.example.com;

    # 前端静态文件
    location / {
        root /var/www/admin-center;
        try_files $uri $uri/ /index.html;
    }

    # 后端 API 代理
    location /api/ {
        proxy_pass http://localhost:3000;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }
}
```

## Next Steps

1. 修改默认密码
2. 添加更多管理员账号
3. 配置生产环境
4. 设置 HTTPS
5. 配置日志轮转
6. 设置监控告警
