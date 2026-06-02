# HiveClaw

管理中心 & 用户中心系统 - 前后端分离架构

## 技术栈

### 后端
- **语言**: Rust (stable)
- **框架**: axum (HTTP), tokio (async)
- **数据库**: MySQL 8.0+
- **缓存**: Redis 7+
- **对象存储**: Rustfs/S3

### 前端
- **语言**: TypeScript
- **框架**: React 18
- **UI 库**: Ant Design 5
- **构建工具**: Vite 5
- **路由**: React Router 6

## 快速开始

### 前置条件

1. Rust 1.85+ (`rustup install stable`)
2. Node.js 18+ (`nvm install 18`)
3. MySQL 8.0+
4. Redis 7+
5. (可选) MinIO 或 S3 兼容存储

### 1. 设置数据库

```bash
# 创建 MySQL 数据库
mysql -u root -p -e "CREATE DATABASE hiveweb CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;"

# 启动 Redis
redis-server
```

### 2. 后端设置

```bash
cd crates/hiveweb

# 复制环境变量文件
cp .env.example .env
# 编辑 .env 文件，配置数据库连接

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

### 3. 管理中心前端 (web-admin)

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

### 4. 用户中心前端 (web-user)

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

## 项目结构

```
hive-claw/
├── crates/hiveweb/       # 后端 Rust 项目
│   ├── src/
│   │   ├── main.rs       # 应用入口
│   │   ├── lib.rs        # 库导出
│   │   ├── api/          # API 路由
│   │   ├── models/       # 数据模型
│   │   ├── services/     # 业务逻辑
│   │   ├── middleware/   # 中间件
│   │   ├── db/           # 数据库连接
│   │   ├── cache/        # Redis 缓存
│   │   ├── storage/      # S3 存储
│   │   └── utils/        # 工具函数
│   ├── migrations/       # 数据库迁移
│   └── Cargo.toml
└── web-admin/                  # 前端 React 项目
    ├── src/
    │   ├── main.tsx      # 入口文件
    │   ├── App.tsx       # 根组件
    │   ├── components/   # React 组件
    │   ├── pages/        # 页面组件
    │   ├── services/     # API 调用
    │   ├── hooks/        # 自定义 Hooks
    │   └── utils/        # 工具函数
    └── package.json
```

## 功能特性

- ✅ 管理员登录（JWT 认证）
- ✅ 管理员账号管理（增删改查）
- ✅ 角色权限控制（普通/系统/超级管理员）
- ✅ 仪表盘概览
- ✅ 登录失败锁定（Redis）
- ✅ 会话管理（24 小时有效期）
- ✅ 文件上传（S3 存储）

## API 文档

详见 [specs/003-admin-center/contracts/api.md](specs/003-admin-center/contracts/api.md)

## 开发指南

### 后端开发

```bash
# 运行测试
cargo test

# 代码格式化
cargo fmt

# 代码检查
cargo clippy -- -D warnings
```

### 前端开发

```bash
# 运行测试
npm test

# 代码格式化
npm run format

# 代码检查
npm run lint
```

## 部署

### 生产环境构建

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

## 环境变量

### 后端环境变量

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

### 前端环境变量

| 变量名 | 说明 | 默认值 |
|--------|------|--------|
| `VITE_API_URL` | 后端 API 地址 | http://localhost:3300 |
| `VITE_APP_TITLE` | 应用标题 | 管理中心 |

## 常见问题

### 数据库连接失败

确保 MySQL 正在运行并且数据库已创建：

```bash
mysql -u root -p -e "SHOW DATABASES LIKE 'hiveweb';"
```

### Redis 连接失败

确保 Redis 正在运行：

```bash
redis-cli ping
# 应该返回 PONG
```

### 端口被占用

修改端口：

```bash
export HIVEWEB_PORT=3001
cargo run
```

## Dev 环境

### Redis

```bash
docker run -d --restart=always --name agent_redis_16379 -p 16379:6379 redis redis-server --requirepass "ai123456"
```

### Rustfs

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
docker run -d --restart=always --name agent_mysql_stack -p 33060:3306 -e MYSQL_ROOT_PASSWORD='ai123456'  mysql:5.7.41
```

## 许可证

MIT
