# Implementation Plan: User Center (web-user)

**Feature**: 005-user-center
**Branch**: `005-user-center`
**Date**: 2026-05-31

## 1. Technical Context

- **Language**: TypeScript
- **Framework**: React 18
- **UI Library**: Ant Design 5
- **Build Tool**: Vite 5
- **Router**: React Router 6
- **HTTP Client**: Axios

## 2. Project Structure

```
web-user/
├── src/
│   ├── main.tsx
│   ├── App.tsx
│   ├── index.css
│   ├── components/
│   │   ├── Layout.tsx
│   │   └── LoginForm.tsx
│   ├── pages/
│   │   ├── LoginPage.tsx
│   │   ├── DashboardPage.tsx
│   │   ├── ChatPage.tsx
│   │   └── ChangePasswordPage.tsx
│   ├── services/
│   │   ├── api.ts
│   │   ├── auth.ts
│   │   ├── chat.ts
│   │   └── dashboard.ts
│   ├── hooks/
│   │   ├── useAuth.tsx
│   │   └── useTheme.tsx
│   └── utils/
│       └── auth.ts
├── package.json
├── tsconfig.json
├── tsconfig.node.json
├── vite.config.ts
├── .env.example
├── .eslintrc.cjs
├── .prettierrc.json
└── index.html
```

## 3. API Contracts

### Authentication
- `POST /api/users/login` — 用户登录
- `POST /api/users/logout` — 用户登出
- `GET /api/users/me` — 获取当前用户信息
- `POST /api/users/change-password` — 修改密码

### Chat
- `POST /api/chat/sessions` — 创建会话
- `GET /api/chat/sessions` — 获取会话列表
- `POST /api/chat/sessions/:id/messages` — 发送消息（SSE）

### Dashboard
- `GET /api/users/dashboard/stats` — 获取用户统计

## 4. Key Decisions

1. **复用 web-admin 代码模式**: 所有组件、服务、hooks 的结构和实现方式与 web-admin 保持一致
2. **独立项目**: web-user 是完全独立的 Vite 项目，不共享代码（避免耦合）
3. **共享后端 API**: 复用 hiveweb 后端已有的 /api/users/* 和 /api/chat/* 端点
4. **简化 Layout**: 用户中心 Layout 仅包含 Dashboard、Chat、修改密码 三个菜单项
5. **主题系统**: 完全复用 web-admin 的 dark/light 主题 tokens

## 5. Dependencies

- Backend endpoints must exist in `crates/hiveweb/src/api/user.rs`
- Chat endpoints exist in `crates/hiveweb/src/api/chat.rs`
