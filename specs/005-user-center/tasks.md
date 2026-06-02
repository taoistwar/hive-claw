# Tasks: User Center (web-user)

**Input**: Design documents from `/specs/005-user-center/`
**Prerequisites**: spec.md, plan.md, existing web-admin implementation as reference

**Organization**: Tasks are grouped by functional area to enable independent implementation.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this belongs to
- Include exact file paths in descriptions

---

## Phase 1: Project Setup

### Project Initialization

- [x] T001 Create `web-user/package.json` with dependencies (react, antd, axios, react-router-dom, dayjs)
- [x] T002 Create `web-user/tsconfig.json` and `web-user/tsconfig.node.json`
- [x] T003 Create `web-user/vite.config.ts`
- [x] T004 Create `web-user/index.html`
- [x] T005 Create `web-user/.env.example` with `VITE_API_URL=http://localhost:3300`
- [x] T006 Create `web-user/.gitignore`

---

## Phase 2: Core Infrastructure

### Utilities & Services

- [x] T007 [P] Create `web-user/src/utils/auth.ts` — Token management (setToken, getToken, removeToken)
- [x] T008 [P] Create `web-user/src/services/api.ts` — Axios client with base URL and interceptors
- [x] T009 [P] Create `web-user/src/services/auth.ts` — User auth API (login, logout, me, changePassword)
- [x] T010 [P] Create `web-user/src/services/chat.ts` — Chat session and message API with SSE support
- [x] T011 [P] Create `web-user/src/services/dashboard.ts` — Dashboard stats API

### Hooks

- [x] T012 [P] Create `web-user/src/hooks/useTheme.tsx` — Theme provider (dark/light) with tokens from web-admin
- [x] T013 Create `web-user/src/hooks/useAuth.tsx` — Auth context for user login/logout/state
- [x] T014 [P] Create `web-user/src/index.css` — Global styles (copy from web-admin)

---

## Phase 3: Authentication UI

### Login Flow

- [x] T015 Create `web-user/src/components/LoginForm.tsx` — User login form (phone + password)
- [x] T016 Create `web-user/src/pages/LoginPage.tsx` — Login page with animated background (simplified from web-admin)

---

## Phase 4: Layout & Navigation

- [x] T017 Create `web-user/src/components/Layout.tsx` — App layout with simplified menu (Dashboard, Chat, Change Password), theme toggle, user dropdown

---

## Phase 5: Feature Pages

### Dashboard

- [x] T018 Create `web-user/src/pages/DashboardPage.tsx` — User dashboard with stats cards

### Chat

- [x] T019 Create `web-user/src/pages/ChatPage.tsx` — Chat page with session list, message history, SSE streaming (simplified from web-admin)

### Change Password

- [x] T020 Create `web-user/src/pages/ChangePasswordPage.tsx` — Password change form

---

## Phase 6: App Routing & Integration

- [x] T021 Create `web-user/src/main.tsx` — App entry point
- [x] T022 Create `web-user/src/App.tsx` — Route configuration with auth guards
- [x] T023 Update README.md — Add web-user setup instructions

---

## Phase 7: Validation

- [x] T024 Run `cd web-user && npm install && npm run build` — Verify build succeeds
- [x] T025 Run `cd web-user && npm run lint` — Verify no lint errors
