# Data Model: Agent Runtime

> **范围更新（2026-07-23）：**
> - `RecommendedGame`、`recommended_games*` 及 `/api/recommended-games*` 相关管理和公开接口已废弃，仅兼容保留；不得新增调用或扩展。
> - 原 `chat_sessions` / `chat_messages` admin 表及管理端测试聊天已由 `81a84fe` 移除并 superseded；不得恢复这些表、`/api/admin-chat*` 或 `/api/chat/sessions*`。
> - 现行普通用户聊天使用 `chat_sessions_user` / `chat_messages_user` 并按 `user_id` 隔离，属于外部 Assistant API 的数据面。

**Created**: 2026-05-26
**Last Updated**: 2026-07-24
**Status**: 004 runtime model current；聊天部分已同步后续用户表/legacy 删除边界

---

## 1. 实体一览

| 实体 | 表名 | 关键关系 |
| --- | --- | --- |
| Admin | `admins` | — |
| LoginRecord | `login_records` | admin_id |
| AdminAuditLog | `admin_audit_logs` | actor_admin_id |
| Capability | `capabilities` | category_id |
| Category | `categories` | self-FK parent_id |
| Tag | `tags` | — |
| Plugin | `plugins` | category_id, taggable |
| Function | `functions` | plugin_id (custom 才有), category_id, taggable |
| Workflow | `workflows` | nodes/edges 在子表, category_id |
| WorkflowNode | `workflow_nodes` | workflow_id, function_id (nullable), node_type |
| WorkflowEdge | `workflow_edges` | workflow_id, src_node_id, dst_node_id, mapping |
| Tool | `tools` | function_id OR workflow_id（互斥）, category_id, taggable |
| Skill | `skills` | 不引用 Function/Workflow, category_id, taggable |
| Agent | `agents` | self-FK parent_agent_id |
| AgentTool（多对多） | `agent_tools` | agent_id, tool_id |
| AgentSkill（多对多） | `agent_skills` | agent_id, skill_id |
| AgentPermission | `agent_permissions` | agent_id, capability_name |
| Taggable（通用 polymorphic） | `taggings` | tag_id + entity_type + entity_id |
| ChatSessionUser（外部 Assistant API） | `chat_sessions_user` | user_id（普通用户） |
| ChatMessageUser（外部 Assistant API） | `chat_messages_user` | session_id, user_id |
| RuntimeAuditLog | `runtime_audit_logs` | session_id?, agent_id, plugin_id, capability |
| RecommendedGame | `recommended_games` | — |

Capability 是**代码内静态注册表**，`capabilities` 表只存描述/危险标记/分类等元数据，启动时由 runtime registry 同步 upsert。

---

## 2. 物理迁移链（SQL / `MIGRATIONS` 为唯一真值）

当前可执行迁移由 `crates/hiveweb/migrations/*.sql` 与
`crates/hiveweb/src/bin/migrate.rs::MIGRATIONS` 共同定义。注册链为
**V001–V033**，其中 **V013 是保留空号**：没有 SQL 文件，也不得为了补齐
编号而新建或重命名迁移。

| 版本 | 物理 SQL | 作用 |
| --- | --- | --- |
| V001 | `V001__create_admins_table.sql` | `admins` |
| V002 | `V002__create_login_records_table.sql` | `login_records` |
| V003 | `V003__seed_super_admin.sql` | 保留的 no-op 占位；Super 由 CLI 创建 |
| V004 | `V004__audit_logs.sql` | `admin_audit_logs` |
| V005 | `V005__categories.sql` | `categories` |
| V006 | `V006__capabilities.sql` | `capabilities`（已含 `category_id`） |
| V007 | `V007__tags.sql` | `tags`、`taggings` |
| V008 | `V008__plugins.sql` | `plugins` |
| V009 | `V009__functions.sql` | `functions`（已含 `required_capabilities`） |
| V010 | `V010__workflows.sql` | `workflows`、`workflow_nodes`、`workflow_edges`；已含分类、输入/输出 schema、四类节点和 capabilities |
| V011 | `V011__tools_skills.sql` | `tools`、`skills`；已含 `source`、`is_always`、分类和 capabilities |
| V012 | `V012__agents.sql` | `agents` 与 tool/skill/permission 关联表 |
| V013 | — | **保留空号；无文件、无注册项** |
| V014 | `V014__seed.sql` | main Agent 与 Capability 元数据 seed |
| V015 | `V015__recommended_games.sql` | legacy `recommended_games` |
| V016 | `V016__seed_capability_categories.sql` | Capability 分类 seed 与回填 |
| V017 | `V017__users_table.sql` | 普通用户 `users` |
| V018 | `V018__split_chat_tables.sql` | `chat_sessions_user`、`chat_messages_user` |
| V019 | `V019__create_global_configs.sql` | `global_configs` |
| V020 | `V020__create_games_table.sql` | `games` |
| V021 | `V021__create_game_alias_entries_table.sql` | `game_alias_entries` |
| V022 | `V022__create_agent_hooks_table.sql` | `agent_hooks` |
| V023 | `V023__create_hook_executions_table.sql` | `hook_executions` |
| V024 | `V024__add_extensions_to_chat_messages_user.sql` | 用户消息 `extensions` |
| V025 | `V025__create_sensitive_words.sql` | `sensitive_words` |
| V026 | `V026__seed_sensitive_words.sql` | 敏感词 seed |
| V027 | `V027__add_uid_nickname_to_users.sql` | 用户 `uid` / `nickname` |
| V028 | `V028__drop_phone_password_status_from_users.sql` | 删除旧用户认证字段 |
| V029 | `V029__game_category_json.sql` | 推荐游戏分类改为 JSON |
| V030 | `V030__recommended_games_channel.sql` | `recommended_games_strategy` |
| V031 | `V031__game_image_text.sql` | 推荐游戏图片字段改为 TEXT |
| V032 | `V032__runtime_audit_logs.sql` | 非 Hook `runtime_audit_logs` |
| V033 | `V033__normalize_workflow_timeout_default.sql` | Workflow DB 默认 timeout 规范化为 33000 ms |

### 2.1 Agent Runtime 核心字段的实际归属

旧设计曾把后补字段写成“V019–V038 扩展”。这些编号是历史规划标签，
**不是当前仓库中的物理迁移**。当前字段已经合并到基础迁移：

| 当前实体/字段 | 实际物理迁移 |
| --- | --- |
| Category | V005 |
| Capability（含 `category_id`） | V006；分类 seed 为 V016 |
| Tag / Tagging | V007 |
| Plugin | V008 |
| Function（含 `category_id`、`required_capabilities`） | V009 |
| Workflow / Node / Edge（含 schema、描述、四类节点、`node_config`、分类、capabilities） | V010 |
| Tool / Skill（含 `source`、`is_always`、分类、capabilities） | V011 |
| Agent 与关联表 | V012；main seed 为 V014 |
| Runtime audit | V032 |
| Workflow timeout 默认值 33000 ms | V033（覆盖 V010 的历史初始默认） |

Workflow service 在 POST / PUT 时还强制
`timeout_ms ∈ 1000..=330000`；省略 POST timeout 时使用 33000 ms。数据库
V033 与服务默认一致，但范围校验属于 service 层。

### 2.2 历史设计快照（Superseded，不可执行）

<details>
<summary>展开查看旧的 V001–V038 规划草稿</summary>

以下 DDL 仅保留用于解释早期评审记录。它不是迁移清单，也不得据此创建、
补号或重命名 SQL；遇到冲突一律以上表和实际 SQL / `MIGRATIONS` 为准。

### V001 admins

```sql
CREATE TABLE admins (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    phone VARCHAR(11) NOT NULL UNIQUE COMMENT '手机号，登录账号',
    password_hash VARCHAR(255) NOT NULL,
    nickname VARCHAR(20) NOT NULL DEFAULT '' COMMENT '昵称',
    role TINYINT NOT NULL DEFAULT 0 COMMENT '1=Super, 2=Editor, 3=Viewer',
    status TINYINT NOT NULL DEFAULT 1 COMMENT '1=active, 0=disabled',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
```

### V002 login_records

```sql
CREATE TABLE login_records (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    admin_id BIGINT NOT NULL COMMENT '成功时关联 admin；失败时为 0 或 NULL',
    phone VARCHAR(11) NOT NULL COMMENT '登录时输入的手机号',
    ip VARCHAR(45) NOT NULL COMMENT '客户端 IP',
    user_agent VARCHAR(255) NULL COMMENT '浏览器 UA',
    status VARCHAR(16) NOT NULL COMMENT 'success|fail_captcha|fail_credentials',
    login_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    INDEX idx_login_records_phone (phone, login_at DESC),
    CONSTRAINT fk_login_records_admin FOREIGN KEY (admin_id) REFERENCES admins(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
```

### V003 seed super admin

```sql
INSERT IGNORE INTO admins (id, phone, password_hash, nickname, role, status)
VALUES (1, '13800138000', '$2b$12$...', '系统超级管理员', 1, 1);
```

### V004 login_records: SET NULL + snapshots

```sql
ALTER TABLE login_records
    DROP FOREIGN KEY fk_login_records_admin;
ALTER TABLE login_records
    ADD CONSTRAINT fk_login_records_admin FOREIGN KEY (admin_id) REFERENCES admins(id) ON DELETE SET NULL;
```

### V005 login_records index

```sql
ALTER TABLE login_records
    ADD INDEX idx_login_records_login_at_desc (login_at DESC);
```

### V006 audit_logs → V028 更名为 admin_audit_logs

```sql
CREATE TABLE admin_audit_logs (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    actor_admin_id BIGINT NULL COMMENT '操作者；admin 删除后 SET NULL',
    action VARCHAR(32) NOT NULL COMMENT 'create|update|delete|login|...',
    entity_type VARCHAR(32) NOT NULL COMMENT 'admin|plugin|function|workflow|tool|skill|agent|...',
    entity_id BIGINT NULL,
    old_values JSON NULL,
    new_values JSON NULL,
    ip VARCHAR(45) NULL,
    user_agent VARCHAR(255) NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    INDEX idx_aal_actor (actor_admin_id),
    INDEX idx_aal_entity (entity_type, entity_id),
    INDEX idx_aal_created_at (created_at DESC),
    CONSTRAINT fk_aal_admin FOREIGN KEY (actor_admin_id) REFERENCES admins(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='管理审计日志（管理员对配置数据的写操作）';
```

### V007 admins index

```sql
ALTER TABLE admins ADD INDEX idx_admins_created_at (created_at DESC);
```

### V008 capabilities (元数据)

```sql
CREATE TABLE capabilities (
    name VARCHAR(64) PRIMARY KEY COMMENT 'e.g. network.http',
    description VARCHAR(255) NOT NULL,
    is_dangerous TINYINT(1) NOT NULL DEFAULT 0 COMMENT '需 Super 才能授予',
    category_id BIGINT NULL COMMENT '所属分类',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT fk_capabilities_category FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
-- 启动时由 runtime registry 同步 upsert。
```

### V009 categories

```sql
CREATE TABLE categories (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    parent_id BIGINT NULL,
    name VARCHAR(64) NOT NULL,
    slug VARCHAR(64) NOT NULL,
    description VARCHAR(255) NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    UNIQUE KEY uk_categories_slug (parent_id, slug),
    CONSTRAINT fk_categories_parent FOREIGN KEY (parent_id) REFERENCES categories(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
```

### V010 tags

```sql
CREATE TABLE tags (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    name VARCHAR(32) NOT NULL UNIQUE,
    color VARCHAR(7) NULL COMMENT 'hex 颜色，可选',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE taggings (
    tag_id BIGINT NOT NULL,
    entity_type VARCHAR(16) NOT NULL COMMENT 'plugin|function|tool|skill|agent',
    entity_id BIGINT NOT NULL,
    PRIMARY KEY (tag_id, entity_type, entity_id),
    INDEX idx_taggings_entity (entity_type, entity_id),
    CONSTRAINT fk_taggings_tag FOREIGN KEY (tag_id) REFERENCES tags(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
```

### V011 plugins

```sql
CREATE TABLE plugins (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    identifier VARCHAR(64) NOT NULL,
    name VARCHAR(128) NOT NULL,
    description VARCHAR(512) NULL,
    manifest JSON NULL COMMENT 'extism manifest 片段',
    runtime VARCHAR(32) NOT NULL DEFAULT 'extism' COMMENT '当前固定 extism',
    version VARCHAR(32) NOT NULL,
    author VARCHAR(64) NULL,
    repository_url VARCHAR(255) NULL,
    s3_key VARCHAR(255) NOT NULL COMMENT '对象存储中 WASM 文件 key',
    sha256 CHAR(64) NOT NULL COMMENT 'WASM 文件 SHA-256',
    size_bytes BIGINT NOT NULL,
    category_id BIGINT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    deleted_at DATETIME NULL COMMENT '软删除',
    UNIQUE KEY uk_plugins_identifier_version (identifier, version),
    INDEX idx_plugins_category (category_id),
    INDEX idx_plugins_deleted_at (deleted_at),
    FULLTEXT INDEX ftx_plugins (name, description, identifier),
    CONSTRAINT fk_plugins_category FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
```

### V012 functions

```sql
CREATE TABLE functions (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    identifier VARCHAR(64) NOT NULL UNIQUE COMMENT '全局唯一标识符',
    name VARCHAR(128) NOT NULL,
    description VARCHAR(512) NULL,
    kind TINYINT NOT NULL COMMENT '1=builtin, 2=custom',
    input_schema JSON NOT NULL COMMENT 'JSON Schema',
    output_schema JSON NOT NULL,
    plugin_id BIGINT NULL COMMENT 'custom 必填',
    plugin_export VARCHAR(64) NULL COMMENT 'extism export 函数名，custom 必填',
    category_id BIGINT NULL,
    required_capabilities JSON NULL COMMENT '声明该 function 执行所需的 capabilities',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    INDEX idx_functions_plugin (plugin_id),
    INDEX idx_functions_category (category_id),
    FULLTEXT INDEX ftx_functions (name, description, identifier),
    CONSTRAINT fk_functions_plugin FOREIGN KEY (plugin_id) REFERENCES plugins(id) ON DELETE RESTRICT,
    CONSTRAINT fk_functions_category FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE SET NULL,
    CONSTRAINT chk_functions_custom CHECK (
        (kind = 1) OR (kind = 2 AND plugin_id IS NOT NULL AND plugin_export IS NOT NULL)
    )
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
```

### V013 workflows

```sql
CREATE TABLE workflows (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    identifier VARCHAR(64) NOT NULL UNIQUE,
    name VARCHAR(128) NOT NULL,
    description VARCHAR(512) NULL,
    timeout_ms INT NOT NULL DEFAULT 33000,
    required_capabilities JSON NULL COMMENT '计算出的执行所需 capabilities（由 DAG 中所有节点的 function 聚合）',
    category_id BIGINT NULL COMMENT '所属分类',
    input_schema JSON NULL COMMENT '工作流起始节点的输入变量定义 (JSON Schema format)',
    start_description VARCHAR(512) NULL COMMENT '起始节点描述/欢迎语',
    output_schema JSON NULL COMMENT '工作流结束节点的输出变量定义 (JSON Schema format)',
    end_description VARCHAR(512) NULL COMMENT '结束节点描述/结束语',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    INDEX idx_workflows_category (category_id),
    CONSTRAINT fk_workflows_category FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE workflow_nodes (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    workflow_id BIGINT NOT NULL,
    node_key VARCHAR(32) NOT NULL COMMENT '工作流内唯一 key',
    node_type ENUM('function_node','start_node','end_node','generate_answer_node') NOT NULL DEFAULT 'function_node' COMMENT '节点类型',
    function_id BIGINT NULL COMMENT 'function_node 必填，其他类型可为 NULL',
    position JSON NULL COMMENT 'reactflow 坐标 {x, y}',
    node_config JSON NULL COMMENT 'Node-specific configuration (e.g., answer node: system_prompt, model_preset, history_window, variables)',
    UNIQUE KEY uk_workflow_node (workflow_id, node_key),
    INDEX idx_workflow_nodes_workflow (workflow_id),
    CONSTRAINT fk_workflow_nodes_workflow FOREIGN KEY (workflow_id) REFERENCES workflows(id) ON DELETE CASCADE,
    CONSTRAINT fk_workflow_nodes_function FOREIGN KEY (function_id) REFERENCES functions(id) ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE workflow_edges (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    workflow_id BIGINT NOT NULL,
    src_node_id BIGINT NOT NULL,
    dst_node_id BIGINT NOT NULL,
    mapping JSON NOT NULL COMMENT '形如 {"dst.input.foo": "src.output.bar"}',
    INDEX idx_workflow_edges_workflow (workflow_id),
    INDEX idx_workflow_edges_src (src_node_id),
    INDEX idx_workflow_edges_dst (dst_node_id),
    CONSTRAINT fk_workflow_edges_workflow FOREIGN KEY (workflow_id) REFERENCES workflows(id) ON DELETE CASCADE,
    CONSTRAINT fk_workflow_edges_src FOREIGN KEY (src_node_id) REFERENCES workflow_nodes(id) ON DELETE CASCADE,
    CONSTRAINT fk_workflow_edges_dst FOREIGN KEY (dst_node_id) REFERENCES workflow_nodes(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
```

### V014 tools / skills

```sql
CREATE TABLE tools (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    identifier VARCHAR(64) NOT NULL UNIQUE,
    name VARCHAR(128) NOT NULL,
    description VARCHAR(512) NOT NULL,
    kind TINYINT NOT NULL COMMENT '1=function-wrap, 2=workflow-wrap',
    source VARCHAR(16) NOT NULL DEFAULT 'workspace' COMMENT 'workspace | builtin',
    is_always TINYINT(1) NOT NULL DEFAULT 0 COMMENT '0=normal, 1=always available for all agents',
    function_id BIGINT NULL,
    workflow_id BIGINT NULL,
    input_schema JSON NOT NULL,
    output_schema JSON NOT NULL,
    category_id BIGINT NULL COMMENT '所属分类',
    required_capabilities JSON NULL COMMENT '声明该 tool 执行所需的 capabilities',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    CONSTRAINT chk_tools_target CHECK (
        (kind = 1 AND workflow_id IS NULL) OR
        (kind = 2 AND function_id IS NULL)
    ),
    CONSTRAINT fk_tools_function FOREIGN KEY (function_id) REFERENCES functions(id) ON DELETE RESTRICT,
    CONSTRAINT fk_tools_workflow FOREIGN KEY (workflow_id) REFERENCES workflows(id) ON DELETE RESTRICT,
    CONSTRAINT fk_tools_category FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE skills (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    identifier VARCHAR(64) NOT NULL UNIQUE,
    name VARCHAR(128) NOT NULL,
    description VARCHAR(512) NOT NULL COMMENT 'frontmatter 中的简短描述',
    frontmatter JSON NULL COMMENT 'YAML frontmatter 解析后的结构',
    content MEDIUMTEXT NOT NULL COMMENT 'markdown 主体；不超过 64KB 建议',
    source VARCHAR(16) NOT NULL DEFAULT 'workspace' COMMENT 'workspace | builtin',
    is_always TINYINT(1) NOT NULL DEFAULT 0 COMMENT '0=normal, 1=always available for all agents',
    category_id BIGINT NULL COMMENT '所属分类',
    required_capabilities JSON NULL COMMENT '声明该 skill 执行所需的 capabilities',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    CONSTRAINT fk_skills_category FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
```

### V015 agents + 关联表

```sql
CREATE TABLE agents (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    identifier VARCHAR(64) NOT NULL UNIQUE COMMENT '入口固定为 "main"',
    name VARCHAR(128) NOT NULL,
    description VARCHAR(512) NULL,
    system_prompt TEXT NOT NULL,
    parent_agent_id BIGINT NULL,
    depth TINYINT NOT NULL DEFAULT 0 COMMENT '深度，main = 0，子 = parent.depth+1',
    model_preset VARCHAR(64) NULL COMMENT 'hiveweb llm_presets.toml 中的命名 preset；NULL = 全局默认',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    INDEX idx_agents_parent (parent_agent_id),
    INDEX idx_agents_model_preset (model_preset),
    CONSTRAINT fk_agents_parent FOREIGN KEY (parent_agent_id) REFERENCES agents(id) ON DELETE RESTRICT,
    CONSTRAINT chk_agents_depth CHECK (depth >= 0 AND depth <= 10)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE agent_tools (
    agent_id BIGINT NOT NULL,
    tool_id BIGINT NOT NULL,
    PRIMARY KEY (agent_id, tool_id),
    CONSTRAINT fk_at_agent FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE CASCADE,
    CONSTRAINT fk_at_tool  FOREIGN KEY (tool_id)  REFERENCES tools(id)  ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE agent_skills (
    agent_id BIGINT NOT NULL,
    skill_id BIGINT NOT NULL,
    PRIMARY KEY (agent_id, skill_id),
    CONSTRAINT fk_as_agent FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE CASCADE,
    CONSTRAINT fk_as_skill FOREIGN KEY (skill_id) REFERENCES skills(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE agent_permissions (
    agent_id BIGINT NOT NULL,
    capability VARCHAR(64) NOT NULL,
    PRIMARY KEY (agent_id, capability),
    CONSTRAINT fk_ap_agent FOREIGN KEY (agent_id) REFERENCES agents(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
```

### 现行用户聊天兼容边界（V018 + V024；原 V016 admin chat 已 Superseded）

原 V016 admin chat DDL 不再有效，相关表已删除。当前表结构由后续外部 Assistant API 迁移维护；这里仅镜像与 Agent Runtime 的衔接字段，避免误把 legacy admin 表当成现役模型。

```sql
CREATE TABLE chat_sessions_user (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    user_id BIGINT NOT NULL COMMENT '关联的普通用户',
    user_phone_snapshot VARCHAR(100) NOT NULL DEFAULT '' COMMENT '用户快照',
    user_nickname_snapshot VARCHAR(64) NOT NULL DEFAULT '' COMMENT '用户快照',
    title VARCHAR(128) NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    INDEX idx_chat_sessions_user_id (user_id),
    INDEX idx_chat_sessions_user_updated_at (updated_at DESC),
    CONSTRAINT fk_chat_sessions_user_user
        FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE chat_messages_user (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    session_id BIGINT NOT NULL,
    user_id BIGINT NOT NULL COMMENT '关联的普通用户',
    role VARCHAR(16) NOT NULL COMMENT 'user|assistant|tool|system',
    content TEXT NULL,
    elapsed_ms INT NULL COMMENT 'assistant 消息耗时（毫秒）',
    extensions JSON NULL COMMENT 'AgentContext 扩展数据',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    INDEX idx_chat_messages_user_session (session_id, created_at),
    INDEX idx_chat_messages_user_user_id (user_id),
    CONSTRAINT fk_chat_msg_user_session
        FOREIGN KEY (session_id) REFERENCES chat_sessions_user(id) ON DELETE CASCADE,
    CONSTRAINT fk_chat_msg_user_user
        FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
```

### V032 runtime_audit_logs

该表保存非 Hook runtime audit 的脱敏、best-effort 持久化副本；同一事件
始终先写 structured tracing。单 worker 有界队列满或 DB 失败时不阻塞主
流程。Hook 执行保持 tracing-only。表无外键，确保资源删除后仍保留审计；
`AUDIT_RETENTION_DAYS` 默认 36500 天（100 年，可配置）。

```sql
CREATE TABLE runtime_audit_logs (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    request_id VARCHAR(64) NULL COMMENT '与 hiveweb request-id 中间件对齐',
    session_id BIGINT NULL,
    agent_id BIGINT NULL,
    plugin_id BIGINT NULL,
    function_id BIGINT NULL,
    capability VARCHAR(64) NULL,
    event_type VARCHAR(32) NOT NULL COMMENT 'capability_call|capability_denied|plugin_invoke|workflow_node|agent_route|llm_invoke|llm_fallback|llm_local_fallback',
    outcome VARCHAR(16) NOT NULL COMMENT 'success|error|denied|timeout',
    elapsed_ms INT NULL,
    error_message VARCHAR(512) NULL,
    payload_summary JSON NULL COMMENT '入参 / 出参摘要（脱敏后）',
    occurred_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    INDEX idx_ral_request (request_id),
    INDEX idx_ral_session (session_id),
    INDEX idx_ral_agent (agent_id),
    INDEX idx_ral_occurred_at (occurred_at DESC),
    INDEX idx_ral_capability (capability, outcome)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='Sanitized non-Hook runtime audit records';
```

### V018 seed

```sql
-- main Agent（不可删除）
INSERT IGNORE INTO agents (id, identifier, name, description, system_prompt, parent_agent_id, depth)
VALUES (1, 'main', '入口 Agent', '系统的入口 Agent；不可删除',
        'You are the main entry agent. Decide whether to answer directly or route to a sub-agent.',
        NULL, 0);
```

### V019–V020 tools 扩展

- **V019**: `ALTER TABLE tools ADD COLUMN source VARCHAR(16) NOT NULL DEFAULT 'workspace'`
- **V020**: `ALTER TABLE tools ADD COLUMN is_always TINYINT(1) NOT NULL DEFAULT 0`；放宽 CHECK 约束允许 kind=1 时 function_id=NULL（meta-tools）

### V021 skills is_always

```sql
ALTER TABLE skills ADD COLUMN is_always TINYINT(1) NOT NULL DEFAULT 0;
```

### V022–V025 recommended_games

```sql
CREATE TABLE recommended_games (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    reply TEXT NOT NULL,
    reason TEXT COMMENT '推荐理由',
    game_id VARCHAR(128) NOT NULL,
    game_name VARCHAR(255) NOT NULL,
    tag VARCHAR(32) COMMENT '标签：运营推荐/新游上线/本周热玩',
    game_category VARCHAR(64) COMMENT '游戏类型',
    game_image VARCHAR(512) COMMENT '推荐图片地址',
    sort_value INT NOT NULL DEFAULT 0,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    UNIQUE KEY uk_game_id (game_id),
    INDEX idx_name (name),
    INDEX idx_created_at (created_at),
    INDEX idx_sort_value (sort_value)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
```

### V026 tools category

```sql
ALTER TABLE tools ADD COLUMN category_id BIGINT NULL;
ALTER TABLE tools ADD CONSTRAINT fk_tools_category FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE SET NULL;
```

### V027 skills category

```sql
ALTER TABLE skills ADD COLUMN category_id BIGINT NULL;
ALTER TABLE skills ADD CONSTRAINT fk_skills_category FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE SET NULL;
```

### V028 rename audit_logs

```sql
RENAME TABLE audit_logs TO admin_audit_logs;
```

### V029 function & tool required_capabilities

```sql
ALTER TABLE functions ADD COLUMN required_capabilities JSON NULL;
ALTER TABLE tools ADD COLUMN required_capabilities JSON NULL;
```

### V030 workflow required_capabilities

```sql
ALTER TABLE workflows ADD COLUMN required_capabilities JSON NULL;
```

### V031 skill required_capabilities

```sql
ALTER TABLE skills ADD COLUMN required_capabilities JSON NULL;
```

### V032 workflow category

```sql
ALTER TABLE workflows ADD COLUMN category_id BIGINT NULL;
ALTER TABLE workflows ADD CONSTRAINT fk_workflows_category FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE SET NULL;
```

### V033 workflow input_schema

```sql
ALTER TABLE workflows ADD COLUMN input_schema JSON NULL;
ALTER TABLE workflows ADD COLUMN start_description VARCHAR(512) NULL;
```

### V034 workflow_node type

```sql
ALTER TABLE workflow_nodes ADD COLUMN node_type ENUM('function_node','start_node') NOT NULL DEFAULT 'function_node';
```

### V035 workflow output_schema

```sql
ALTER TABLE workflows ADD COLUMN output_schema JSON NULL;
ALTER TABLE workflows ADD COLUMN end_description VARCHAR(512) NULL;
```

### V036 workflow answer_node

```sql
ALTER TABLE workflow_nodes
    MODIFY COLUMN node_type ENUM('function_node','start_node','end_node','generate_answer_node') NOT NULL DEFAULT 'function_node';
ALTER TABLE workflow_nodes MODIFY COLUMN function_id BIGINT NULL;
ALTER TABLE workflow_nodes ADD COLUMN node_config JSON NULL;
```

### V037 capabilities category

```sql
ALTER TABLE capabilities ADD COLUMN category_id BIGINT NULL;
ALTER TABLE capabilities ADD CONSTRAINT fk_capabilities_category FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE SET NULL;
```

### V038 seed capability categories

```sql
-- 预置 Capability 内置分类（与 runtime/capability.rs 中的 CAPABILITIES 对应）
INSERT IGNORE INTO categories (parent_id, name, slug, description) VALUES
    (NULL, '网络', 'network', '网络相关能力'),
    (NULL, '文件系统', 'fs', '文件系统读写能力'),
    (NULL, '对象存储', 's3', '对象存储（Rustfs）能力'),
    (NULL, '数据库', 'db', '数据库查询与执行'),
    (NULL, 'LLM', 'llm', '大语言模型调用'),
    (NULL, '密钥管理', 'secret', '密钥读取与管理'),
    (NULL, '时间', 'time', '服务器时间相关'),
    (NULL, '日志', 'log', '日志写入'),
    (NULL, '聊天', 'chat', '聊天交互'),
    (NULL, '命令执行', 'exec', 'Shell 命令执行'),
    (NULL, 'Agent', 'agent', 'Agent 管理'),
    (NULL, '定时任务', 'cron', 'Cron 任务管理');

-- 更新 capabilities 的 category_id（按 slug 匹配）
UPDATE capabilities c
JOIN categories cat ON cat.slug = SUBSTRING_INDEX(c.name, '.', 1) AND cat.parent_id IS NULL
SET c.category_id = cat.id
WHERE cat.slug IN ('network', 'fs', 's3', 'db', 'llm', 'secret', 'time', 'log', 'chat', 'exec', 'agent', 'cron');
```

</details>

---

## 3. 关系图（简化）

```
Admin --< LoginRecord
Admin --< AdminAuditLog
User --< ChatSessionUser --< ChatMessageUser

Category --< Plugin >--*-- Tag
Category --< Function >--*-- Tag
Category --< Workflow >--*-- Tag
Category --< Tool >--*-- Tag
Category --< Skill >--*-- Tag
Category --< Capability

                |
                v
            Function --< WorkflowNode(node_type: function_node/start_node/end_node/generate_answer_node) >--in Workflow
                |                 \
                v                  >---< WorkflowEdge
              Tool/Skill            mapping
                 \   /
                  v
                 Agent (self-ref tree, depth ≤ 10)
                  |
                  +--*-- Tool
                  +--*-- Skill
                  +--*-- Capability (permissions)
                  |
                  v
        RuntimeAuditLog

RecommendedGame (独立实体)
```

---

## 4. 不变量（必须由 service 层强制）

1. `agents.identifier = 'main'` 的行不可删除（service 层 + FE 双重）
2. `agents.depth = parent.depth + 1`，最大 10（service 层校验）
3. `functions.kind = 2` 必须有 plugin_id + plugin_export，且对应 plugin `deleted_at IS NULL`
4. `plugins.deleted_at IS NULL` 的行被 `functions.plugin_id` 引用时，禁止删除（spec FR-007）
5. `tools.kind` 与 function_id/workflow_id 一一对应（DB CHECK 已表达）
6. `workflow_edges` 不能形成环（service 层 DFS 校验）
7. `agent_permissions.capability` 必须属于 `capabilities.name`（service 层 lookup）
8. 危险 capability（`is_dangerous = 1`）只能由 role=1 Super 授予
9. （历史，已 superseded）原 admin `chat_messages.seq` 单调不变量已随表删除；现行 `chat_messages_user` 不定义 `seq`
10. `agents.model_preset` 取值必须为启动期从 hiveweb `llm_presets.toml` 加载的命名 preset；service 层在保存时校验未知 preset → 5007 `ModelPresetUnknown`。子 Agent 不继承父的 preset；运行时只有 `model_preset IS NULL` 才选择全局 default。显式非 NULL 名称若在当前启动 registry 中未知（例如配置删除或无效 preset 被跳过），必须 fail closed，禁止回退 default
11. `tools.kind=1`（function-wrap）时，`tools.input_schema` / `tools.output_schema` 必须**完全等于**其引用 function 的对应字段；service 层在 PUT/POST tools 时做深度 JSON 等值校验，不一致 → 5002 `Schema mismatch`。`tools.kind=2`（workflow-wrap）时，`tools.input_schema` 必须能赋值给 workflow 入口 function 的 input_schema（至少包含所有 required 字段且类型一致），output_schema 由编辑者声明（默认 = DAG 终点输出）
12. `chat_sessions_user.user_id` 必须对应当前普通用户，API/service 同时校验 `session_id` 与 `user_id`；用户删除时会话/消息按 FK CASCADE 删除。不存在 admin/Super 跨用户读取例外
13. `functions` 表**不支持软删除**（无 `deleted_at` 列）；任何 Function 的 DELETE 都是物理删除。前置检查：被 `workflow_nodes` / `tools` 引用时拒绝（4093）
14. `skills` 不引用 Function/Workflow；删除前必须校验 `agent_skills` 引用计数 > 0 → 4093 拒绝
15. `tools.source = 'builtin'` 的 Tool 不可编辑，只能包装 builtin Function (kind=1)
16. `tools.is_always = 1` 的 Tool 对所有 Agent 自动可用，无需在 `agent_tools` 中建立关联
17. `skills.is_always = 1` 的 Skill 对所有 Agent 自动加载，无需在 `agent_skills` 中建立关联
18. `workflow_nodes.node_type = 'generate_answer_node'` 时，`function_id` 可为 NULL，`node_config` 中应包含 `system_prompt` 等 LLM 生成配置
19. `workflow_nodes.node_type = 'start_node'` 或 `'end_node'` 时，`function_id` 可为 NULL
20. 每个 `LlmPresetName` 的完整 provider chain 在启动期构造并缓存；provider 条目以 `(preset name, providers[] ordinal)` 作为稳定内部 identity，不能以可重复的 model 字符串作为映射键。`runtime_audit_logs.payload_summary` 中的 LLM 元数据只能包含 `actual_model`、`fallback_used`、静态 `reason` 与稳定 provider identity；provider fallback 使用 `llm_fallback`，应用层本地文本使用 `llm_local_fallback`

---

## 5. 索引选型 cheat-sheet

- 全文搜索（spec FR-009）：Plugin / Function 的 `FULLTEXT (name, description, identifier)`，MySQL `MATCH ... AGAINST`
- 高频路径：
  - `idx_plugins_category` + 标签经 `taggings (entity_type='plugin')` 关联
  - `idx_taggings_entity` 实现 "某 entity 的全部 tag" 反向查询
  - `idx_chat_sessions_user_updated_at` 普通用户最近会话列表（外部 Assistant API）
  - `idx_ral_occurred_at` + `idx_ral_capability` 用于 capability 鉴权审计的反查
  - `idx_workflows_category` / `idx_tools_category` / `idx_skills_category` / `idx_functions_category` 分类过滤
  - `idx_sort_value` 推荐游戏排序

---

## 6. Capability 静态注册表（代码侧，参考）

非 DB 实体，列在此让设计完整：

| name | 描述 | is_dangerous |
| --- | --- | --- |
| `network.http` | HTTP/HTTPS GET/POST/PUT/DELETE（含 allowlist 域名） | 1 |
| `fs.read` | 受控临时目录读 | 0 |
| `fs.write` | 受控临时目录写 | 0 |
| `s3.read` | Rustfs 桶 GET | 0 |
| `s3.write` | Rustfs 桶 PUT/DELETE | 0 |
| `db.query` | 命名 SELECT（不接受自由 SQL） | 0 |
| `db.execute` | 命名 DML（INSERT/UPDATE/DELETE）；自由 SQL 永远禁用 | 1 |
| `llm.invoke` | 调 LLM（OpenAI 兼容） | 0 |
| `secret.get` | 读取 vault 中按 key 注册的密钥 | 1 |
| `time.now` | 服务器当前时间（避免 Plugin 自己引入时钟） | 0 |
| `log.emit` | 写一行结构化日志（受 rate-limit） | 0 |
