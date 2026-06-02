# Implementation Plan: 游戏别名管理

**Branch**: `006-game-alias-management` | **Date**: 2026-06-01 | **Spec**: `specs/006-game-alias-management/spec.md`
**Input**: Feature specification from `specs/006-game-alias-management/spec.md`

## Summary

在管理中心中添加游戏别名管理功能，支持 System/Super 管理员增删改查、Normal 管理员只读。采用 Rust + axum 后端架构和 React + TypeScript 前端架构，复用管理中心的认证中间件、RBAC 权限体系、审计日志和统一响应格式。数据存储采用一对多关系：`games` 表存储游戏基本信息，`game_alias_entries` 表存储每个别名单独一行，`alias` 字段加唯一索引保证全局唯一。

## Technical Context

**Language/Version**: Rust 1.85+ (backend), TypeScript 5.x (frontend)
**Primary Dependencies**: axum (API), React (web), tokio (async runtime), SQLx (MySQL client), serde_json (JSON handling), Ant Design (UI components)
**Storage**: MySQL 8.0+ (InnoDB 引擎), `games` + `game_alias_entries` 一对多设计，`alias` 字段 UNIQUE 索引
**Testing**: cargo test (Rust), Vitest + Testing Library (React)
**Target Platform**: Linux server (backend), Web browser (frontend)
**Project Type**: Web application feature module (管理中心的子模块)
**Performance Goals**: API p95 < 200ms, 列表加载 < 2 秒 (首屏 10 条)
**Constraints**: 名称唯一、别名全局唯一、名称最多 50 字符、每个别名最多 50 字符、每个游戏最多 20 个别名
**Scale/Scope**: 支持 1000+ 游戏别名配置，复用现有认证/权限/审计体系

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- **Principle I - Code Quality & Maintainability**: cargo fmt + cargo clippy (-D warnings) + ESLint + Prettier。新代码遵循现有命名和结构规范。
- **Principle II - Test-First Development (NON-NEGOTIABLE)**: 追溯补测试 — 后端对 service 层、API 层编写测试；前端对 GameTable、GameAliasForm 组件编写测试。契约测试覆盖全部 5 个端点。
- **Principle III - User Experience Consistency**: 复用管理中心的统一响应格式、错误码体系、Ant Design 组件。a11y 遵循 FR-015（键盘导航、WCAG 2.1 AA、axe-core）。
- **Principle IV - Performance & Efficiency**: API p95 < 200ms 预算。B-tree 索引 on name + alias 使 `LIKE` 高效；分页限制最大 100；单查询 `LEFT JOIN` + `JSON_ARRAYAGG` 避免 N+1。
- **Principle V - Simplicity & YAGNI**: 复用现有认证/权限/审计体系，不引入新框架或依赖。一对多表设计（而非过度正规化的三表设计），别名直接存 `VARCHAR` 而非独立表 + 映射表。
- **Principle VI - Observability & Structured Logging**: 复用现有 `tracing` + JSON 日志 + `request_id` 中间件。审计日志复用 `admin_audit_logs` 表，新增游戏别名操作类型。
- **Security Requirements**: RBAC 权限检查（后端权威 403 + 错误码 2001）、输入验证（长度、唯一性、数组大小）、前端"无权限 = 不渲染"。
- **Technology Stack**: Rust + axum (后端), TypeScript + React + Ant Design (前端), MySQL (一对多关系表) — 符合宪法 v1.3.0 规定，无偏离。

**Gate Result**: PASS — 全部 6 项原则合规，无偏离。

## Project Structure

### Documentation (this feature)

```text
specs/006-game-alias-management/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── api.md
└── tasks.md             # Phase 2 output (/speckit-tasks command)
```

### Source Code (repository root)

```text
crates/hiveweb/
├── migrations/
│   ├── V045__create_games_table.sql
│   └── V046__create_game_alias_entries_table.sql
├── src/
│   ├── models/
│   │   └── game.rs                      # Game 模型 + DTOs
│   ├── services/
│   │   └── game_service.rs              # 游戏别名业务逻辑
│   ├── api/
│   │   └── game.rs                      # 游戏别名 API handlers
│   └── main.rs / lib.rs                 # 路由注册

web-admin/
├── src/
│   ├── pages/
│   │   └── GameAliasPage.tsx            # 游戏别名管理页
│   ├── components/
│   │   ├── GameTable.tsx                # 游戏列表组件
│   │   ├── GameAliasForm.tsx            # 游戏别名表单组件 (Tags 输入)
│   │   └── PermissionGuard.tsx          # 权限守卫（复用）
│   └── services/
│       └── gameAlias.ts                 # API 调用封装
```

**Structure Decision**:
- 作为 003-admin-center 管理中心的子模块，集成到现有 hiveweb crate 和 web-admin 前端项目中
- 复用现有认证中间件、权限检查、审计日志、响应格式
- 不创建新 crate 或新前端包（符合 Principle V：最多三个部署单元）

## Complexity Tracking

> 无需偏离。本功能完全复用现有技术栈和基础设施。

## Phase 0: Research

### Research Findings

详见 `specs/006-game-alias-management/research.md`

**关键决策**:
- 数据模型：一对多设计 — `games` + `game_alias_entries`，`alias` 字段 UNIQUE 索引保证全局唯一
- 搜索实现：`LIKE` on name + `EXISTS` subquery on alias（B-tree 索引，1000 级数据量高效）
- 插入/更新：事务中操作 — INSERT game → INSERT N alias rows；UNIQUE conflict 时自动回滚
- 权限复用：复用现有 RBAC，Normal 只读 / System+Super 增删改查
- API 设计：遵循现有 `/api/game-aliases` 路径 + 统一响应格式
- 错误码范围：4001-4999（游戏别名管理专用），新增 4008 ALIAS_ALREADY_IN_USE
- 审计日志：复用现有 `audit_logs` 表，新增 GAME_ALIAS_CREATE/UPDATE/DELETE 操作
- 前端编辑器：Ant Design `Select mode="tags"` Tags 输入组件
- 去重策略：前后端双重去重 + 后端 UNIQUE 索引作为最终权威

## Phase 1: Design

### Data Model (data-model.md)

详见 `specs/006-game-alias-management/data-model.md`

**核心实体**:

1. **Game (游戏)**
   - id: BIGINT (主键，自增)
   - name: VARCHAR(50) (唯一，索引)
   - created_at: DATETIME
   - updated_at: DATETIME (自动更新)

2. **GameAliasEntry (游戏别名条目)**
   - id: BIGINT (主键，自增)
   - game_id: BIGINT (外键 → games.id ON DELETE CASCADE)
   - alias: VARCHAR(50) (唯一，全局去重)

### API Contracts (contracts/)

详见 `specs/006-game-alias-management/contracts/api.md`

**API Endpoints**:
- `GET /api/game-aliases` - 获取游戏别名列表（分页 + 搜索）
- `POST /api/game-aliases` - 添加游戏别名（System/Super only）
- `GET /api/game-aliases/:id` - 获取游戏别名详情
- `PUT /api/game-aliases/:id` - 编辑游戏别名（System/Super only）
- `DELETE /api/game-aliases/:id` - 删除游戏别名（System/Super only）

### Quick Start Guide (quickstart.md)

详见 `specs/006-game-alias-management/quickstart.md`
