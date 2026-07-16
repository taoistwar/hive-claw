# Research: HiveGUI 独立桌面管理工具

**Date**: 2026-07-02 (Updated)

## 1. 架构

**Decision**: HiveGUI 作为独立桌面应用，包含以下功能模块：
- 数据源管理（MySQL 连接配置）
- 全局配置管理
- LLM 配置管理（Preset/Provider/Model）
- 9 个新增实体管理（Tag、Category、Capability、Plugin、Function、Workflow、Tool、Skill、Agent）

**Rationale**: 仿照 hiveweb 数据模型，实现完整的本地配置管理能力。

## 2. gpui-component

**Decision**: 表单输入使用 `InputState::new(window, cx)` + `Option<Entity<InputState>>` 延迟初始化。

**Rationale**: 统一表单组件模式，提高代码复用性。

## 3. 全局配置

**Decision**: 完整复制 hiveweb 的 `global_configs` 表结构（id, name, key UNIQUE, type, data, created_at, updated_at），支持分页搜索。

- 类型字段支持: text, number, json, boolean
- 分页: 每页 20 条
- 搜索: 按 name 或 key LIKE 匹配

## 4. 导航

**Decision**: 13 路由 — Home, DataSource, GlobalConfig, LLMConfig, Tag, Category, Capability, Plugin, Function, Workflow, Tool, Skill, Agent。

**Rationale**: 每个实体独立管理界面，侧边栏提供快速导航。

## 5. LLM 配置

**Decision**: Model 独立实体（`models` 表），Preset 1→N Provider，Provider N→1 Model。API Key chacha20poly1305 加密。

## 6. 新增实体设计决策

### 6.1 数据模型对齐

**Decision**: 9 个新实体的字段完整对齐 hiveweb，包括所有 JSON 字段和可选字段。

**Rationale**: 确保数据模型一致性，便于未来可能的数据导入/导出。

### 6.2 identifier 唯一性

**Decision**: Plugin、Function、Workflow、Tool、Skill、Agent 的 identifier 字段必须唯一（UNIQUE 约束）。

**Rationale**: identifier 作为程序化标识符，唯一性确保引用一致性。

### 6.3 Category 层级结构

**Decision**: Category 支持 parent_id 字段，实现树形层级结构。UI 使用缩进列表展示（简化版树形视图）。

**Rationale**: 对齐 hiveweb 设计，同时简化 UI 实现（避免复杂的树形组件）。

### 6.4 Workflow 管理范围

**Decision**: 仅管理 Workflow 主表元数据，不包含 WorkflowNode/WorkflowEdge 子实体的 CRUD。

**Rationale**: 节点/边的可视化编辑需要画布 UI，复杂度高，与 HiveGUI 轻量级定位不符。

### 6.5 关系管理

**Decision**: 所有实体仅实现独立 CRUD，不实现实体间关联管理（如 Agent 不分配 Tool/Skill，不打标签）。

**Rationale**: 简化实现，聚焦核心配置管理功能。

## 7. SQLite 表设计

**Decision**: 新增 9 个表，字段完整对齐 hiveweb：

1. **tags**: id, name (UNIQUE), color, created_at
2. **categories**: id, parent_id (FK→自身), name, slug (UNIQUE), description, created_at, updated_at
3. **capabilities**: name (PK), description, is_dangerous, category_id (FK), created_at
4. **plugins**: id, identifier (UNIQUE), name, description, manifest (JSON), runtime, version, author, repository_url, s3_key, sha256, size_bytes, category_id (FK), created_at, updated_at, deleted_at
5. **functions**: id, identifier (UNIQUE), name, description, kind, input_schema (JSON), output_schema (JSON), plugin_id (FK), plugin_export, category_id (FK), required_capabilities (JSON), created_at, updated_at
6. **workflows**: id, identifier (UNIQUE), name, description, timeout_ms, category_id (FK), input_schema (JSON), start_description, output_schema (JSON), required_capabilities (JSON), created_at, updated_at
7. **tools**: id, identifier (UNIQUE), name, description, kind, source, is_always, function_id (FK), workflow_id (FK), input_schema (JSON), output_schema (JSON), category_id (FK), required_capabilities (JSON), created_at, updated_at
8. **skills**: id, identifier (UNIQUE), name, description, frontmatter (JSON), content, source, is_always, category_id (FK), required_capabilities (JSON), created_at, updated_at
9. **agents**: id, identifier (UNIQUE), name, description, system_prompt, parent_agent_id (FK→自身), depth, model_preset, created_at, updated_at

**Rationale**: 完整对齐 hiveweb 数据模型，确保一致性。

## 8. UI 组件复用

**Decision**: 复用现有的列表/表单组件模式：
- 列表视图：表格展示 + 搜索栏 + 分页 + 添加/编辑/删除按钮
- 表单视图：模态框或侧边面板，包含输入字段 + 保存/取消按钮

**Rationale**: 统一用户体验，减少开发工作量。

## 9. Category 树形视图

**Decision**: 使用缩进列表展示层级结构（每个条目前添加缩进符号表示层级）。

**Rationale**: 简化实现，避免复杂的树形组件。未来可升级为完整的树形视图。
