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

- [ ] T001 Create `web-user/package.json` with dependencies (react, antd, axios, react-router-dom, dayjs)
- [ ] T002 Create `web-user/tsconfig.json` and `web-user/tsconfig.node.json`
- [ ] T003 Create `web-user/vite.config.ts`
- [ ] T004 Create `web-user/index.html`
- [ ] T005 Create `web-user/.env.example` with `VITE_API_URL=http://localhost:3000`
- [ ] T006 Create `web-user/.eslintrc.cjs` and `web-user/.prettierrc.json`

---

## Phase 2: Core Infrastructure

### Utilities & Services

- [ ] T007 [P] Create `web-user/src/utils/auth.ts` — Token management (setToken, getToken, removeToken)
- [ ] T008 [P] Create `web-user/src/services/api.ts` — Axios client with base URL and interceptors
- [ ] T009 [P] Create `web-user/src/services/auth.ts` — User auth API (login, logout, me, changePassword)
- [ ] T010 [P] Create `web-user/src/services/chat.ts` — Chat session and message API with SSE support
- [ ] T011 [P] Create `web-user/src/services/dashboard.ts` — Dashboard stats API

### Hooks

- [ ] T012 [P] Create `web-user/src/hooks/useTheme.tsx` — Theme provider (dark/light) with tokens from web-admin
- [ ] T013 Create `web-user/src/hooks/useAuth.tsx` — Auth context for user login/logout/state
- [ ] T014 [P] Create `web-user/src/index.css` — Global styles (copy from web-admin)

---

## Phase 3: Authentication UI

### Login Flow

- [ ] T015 Create `web-user/src/components/LoginForm.tsx` — User login form (phone + password)
- [ ] T016 Create `web-user/src/pages/LoginPage.tsx` — Login page with animated background (simplified from web-admin)

---

## Phase 4: Layout & Navigation

- [ ] T017 Create `web-user/src/components/Layout.tsx` — App layout with simplified menu (Dashboard, Chat, Change Password), theme toggle, user dropdown

---

## Phase 5: Feature Pages

### Dashboard

- [ ] T018 Create `web-user/src/pages/DashboardPage.tsx` — User dashboard with stats cards

### Chat

- [ ] T019 Create `web-user/src/pages/ChatPage.tsx` — Chat page with session list, message history, SSE streaming (simplified from web-admin)

### Change Password

- [ ] T020 Create `web-user/src/pages/ChangePasswordPage.tsx` — Password change form

---

## Phase 6: App Routing & Integration

- [ ] T021 Create `web-user/src/main.tsx` — App entry point
- [ ] T022 Create `web-user/src/App.tsx` — Route configuration with auth guards
- [ ] T023 Update README.md — Add web-user setup instructions

---

## Phase 7: Validation

- [ ] T024 Run `cd web-user && npm install && npm run build` — Verify build succeeds
- [ ] T025 Run `cd web-user && npm run lint` — Verify no lint errors
