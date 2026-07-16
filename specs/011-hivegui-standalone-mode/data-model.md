# Data Model: HiveGUI 独立桌面管理工具

**Feature**: HiveGUI Standalone Mode  
**Date**: 2026-07-02 (Updated)

## Entity Relationship Overview

```
data_sources (独立)
global_configs (独立)

llm_presets ── models ── llm_providers

tags (独立)
categories ── categories (parent_id 自引用)

capabilities ── categories (category_id)
plugins ── categories (category_id)
functions ── plugins (plugin_id), categories (category_id)
workflows ── categories (category_id)
tools ── functions (function_id), workflows (workflow_id), categories (category_id)
skills ── categories (category_id)
agents ── agents (parent_agent_id 自引用)
```

注：所有新增实体仅独立 CRUD，不实现关联管理。

## Table Definitions (SQLite)

### 1. data_sources (已有)

远程 MySQL 数据源连接配置。

```sql
CREATE TABLE IF NOT EXISTS data_sources (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    host TEXT NOT NULL,
    port INTEGER NOT NULL,
    username TEXT NOT NULL,
    password_encrypted BLOB NOT NULL,
    password_nonce BLOB NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
```

### 2. global_configs (已有)

全局配置键值对。

```sql
CREATE TABLE IF NOT EXISTS global_configs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    key TEXT NOT NULL UNIQUE,
    type TEXT NOT NULL DEFAULT 'text',
    data TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
```

### 3. models (已有)

Preset 内的模型条目；每个 Model 引用一个独立 Provider。

```sql
CREATE TABLE IF NOT EXISTS models (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    preset_id INTEGER NOT NULL REFERENCES llm_presets(id) ON DELETE CASCADE,
    provider_id INTEGER NOT NULL REFERENCES llm_providers(id) ON DELETE RESTRICT,
    priority INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
```

### 4. llm_presets (已有)

LLM 预设。

```sql
CREATE TABLE IF NOT EXISTS llm_presets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL DEFAULT '',
    is_default INTEGER NOT NULL DEFAULT 0,
    max_tokens INTEGER NOT NULL DEFAULT 2048,
    temperature REAL NOT NULL DEFAULT 0.7,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
```

### 5. llm_providers (已有)

独立的 LLM 后端连接配置，不属于任何 Preset。

```sql
CREATE TABLE IF NOT EXISTS llm_providers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL DEFAULT 'openai_compat',
    base_url TEXT NOT NULL DEFAULT '',
    api_key_encrypted BLOB,
    api_key_nonce BLOB,
    api_key_env TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
```

### 6. tags (新增)

标签。

```sql
CREATE TABLE IF NOT EXISTS tags (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    color TEXT,
    created_at TEXT NOT NULL
);
```

### 7. categories (新增)

分类，支持树形层级。

```sql
CREATE TABLE IF NOT EXISTS categories (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    parent_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    name TEXT NOT NULL,
    slug TEXT NOT NULL UNIQUE,
    description TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
```

### 8. capabilities (新增)

能力。name 为主键。

```sql
CREATE TABLE IF NOT EXISTS capabilities (
    name TEXT PRIMARY KEY,
    description TEXT NOT NULL,
    is_dangerous INTEGER NOT NULL DEFAULT 0,
    category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL
);
```

### 9. plugins (新增)

插件。identifier 唯一，支持软删除。

```sql
CREATE TABLE IF NOT EXISTS plugins (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    identifier TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    description TEXT,
    manifest TEXT,                     -- JSON
    runtime TEXT NOT NULL,
    version TEXT NOT NULL,
    author TEXT,
    repository_url TEXT,
    s3_key TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted_at TEXT
);
```

### 10. functions (新增)

函数。identifier 唯一。

```sql
CREATE TABLE IF NOT EXISTS functions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    identifier TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    description TEXT,
    kind INTEGER NOT NULL DEFAULT 1,   -- 1=builtin, 2=custom
    input_schema TEXT NOT NULL DEFAULT '{}',   -- JSON
    output_schema TEXT NOT NULL DEFAULT '{}',  -- JSON
    plugin_id INTEGER REFERENCES plugins(id) ON DELETE SET NULL,
    plugin_export TEXT,
    category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    required_capabilities TEXT,        -- JSON
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
```

### 11. workflows (新增)

工作流主表。identifier 唯一。不包含 nodes/edges 子表。

```sql
CREATE TABLE IF NOT EXISTS workflows (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    identifier TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    description TEXT,
    timeout_ms INTEGER NOT NULL DEFAULT 30000,
    category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    input_schema TEXT,                 -- JSON
    start_description TEXT,
    output_schema TEXT,                -- JSON
    required_capabilities TEXT,        -- JSON
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
```

### 12. tools (新增)

工具。identifier 唯一。function_id 和 workflow_id 互斥。

```sql
CREATE TABLE IF NOT EXISTS tools (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    identifier TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    kind INTEGER NOT NULL DEFAULT 1,   -- 1=function-wrap, 2=workflow-wrap
    source TEXT NOT NULL DEFAULT 'workspace',  -- workspace|builtin
    is_always INTEGER NOT NULL DEFAULT 0,
    function_id INTEGER REFERENCES functions(id) ON DELETE SET NULL,
    workflow_id INTEGER REFERENCES workflows(id) ON DELETE SET NULL,
    input_schema TEXT NOT NULL DEFAULT '{}',   -- JSON
    output_schema TEXT NOT NULL DEFAULT '{}',  -- JSON
    category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    required_capabilities TEXT,        -- JSON
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (
        (kind = 1 AND function_id IS NOT NULL AND workflow_id IS NULL) OR
        (kind = 2 AND workflow_id IS NOT NULL AND function_id IS NULL)
    )
);
```

### 13. skills (新增)

技能。identifier 唯一。

```sql
CREATE TABLE IF NOT EXISTS skills (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    identifier TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    frontmatter TEXT,                  -- JSON
    content TEXT NOT NULL,             -- markdown
    source TEXT NOT NULL DEFAULT 'workspace',
    is_always INTEGER NOT NULL DEFAULT 0,
    category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    required_capabilities TEXT,        -- JSON
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
```

### 14. agents (新增)

Agent。identifier 唯一。parent_agent_id 自引用。

```sql
CREATE TABLE IF NOT EXISTS agents (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    identifier TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    description TEXT,
    system_prompt TEXT NOT NULL DEFAULT '',
    parent_agent_id INTEGER REFERENCES agents(id) ON DELETE SET NULL,
    depth INTEGER NOT NULL DEFAULT 0,
    model_preset TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
```

## Validation Rules

- `tags.name`: UNIQUE
- `categories.slug`: UNIQUE
- `capabilities.name`: PRIMARY KEY (自然唯一)
- `plugins.identifier`: UNIQUE
- `functions.identifier`: UNIQUE
- `workflows.identifier`: UNIQUE
- `tools.identifier`: UNIQUE
- `skills.identifier`: UNIQUE
- `agents.identifier`: UNIQUE
- `tools.kind=1` 时 `function_id` 非空，`kind=2` 时 `workflow_id` 非空（CHECK 约束）
- `plugins.deleted_at` 非空时，列表默认过滤（软删除）

## Cascade Rules

- 删除 Category 时，引用该 Category 的实体的 category_id 置为 NULL（ON DELETE SET NULL）
- 删除 Category 时，子分类的 parent_id 置为 NULL（ON DELETE SET NULL）
- 删除 Agent 时，子 Agent 的 parent_agent_id 置为 NULL（ON DELETE SET NULL）
- 删除 Plugin 时，引用该 Plugin 的 Function 的 plugin_id 置为 NULL
- 删除 Function 时，引用该 Function 的 Tool 的 function_id 置为 NULL
- 删除 Workflow 时，引用该 Workflow 的 Tool 的 workflow_id 置为 NULL
