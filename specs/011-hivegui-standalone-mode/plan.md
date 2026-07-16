# Implementation Plan: HiveGUI 独立桌面管理工具

**Branch**: `260517-hivegui-standalone-mode` | **Date**: 2026-07-02 | **Spec**: [spec.md](file:///home/developer/agent/gpui-claw/hive-claw-worktree/specs/011-hivegui-standalone-mode/spec.md)
**Input**: Feature specification from `/specs/011-hivegui-standalone-mode/spec.md`

## Summary

将 HiveGUI 从依赖远程 hiveweb 服务器的客户端转变为独立运行的桌面管理工具。核心功能包括：
- 本地 SQLite 数据库存储所有配置
- 数据源管理（MySQL 连接配置）
- 全局配置管理
- LLM 配置管理（Preset/Provider/Model 三层结构）
- 9 个新增实体的 CRUD 管理（Tag、Category、Capability、Plugin、Function、Workflow、Tool、Skill、Agent）

所有实体管理对齐 hiveweb 数据模型，但仅实现独立 CRUD，不实现实体间关联管理。

## Technical Context

**Language/Version**: Rust 1.85 (stable)
**Primary Dependencies**: 
- gpui (桌面 GUI 框架)
- sqlx (SQLite 数据库访问，编译时 SQL 验证)
- tokio (异步运行时)
- serde/serde_json (JSON 序列化)
- chacha20poly1305 (密码加密)

**Storage**: SQLite (本地嵌入式数据库)
**Testing**: cargo test (单元测试 + 集成测试)
**Target Platform**: Linux/macOS/Windows 桌面端
**Project Type**: desktop-app
**Performance Goals**: 
- CRUD 操作 < 1 秒
- 搜索/分页 < 500ms
- Category 树形渲染 < 200ms
- 连接测试 < 5 秒

**Constraints**: 
- 纯本地应用，不连接远程服务器
- 单实例写入（文件锁）
- 敏感数据加密存储
- identifier 字段 UNIQUE 约束

**Scale/Scope**: 
- 13 个用户故事
- 14 个数据实体（5 已有 + 9 新增）
- 13 个管理界面

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

### Pre-Design Gate Evaluation

**Principle I (Code Quality)**: ✅ PASS
- 使用 Rust + gpui，符合项目规范
- SQLx 编译时 SQL 验证满足代码质量要求

**Principle II (Test-First)**: ✅ PASS
- 用户故事包含明确的验收场景
- 测试策略：单元测试 + UI 测试

**Principle III (UX Consistency)**: ✅ PASS
- 所有 CRUD 界面遵循统一模式（列表 + 表单）
- 错误消息明确（如 identifier 重复提示）

**Principle IV (Performance)**: ✅ PASS
- 明确的性能指标（CRUD < 1s, 搜索 < 500ms）
- SQLite 本地数据库，无网络延迟
- 分页避免大数据集性能问题

**Principle V (Simplicity & YAGNI)**: ✅ PASS
- 仅实现独立 CRUD，不实现关联管理
- 不实现 Workflow 节点/边可视化编辑
- 不实现数据迁移功能

**Principle VI (Observability)**: ⚠️ PARTIAL
- 当前 spec 未明确要求日志/监控
- 建议在关键操作（数据库写入、加密操作）添加结构化日志

**Security Requirements**: ✅ PASS
- 密码使用 chacha20poly1305 加密
- API Key 加密存储
- 输入验证（identifier 唯一性约束）

**Technology Stack Compliance**: ✅ PASS
- Rust + gpui：符合桌面应用规范
- SQLite：符合本地嵌入式数据库需求（非生产环境）
- SQLx：符合数据库访问规范

**Gate Decision**: ✅ PASS - 可进入 Phase 0

## Project Structure

### Documentation (this feature)

```text
specs/011-hivegui-standalone-mode/
├── plan.md              # This file
├── spec.md              # Feature specification
├── research.md          # Phase 0 output (needs update)
├── data-model.md        # Phase 1 output (needs update)
├── quickstart.md        # Phase 1 output (needs update)
├── contracts/           # Phase 1 output (if needed)
└── tasks.md             # Phase 2 output
```

### Source Code (current structure)

```text
crates/hivegui/src/
├── main.rs                    # Entry point
├── lib.rs                     # Public exports
├── config.rs                  # Configuration
├── datasource/
│   ├── mod.rs
│   ├── store.rs               # SQLite store (data_sources, global_configs tables)
│   ├── llm_store.rs           # LLM store (models, llm_presets, llm_providers tables)
│   ├── crypto.rs              # Password encryption (chacha20poly1305)
│   └── mysql_client.rs        # MySQL connection test
└── ui/
    ├── mod.rs
    ├── app.rs                 # AppRoute enum (Home, DataSource, GlobalConfig, LLMConfig)
    ├── sidebar_nav.rs         # Sidebar navigation (4 buttons)
    ├── home.rs                # Home view
    ├── datasource_view.rs     # DataSource list view
    ├── datasource_form.rs     # DataSource form
    ├── global_config.rs       # GlobalConfig list + form
    └── llm_config.rs          # LLM Config (Model/Preset/Provider management)
```

### Source Code (target structure after implementation)

```text
crates/hivegui/src/
├── main.rs
├── lib.rs
├── config.rs
├── datasource/
│   ├── mod.rs
│   ├── store.rs               # SQLite store (existing tables)
│   ├── llm_store.rs           # LLM store (existing tables)
│   ├── crypto.rs
│   ├── mysql_client.rs
│   └── entity_store.rs        # [NEW] New entity CRUD (Tag, Category, Capability, Plugin, Function, Workflow, Tool, Skill, Agent)
└── ui/
    ├── mod.rs
    ├── app.rs                 # AppRoute enum (add 9 new routes)
    ├── sidebar_nav.rs         # Sidebar navigation (add 9 new buttons)
    ├── home.rs
    ├── datasource_view.rs
    ├── datasource_form.rs
    ├── global_config.rs
    ├── llm_config.rs
    ├── tag_view.rs            # [NEW] Tag management
    ├── category_view.rs       # [NEW] Category management (tree view)
    ├── capability_view.rs     # [NEW] Capability management
    ├── plugin_view.rs         # [NEW] Plugin management
    ├── function_view.rs       # [NEW] Function management
    ├── workflow_view.rs       # [NEW] Workflow management
    ├── tool_view.rs           # [NEW] Tool management
    ├── skill_view.rs          # [NEW] Skill management
    └── agent_view.rs          # [NEW] Agent management
```

**Structure Decision**: 扩展现有 hivegui crate，新增 entity_store.rs 统一管理 9 个新实体的 CRUD，每个实体对应一个独立的 UI 视图文件。

## Complexity Tracking

### Deviation: SQLite instead of MySQL

**Constitution Requirement**: Technology Stack specifies MySQL 8.0+ as canonical relational database.

**Specific Need**: HiveGUI is a standalone desktop application that must run without any external database server. Users should not need to install or configure MySQL.

**Alternative Chosen**: SQLite (embedded, file-based database)

**Simpler Approach Considered and Rejected**:
- JSON file storage: Rejected because it lacks query capabilities, transactions, and concurrent access support
- In-memory only: Rejected because data must persist across application restarts
- MySQL with embedded mode: Rejected because MySQL does not support true embedded mode; requires server process

**Maintenance/Review Impact**:
- SQLite is well-supported in Rust ecosystem (sqlx crate)
- No additional infrastructure or deployment complexity
- Trade-off: Limited concurrent write support (acceptable for single-user desktop app)
- Trade-off: No built-in replication/clustering (not needed for local desktop app)

**Justification**: HiveGUI is explicitly designed as a "纯本地桌面应用" (pure local desktop application) per spec.md. Using SQLite aligns with the product requirement of zero external dependencies. This deviation is scoped to HiveGUI only; other services (HiveClaw, hiveweb) continue using MySQL per Constitution.

## Phase 0: Research (已完成)

research.md 已更新，包含新增 9 个实体的设计决策：
- 数据模型对齐 hiveweb
- identifier 唯一性约束
- Category 层级结构（parent_id）
- Workflow 仅管理主表
- 仅实现独立 CRUD，不实现关联管理

## Phase 1: Design & Contracts (已完成)

### 已完成的设计文档

1. **data-model.md**: 已更新，包含 14 个表的完整定义
   - 5 个已有表（data_sources, global_configs, models, llm_presets, llm_providers）
   - 9 个新增表（tags, categories, capabilities, plugins, functions, workflows, tools, skills, agents）
   - 包含验证规则和级联删除规则

2. **quickstart.md**: 已更新，反映新的项目结构和开发流程
   - 新增 entity_store.rs
   - 新增 9 个 UI 视图文件
   - 开发检查清单

3. **Contracts**: 跳过（纯本地桌面应用，无外部 API）

4. **Agent context**: 已更新 project_rules.md 指向新的 plan.md

### Post-Design Constitution Check

**Principle I (Code Quality)**: ✅ PASS
- 使用 SQLx 编译时 SQL 验证
- 统一的 CRUD 模式

**Principle II (Test-First)**: ✅ PASS
- 每个实体都有明确的验收场景
- 测试策略清晰

**Principle III (UX Consistency)**: ✅ PASS
- 所有管理界面遵循统一的列表 + 表单模式
- 错误消息格式一致

**Principle IV (Performance)**: ✅ PASS
- 明确的性能指标
- 分页避免大数据集问题
- SQLite 本地数据库无网络延迟

**Principle V (Simplicity & YAGNI)**: ✅ PASS
- 仅实现独立 CRUD
- 不实现关联管理
- 不实现 Workflow 可视化编辑
- 使用缩进列表代替复杂树形组件

**Principle VI (Observability)**: ✅ PASS
- 使用 tracing 结构化日志
- 环境变量控制日志级别

**Security Requirements**: ✅ PASS
- 密码加密（chacha20poly1305）
- identifier 唯一性约束
- 输入验证

**Technology Stack Compliance**: ✅ PASS
- Rust + gpui：符合桌面应用规范
- SQLite：符合本地嵌入式数据库需求
- SQLx：符合数据库访问规范

**Post-Design Gate Decision**: ✅ PASS - 可进入 Phase 2（任务生成）
