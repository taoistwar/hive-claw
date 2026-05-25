# Implementation Plan: 管理中心

**Branch**: `003-admin-center` | **Date**: 2026-05-25 | **Spec**: [specs/003-admin-center/spec.md](file:///home/developer/agent/hive-claw/specs/003-admin-center/spec.md)
**Input**: Feature specification from `/specs/003-admin-center/spec.md`

**Note**: This template is filled in by the `/speckit-plan` command. See `.specify/templates/plan-template.md` for the execution workflow.

## Summary

构建管理中心系统，采用前后端分离架构。后端使用 Rust (hiveweb crate) 提供 RESTful API，前端使用 React + TypeScript 构建管理界面。系统支持管理员登录、管理员账号管理（增删改查、禁用/启用）、角色权限控制（普通管理员、系统管理员、超级管理员）和仪表盘概览功能。数据库使用 MySQL 8.0+。

## Technical Context

**Language/Version**: Rust 1.75+ (backend), TypeScript 5.x (frontend)  
**Primary Dependencies**: axum (API), React (web), tokio (async runtime), mysql_async/SQLx (MySQL client)  
**Storage**: MySQL 8.0+ (InnoDB 引擎)  
**Testing**: cargo test (Rust), Vitest + Testing Library (React)  
**Target Platform**: Linux server (backend), Web browser (frontend)  
**Project Type**: Web application (前后端分离)  
**Performance Goals**: API p95 < 200ms, 支持 100+ 管理员账号，仪表盘加载 < 3 秒  
**Constraints**: 密码加密存储，会话管理安全，角色权限隔离 100% 准确  
**Scale/Scope**: 100+ 管理员账号，日登录次数 1000+，会话有效期 24 小时

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

✅ **Principle I - Code Quality & Maintainability**: 将通过 cargo fmt, cargo clippy, ESLint, Prettier 保证代码质量
✅ **Principle II - Test-First Development**: 将为登录、权限验证、管理员 CRUD 操作编写测试
✅ **Principle III - User Experience Consistency**: 统一错误提示格式、统一的 UI 组件库、一致的权限控制
✅ **Principle IV - Performance & Efficiency**: API p95 < 200ms，数据库查询使用索引，无 N+1 查询
✅ **Principle V - Simplicity & YAGNI**: 仅使用 axum + React，不引入过度抽象，状态管理优先使用 React hooks
✅ **Principle VI - Observability & Structured Logging**: 登录操作、权限变更、敏感操作记录结构化日志
✅ **Security Requirements**: 密码加密存储、输入验证、会话管理、登录失败锁定
✅ **Technology Stack**: Rust + axum (后端), TypeScript + React (前端), MySQL (存储) - 符合宪法规定

**Gate Result**: PASS - 所有宪法检查通过，无违例

## Project Structure

### Documentation (this feature)

```text
specs/003-admin-center/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
└── tasks.md             # Phase 2 output
```

### Source Code (repository root)

```text
crates/hiveweb/
├── src/
│   ├── main.rs              # 应用入口
│   ├── lib.rs               # 库导出
│   ├── models/
│   │   ├── mod.rs           # 模块定义
│   │   ├── admin.rs         # 管理员模型
│   │   ├── login_record.rs  # 登录记录模型
│   │   └── role.rs          # 角色权限模型
│   ├── services/
│   │   ├── mod.rs
│   │   ├── auth.rs          # 认证服务
│   │   ├── admin_service.rs # 管理员管理服务
│   │   └── dashboard.rs     # 仪表盘服务
│   ├── api/
│   │   ├── mod.rs
│   │   ├── auth.rs          # 认证 API
│   │   ├── admin.rs         # 管理员管理 API
│   │   └── dashboard.rs     # 仪表盘 API
│   ├── middleware/
│   │   ├── mod.rs
│   │   └── auth.rs          # 认证中间件
│   └── db/
│       ├── mod.rs
│       ├── connection.rs    # MySQL 连接池
│       └── migrations/      # 数据库迁移
web/
├── src/
│   ├── main.tsx             # 入口文件
│   ├── App.tsx              # 根组件
│   ├── components/
│   │   ├── Layout.tsx       # 布局组件
│   │   ├── LoginForm.tsx    # 登录表单
│   │   ├── AdminTable.tsx   # 管理员列表
│   │   ├── AdminForm.tsx    # 管理员表单
│   │   └── Dashboard.tsx    # 仪表盘
│   ├── pages/
│   │   ├── LoginPage.tsx    # 登录页
│   │   ├── AdminPage.tsx    # 管理员管理页
│   │   └── DashboardPage.tsx # 仪表盘页
│   ├── services/
│   │   ├── api.ts           # API 调用
│   │   ├── auth.ts          # 认证服务
│   │   └── admin.ts         # 管理员服务
│   ├── hooks/
│   │   ├── useAuth.ts       # 认证钩子
│   │   ── useAdmin.ts      # 管理员钩子
│   └── utils/
│       ├── auth.ts          # 认证工具
│       ── validators.ts    # 验证工具
── package.json
└── vite.config.ts
```

**Structure Decision**: 
- 后端：`crates/hiveweb/` - Rust workspace crate，使用 axum 提供 REST API
- 前端：`web/` - 独立的 npm 包，使用 Vite + React + TypeScript
- 数据库：MySQL 8.0+，使用连接池管理
- 符合宪法规定的双栈架构（Rust 后端 + React 前端）

## Complexity Tracking

> **Fill ONLY if Constitution Check has violations that must be justified**

无需填写 - Constitution Check 全部通过，无违例项

## Phase 0: Research

### Research Tasks

1. **研究 Rust axum 框架的认证授权最佳实践**
   - JWT vs Session 选择
   - 密码加密方案（bcrypt/argon2）
   - 中间件实现方式

2. **研究 React 权限管理方案**
   - 基于角色的路由控制
   - 组件级权限控制
   - 状态管理方案

3. **研究 MySQL 数据库设计**
   - 管理员表结构设计
   - 登录记录表设计
   - 索引优化策略
   - 连接池配置

4. **研究前端 UI 组件库**
   - Ant Design vs Material-UI
   - 表单验证方案
   - 表格组件选择

### Research Findings (research.md)

详见 [research.md](file:///home/developer/agent/hive-claw/specs/003-admin-center/research.md)

**关键决策**:
- 认证方案：JWT Token + 刷新 Token 机制
- 密码加密：bcrypt（Rust: `bcrypt` crate）
- 会话管理：前端存储 JWT，后端验证
- UI 组件库：Ant Design（功能丰富，适合后台管理）
- 数据库：MySQL 8.0+，使用连接池（max_connections = 20）
- 登录失败锁定：内存缓存 + 数据库持久化
- MySQL 客户端：SQLx（编译时 SQL 验证）

## Phase 1: Design

### Data Model (data-model.md)

详见 [data-model.md](file:///home/developer/agent/hive-claw/specs/003-admin-center/data-model.md)

**核心实体**:
1. **Admin (管理员)**
   - id: BIGINT (主键，自增)
   - phone: VARCHAR(11) (唯一)
   - nickname: VARCHAR(20)
   - password_hash: VARCHAR(60) (bcrypt 加密)
   - role: TINYINT (1=Normal, 2=System, 3=Super)
   - status: TINYINT (0=Disabled, 1=Active)
   - created_at: DATETIME
   - updated_at: DATETIME
   - last_login_at: DATETIME (NULL)

2. **LoginRecord (登录记录)**
   - id: BIGINT (主键，自增)
   - admin_id: BIGINT (外键)
   - login_at: DATETIME
   - ip_address: VARCHAR(45) (IPv4/IPv6)
   - success: BOOLEAN
   - failure_reason: VARCHAR(50) (NULL)

3. **RolePermission (角色权限)**
   - role: TINYINT
   - accessible_menus: Vec<String>
   - allowed_operations: Vec<String>

### API Contracts (contracts/)

详见 [contracts/](file:///home/developer/agent/hive-claw/specs/003-admin-center/contracts/)

**API Endpoints**:

1. **认证 API**
   - `POST /api/auth/login` - 管理员登录
   - `POST /api/auth/logout` - 登出
   - `GET /api/auth/me` - 获取当前管理员信息

2. **管理员管理 API**
   - `GET /api/admins` - 获取管理员列表
   - `POST /api/admins` - 添加管理员
   - `GET /api/admins/:id` - 获取管理员详情
   - `PUT /api/admins/:id` - 修改管理员
   - `DELETE /api/admins/:id` - 删除管理员
   - `PATCH /api/admins/:id/status` - 禁用/启用管理员

3. **仪表盘 API**
   - `GET /api/dashboard/stats` - 获取统计数据
   - `GET /api/dashboard/recent-logins` - 获取最近登录记录

### Quick Start Guide (quickstart.md)

详见 [quickstart.md](file:///home/developer/agent/hive-claw/specs/003-admin-center/quickstart.md)

**开发环境搭建**:
1. 安装 Rust (1.75+), Node.js (18+), MySQL 8.0+
2. 克隆仓库并切换到 003-admin-center 分支
3. 创建 MySQL 数据库：`CREATE DATABASE hiveweb CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;`
4. 后端：配置 DATABASE_URL，运行迁移 `cargo run --bin migrate`
5. 前端：`cd web && npm install && npm run dev`
6. 访问 http://localhost:5173

**初始化超级管理员**:
```bash
cargo run --bin create-super-admin -- --phone <手机号> --password <密码> --nickname <昵称>
```

### Agent Context Update

更新 `.trae/rules/project_rules.md` 中的 plan 引用（已更新）
