# Feature: 用户中心 (User Center)

**Feature Branch**: `005-user-center`
**Created**: 2026-05-31
**Status**: Draft
**Input**: 仿照 web-admin 模式，创建独立的用户中心前端应用 web-user

## Background

当前管理中心 (web-admin) 仅限管理员使用。需要创建一个面向普通用户的用户中心 (web-user)，让注册的用户能够：
- 登录用户中心
- 查看个人仪表盘
- 修改密码
- 使用聊天功能
- 管理个人资料

用户中心使用 `users` 表进行认证（区别于管理中心的 `admins` 表），复用后端已有的 `/api/users/*` 端点。

## Requirements

### 用户认证
- 用户使用手机号 + 密码登录 (`POST /api/users/login`)
- 登录成功后返回 JWT token
- Token 通过 `Authorization: Bearer <token>` 传递
- 登出 (`POST /api/users/logout`)
- 获取当前用户信息 (`GET /api/users/me`)

### 功能模块
1. **Dashboard** — 显示用户统计信息（会话数、消息数等）
2. **Chat** — 与 Agent 对话（复用管理中心的聊天 UI 和 API）
3. **修改密码** — 修改当前用户密码 (`POST /api/users/change-password`)

### 非功能性
- 与 web-admin 相同的主题系统（dark/light mode）
- 响应式设计
- 相同的 Ant Design 5 + Vite 5 + React 18 技术栈

## Architecture

### 前端架构
```
web-user/
├── src/
│   ├── main.tsx              # 入口文件
│   ├── App.tsx               # 路由配置
│   ├── index.css             # 全局样式
│   ├── components/
│   │   ├── Layout.tsx        # 布局组件（简化版，无管理菜单）
│   │   └── LoginForm.tsx     # 用户登录表单
│   ├── pages/
│   │   ├── LoginPage.tsx     # 登录页
│   │   ├── DashboardPage.tsx # 仪表盘
│   │   ├── ChatPage.tsx      # 聊天页
│   │   └── ChangePasswordPage.tsx # 修改密码页
│   ├── services/
│   │   ├── api.ts            # API 客户端
│   │   ├── auth.ts           # 认证服务
│   │   ├── chat.ts           # 聊天服务
│   │   └── dashboard.ts      # 仪表盘服务
│   ├── hooks/
│   │   ├── useAuth.tsx       # 认证上下文
│   │   └── useTheme.tsx      # 主题上下文
│   └── utils/
│       └── auth.ts           # Token 管理工具
├── package.json
├── tsconfig.json
├── vite.config.ts
└── .env.example
```

### 后端端点复用
所有 API 端点复用 `crates/hiveweb/src/api/user.rs` 和 `crates/hiveweb/src/api/users.rs` 中已实现的端点。

## Error Codes

复用已有用户认证错误码：
- `1001` WRONG_PASSWORD — 密码错误
- `1002` ACCOUNT_DISABLED — 账户已禁用
- `1003` ACCOUNT_LOCKED — 账户已锁定
- `2001` TOKEN_EXPIRED — Token 已过期
- `2002` TOKEN_INVALID — Token 无效

## Out of Scope
- 用户注册（当前假设用户由管理员创建或通过其他方式注册）
- 用户管理其他用户
- 管理功能（Agent、Plugin、Workflow 等）
