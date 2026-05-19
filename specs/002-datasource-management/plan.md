# Implementation Plan: 数据源管理

**Branch**: `002-mysql-data-source` | **Date**: 2026-05-15 | **Spec**: [spec.md](./spec.md)

**Note**: This template is filled in by the `/speckit-plan` command. See `.specify/templates/plan-template.md` for the execution workflow.

## Summary

在 HiveGUI 中增加数据源管理功能，支持添加/编辑/删除 MySQL 数据源，通过左侧树形导航浏览数据库和表，右侧面板展示表的列信息、DDL 和数据预览（支持 WHERE 筛选、排序、分页）。数据源连接信息使用 SQLite 持久化存储，MySQL 客户端使用 `mysql_async` crate 建立连接并执行查询。

## Technical Context

**Language/Version**: Rust (stable, MSRV 1.80)  
**Primary Dependencies**: `mysql_async` (MySQL 客户端)、`sqlx` 或 `rusqlite` (SQLite 持久化)、`gpui` (GUI)  
**Storage**: SQLite (嵌入式关系存储，持久化数据源配置，直接在 hivegui 进程中)  
**Testing**: `cargo test` 单元测试 + 集成测试  
**Target Platform**: Linux desktop (HiveGUI)  
**Project Type**: desktop-app (HiveGUI)  
**Performance Goals**: 树列表加载 < 3s、列信息 < 2s、数据预览 < 5s（100 条）  
**Constraints**: gpui 框架下实现 UI；MySQL 连接复用/按需创建；密码加密存储  
**Scale/Scope**: 单用户桌面应用，管理数十个数据源，每个数据库数十到数百张表

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Gate | Status | Justification |
|------|--------|---------------|
| **Tech Stack Compliance** | ✅ PASS | 使用 Rust、gpui、SQLite，均为宪法指定技术；`mysql_async` 是新依赖但宪法允许功能所需的外部客户端库 |
| **Project Limit (≤3)** | ✅ PASS | 仅在 `hivegui` crate 内新增模块，不涉及 `hiveclaw` 变更，远未超过 3 个部署单元 |
| **TDD Mandatory (Principle II)** | ✅ PASS | 计划遵循先写测试 → 验证失败 → 实现 → 重构的流程 |
| **Performance Budget (Principle IV)** | ✅ PASS | 性能目标与宪法 < 200ms p95 对齐（仅适用于 API 端点；GUI 查询延迟为 spec 定义） |
| **Observability (Principle VI)** | ✅ PASS | 数据源连接/查询操作将记录结构化日志 |
| **Security Requirements** | ⚠ REVIEW | 密码需加密存储（见 research.md），需确保凭证不泄露 |
| **YAGNI (Principle V)** | ✅ PASS | 仅实现 MySQL，不预置其他数据库扩展点；不添加无当前消费者的抽象 |

## Project Structure

### Documentation (this feature)

```text
specs/002-datasource-management/
├── plan.md              # This file (/speckit-plan command output)
├── research.md          # Phase 0 output (/speckit-plan command)
├── data-model.md        # Phase 1 output (/speckit-plan command)
├── quickstart.md        # Phase 1 output (/speckit-plan command)
├── contracts/           # Phase 1 output (/speckit-plan command)
└── tasks.md             # Phase 2 output (/speckit-tasks command - NOT created by /speckit-plan)
```

### Source Code (repository root)

```text
crates/hivegui/
├── src/
│   ├── ...existing...
│   ├── datasource/              # 数据源管理核心模块（新增）
│   │   ├── mod.rs               # 模块入口
│   │   ├── models.rs            # 数据源数据模型（DataSource, DatabaseInfo, TableInfo, ColumnInfo, TableData）
│   │   ├── store.rs             # SQLite 持久化层（CRUD 操作）
│   │   ├── mysql_client.rs      # MySQL 连接与查询封装
│   │   └── crypto.rs            # 密码加解密
│   └── ui/
│       ├── ...existing...
│       ├── datasource_panel.rs  # 数据源管理面板（左列）
│       ├── tree_nav.rs          # 树形导航组件（中列）
│       └── table_viewer.rs      # 表查看器（右列，含三 Tab）
└── tests/
    └── datasource/
        ├── store.rs             # SQLite 存储层测试
        └── mysql_client.rs      # MySQL 客户端测试
```

**Structure Decision**: 所有功能在 `hivegui` 一个 crate 内实现，不经过 `hiveclaw` 后端 API。`hivegui` 直接连接 MySQL（通过 `mysql_async`）、直接管理 SQLite 持久化（通过 `sqlx`），UI 层调用本地服务层。

### UI Layout: 左中右三列

```text
┌─────────────┬──────────────────┬─────────────────────────────┐
│  左列        │     中列          │          右列                │
│ 数据源面板   │   树形导航        │       表查看器               │
│             │                  │                             │
│ [数据源列表]│  ┌─ 数据源名称   │  ┌───────────────────────┐  │
│ + 添加      │  │ ── 📁 db1    │  │ [列] [DDL] [数据]     │  │
│ + 编辑      │  │ │ ── 📋 t1  │  │                       │  │
│ + 删除      │  │ │ ── 📋 t2  │  │  Tab 内容区域           │  │
│             │  │ ── 📁 db2    │  │                       │  │
│             │  │   ── 📋 t3   │  │                       │  │
│             │  └────────────── │  └───────────────────────┘  │
│             │                  │                             │
└─────────────┴──────────────────┴─────────────────────────────┘
```

- **左列（数据源面板）**：数据源列表 + 添加/编辑/删除操作按钮
- **中列（树形导航）**：展开数据源 → 数据库 → 表的三级树形结构
- **右列（表查看器）**：选中表后展示三个 Tab（列、DDL、数据）

## Complexity Tracking

> **Fill ONLY if Constitution Check has violations that must be justified**

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| N/A | 无宪法违反项 | — |
