# Implementation Plan: 管理中心

**Branch**: `003-admin-center` | **Date**: 2026-05-25 | **Spec**: [specs/003-admin-center/spec.md](file:///home/developer/agent/hive-claw/specs/003-admin-center/spec.md)
**Input**: Feature specification from `/specs/003-admin-center/spec.md`

**Note**: This template is filled in by the `/speckit-plan` command. See `.specify/templates/plan-template.md` for the execution workflow.

## Summary

构建管理中心系统，采用前后端分离架构。后端使用 Rust (hiveweb crate) 提供 RESTful API，前端使用 React + TypeScript 构建管理界面。系统支持管理员登录、管理员账号管理（增删改查、禁用/启用）、角色权限控制（普通管理员、系统管理员、超级管理员）和仪表盘概览功能。技术栈：MySQL 8.0+ (数据库)、Redis 7+ (缓存)、Rustfs (S3 对象存储)。

## Technical Context

**Language/Version**: Rust 1.85+ (backend), TypeScript 5.x (frontend)
**Primary Dependencies**: axum (API), React (web), tokio (async runtime), SQLx (MySQL client), redis (Redis client)
**Storage**: MySQL 8.0+ (InnoDB 引擎), Redis 7+ (缓存), Rustfs/S3 (对象存储)
**Testing**: cargo test (Rust), Vitest + Testing Library (React)
**Target Platform**: Linux server (backend), Web browser (frontend)
**Project Type**: Web application (前后端分离)
**Performance Goals**: API p95 < 200ms, 支持 100+ 管理员账号，仪表盘加载 < 3 秒
**Constraints**: 密码加密存储，会话管理安全，角色权限隔离 100% 准确
**Scale/Scope**: 100+ 管理员账号，日登录次数 1000+，会话有效期 24 小时

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

✅ **Principle I - Code Quality & Maintainability**: 将通过 cargo fmt, cargo clippy, ESLint, Prettier 保证代码质量
✅ **Principle II - Test-First Development**: 追溯补测试已完成 — Phase 2.5 全部 13 项红灯测试（T026a–T026m）转绿；后端 33/33、前端 16/16。详见 Complexity Tracking 偏离 1（已退出）。
✅ **Principle III - User Experience Consistency**: a11y（键盘导航 / WCAG 2.1 AA / axe-core）补充至 FR-020；T094 axe 3/3 全绿（SC-008 = 0 critical/serious），T095 ARIA label 落实
✅ **Principle IV - Performance & Efficiency**: 实测 admin list / dashboard p95 < 20ms（≪ SC-002/SC-003 预算）；EXPLAIN 证据于 perf-evidence.md；login p95 = 884ms 因 bcrypt 加密成本登记为偏离 4
✅ **Principle V - Simplicity & YAGNI**: 仅使用 axum + React，不引入过度抽象，状态管理优先使用 React hooks
✅ **Principle VI - Observability & Structured Logging**: request_id / JSON 日志 / PII 遮码 落实（T092 中间件 + utils/logging::mask_phone）；审计日志 V006 表（T093）
✅ **Security Requirements**: 密码加密存储、输入验证、会话管理、登录失败锁定
✅ **Technology Stack**: Rust + axum (后端), TypeScript + React (前端), MySQL (数据库), Redis (缓存), Rustfs/S3 (对象存储) - 符合宪法 v1.3.0 规定

**Gate Result**: PASS — 全部 6 项原则合规。Login 端点的 bcrypt 成本超 Principle IV 预算属可接受偏离（详见 Complexity Tracking 偏离 4），不影响整体 gate。

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

### 偏离 1 — Principle II (Test-First, NON-NEGOTIABLE) — **已退出 (2026-05-26)**

| 项 | 内容 |
| --- | --- |
| 现状（历史） | 实现代码（T027–T091）先于测试编写完成；最初的 tasks.md L11 声明 "Tests are OPTIONAL"，未生成测试任务 |
| 偏离类型 | 流程偏离（顺序违反 Red → Green → Refactor） |
| 触发原因 | 任务模板默认未启用测试；MVP 优先交付期内未触发测试补全闭环 |
| 风险 | 回归保护薄弱；契约 / RBAC 等关键路径缺乏自动化验证 |
| 缓解执行 | Phase 2.5 T026a–T026m 13 项追溯红灯测试已全部转绿 + 触发了 7+ 个真实后端 bug 修复（is_account_locked TTL、create_login_record 列名错位、create_admin/update_admin fetch_one on INSERT、PUT vs PATCH 状态路由、Normal-role 拒列表、System-role 可删除、login envelope unwrap 等） |
| 退出依据 | 后端 cargo test 33/33 全绿、前端 npm test 16/16 全绿（含 3 个 a11y）；commits 5e0fa9d → 31ae8f2 |
| 替代方案（已否决） | (a) 完整推翻已实现代码并重新 TDD — 工作量大且与现有产出冲突；(b) 维持 Tests Optional 并永久豁免 — 直接违反宪法 NON-NEGOTIABLE 条款 |

### 偏离 2 — Principle III (a11y 未在初版 spec 体现) — **已退出 (2026-05-26)**

| 项 | 内容 |
| --- | --- |
| 现状（历史） | 初版 spec.md 未提及键盘导航 / 对比度 / ARIA；实现使用 Ant Design 默认能力但未验证 |
| 缓解执行 | spec.md 增 FR-020 / SC-008；T094 axe-core 集成 vitest，覆盖 LoginForm / AdminTable / PermissionGuard 三组件；T095 给 AdminTable filter 区的 5 个控件（2 × Select、Input、2 × RangePicker）补 aria-label + Form.Item label |
| 退出依据 | a11y.test.tsx 3/3 全绿，0 critical/serious 违规 |

### 偏离 3 — Principle VI (结构化日志 / PII 未在初版 spec 体现) — **已退出 (2026-05-26)**

| 项 | 内容 |
| --- | --- |
| 现状（历史） | 初版 spec 仅泛泛要求"日志"；T083 已引入 tracing，但未规定 request_id / JSON 格式 / 手机号遮码 |
| 缓解执行 | spec.md 增 FR-021 / FR-022 / SC-009；T092 落地：middleware/request_id.rs（UUID 透传 + X-Request-Id 响应头 + tracing span 跨 .await 用 .instrument() 保持）、utils/logging::mask_phone（138****8000）、api/auth.rs 登录 outcome 结构化 info 日志；T093 落地：V006 audit_logs 表、services/audit.rs::record()、admin CRUD 四个 handler 接入 |
| 退出依据 | 后端 33/33 测试通过（含 utils/logging 与 utils/validation 内置 unit tests） |

### 偏离 4 — Principle IV / POST /api/auth/login 超 200 ms 预算

| 项 | 内容 |
| --- | --- |
| 现状 | 实测 p95 ≈ 884 ms，超过 Principle IV 的 < 200 ms 预算（perf-evidence.md §2.1/2.3） |
| 偏离类型 | 不可压缩的加密成本 |
| 触发原因 | spec FR-016 强制 bcrypt；data-model.md 指定 cost=12，单次哈希 ≈ 400 ms（与并发无关） |
| 风险 | 极端情况下登录吞吐 ≈ 8 req/s/core；正常业务场景每个管理员每天登录数次，不构成瓶颈 |
| 替代方案（已否决） | (a) 降低 bcrypt cost — 弱化暴力破解防御，与 FR-016 精神冲突；(b) 切 Argon2id — 切换密码哈希算法需要无密码迁移路径，本期不开 |
| 退出条件 | 永久接受偏离。需要在 perf-evidence.md / SLO 文档中明示登录端点的延迟为加密成本，区别于普通 CRUD 端点的 < 200 ms 预算 |

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
1. 安装 Rust (1.85+), Node.js (18+), MySQL 8.0+
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
