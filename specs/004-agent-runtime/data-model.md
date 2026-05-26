# Data Model: Agent Runtime

**Created**: 2026-05-26
**Status**: Phase 1 output

---

## 1. 实体一览

| 实体 | 表名 | 关键关系 |
| --- | --- | --- |
| Category | `categories` | self-FK parent_id |
| Tag | `tags` | — |
| Plugin | `plugins` | category_id, taggable |
| Function | `functions` | plugin_id (custom 才有), category_id, taggable |
| Workflow | `workflows` | nodes/edges 在子表 |
| WorkflowNode | `workflow_nodes` | workflow_id, function_id |
| WorkflowEdge | `workflow_edges` | workflow_id, src_node_id, dst_node_id, mapping |
| Tool | `tools` | function_id OR workflow_id（互斥） |
| Skill | `skills` | function_id OR workflow_id（互斥） |
| Agent | `agents` | self-FK parent_agent_id |
| AgentTool（多对多） | `agent_tools` | agent_id, tool_id |
| AgentSkill（多对多） | `agent_skills` | agent_id, skill_id |
| AgentPermission | `agent_permissions` | agent_id, capability_name |
| Taggable（通用 polymorphic） | `taggings` | tag_id + entity_type + entity_id |
| ChatSession | `chat_sessions` | admin_id（操作者） |
| ChatMessage | `chat_messages` | session_id |
| RuntimeAuditLog | `runtime_audit_logs` | session_id?, agent_id, plugin_id, capability |

Capability 是**代码内静态注册表**，不入库；只有"描述/危险标记"可选入 `capabilities`（只读元数据表）。

---

## 2. 详细字段 + DDL（迁移按顺序 V008..V020）

### V008 capabilities (元数据，只读)

```sql
CREATE TABLE capabilities (
    name VARCHAR(64) PRIMARY KEY COMMENT 'e.g. network.http',
    description VARCHAR(255) NOT NULL,
    is_dangerous TINYINT(1) NOT NULL DEFAULT 0 COMMENT '需 Super 才能授予',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
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
-- 多版本共存：unique 在 (identifier, version) 上。
-- 软删除：deleted_at IS NOT NULL；引用检查见 services::plugin。
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
-- builtin function 由代码注册时 INSERT IGNORE 写入，identifier 即 builtin 名。
-- FK ON DELETE RESTRICT 实现 spec FR-007 的"plugin 被 function 引用时不能软删除"
-- （应用层另判断 deleted_at NULL 即可）。
```

### V013 workflows

```sql
CREATE TABLE workflows (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    identifier VARCHAR(64) NOT NULL UNIQUE,
    name VARCHAR(128) NOT NULL,
    description VARCHAR(512) NULL,
    timeout_ms INT NOT NULL DEFAULT 30000,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE workflow_nodes (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    workflow_id BIGINT NOT NULL,
    node_key VARCHAR(32) NOT NULL COMMENT '工作流内唯一 key',
    function_id BIGINT NOT NULL,
    position JSON NULL COMMENT 'reactflow 坐标 {x, y}',
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
    kind TINYINT NOT NULL COMMENT '1=function, 2=workflow',
    function_id BIGINT NULL,
    workflow_id BIGINT NULL,
    input_schema JSON NOT NULL,
    output_schema JSON NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    CONSTRAINT chk_tools_target CHECK (
        (kind = 1 AND function_id IS NOT NULL AND workflow_id IS NULL) OR
        (kind = 2 AND workflow_id IS NOT NULL AND function_id IS NULL)
    ),
    CONSTRAINT fk_tools_function FOREIGN KEY (function_id) REFERENCES functions(id) ON DELETE RESTRICT,
    CONSTRAINT fk_tools_workflow FOREIGN KEY (workflow_id) REFERENCES workflows(id) ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Skill 沿用 crates/agent::SkillsLoader 的"markdown + frontmatter"模式：
-- 调用 Skill = 把 content 拼到 Agent 的 system prompt 末尾。
-- 因此 Skill 不持有 schema、不引用 Function/Workflow（如需调函数，由 Skill
-- 内容描述 + Agent LLM 在 system_prompt 指引下自行选用已绑定的 Tool）。
--
-- 删除策略（CHK166，与 FR-007 / FR-009 引用阻塞原则对齐）：
--   * `agent_skills` 是多对多关系；删除 Skill 前 service 层必须先校验
--     "SELECT COUNT(*) FROM agent_skills WHERE skill_id = ?"，> 0 → 4093 拒绝。
--   * 不依赖 DB CASCADE（CASCADE 会悄无声息从 Agent 列表移除 Skill，与"删除前必须看到引用列表"的运营预期冲突）。
--   * 强引用阻塞 = 与 Tool / Function / Plugin 一致的删除模型。
CREATE TABLE skills (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    identifier VARCHAR(64) NOT NULL UNIQUE,
    name VARCHAR(128) NOT NULL,
    description VARCHAR(512) NOT NULL COMMENT 'frontmatter 中的简短描述',
    frontmatter JSON NULL COMMENT 'YAML frontmatter 解析后的结构',
    content MEDIUMTEXT NOT NULL COMMENT 'markdown 主体；不超过 64KB 建议',
    source VARCHAR(16) NOT NULL DEFAULT 'workspace' COMMENT 'workspace | builtin',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
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
-- main agent 由 seed 写入 (identifier='main', depth=0)，与"不可删除"约束在 service 层强制。
```

### V016 chat

```sql
CREATE TABLE chat_sessions (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    admin_id BIGINT NULL COMMENT '发起测试的管理员；admin 删除后 SET NULL，session 仅 Super 可访问',
    admin_phone_snapshot VARCHAR(11) NOT NULL DEFAULT '' COMMENT '快照：admin 删除后仍可追溯发起者',
    admin_nickname_snapshot VARCHAR(20) NOT NULL DEFAULT '' COMMENT '快照：admin 删除后仍可追溯发起者',
    title VARCHAR(128) NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    INDEX idx_chat_sessions_admin (admin_id),
    INDEX idx_chat_sessions_updated_at (updated_at DESC),
    CONSTRAINT fk_chat_sessions_admin FOREIGN KEY (admin_id) REFERENCES admins(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
-- 所有权语义（与 FR-027 v7 对齐）：
--   * admin 仍存在 → service 层校验 JWT.admin_id == admin_id 才放行；Super 例外
--   * admin 已删除（admin_id IS NULL）→ session 仅 Super 可访问，普通管理员一律 403
--   * snapshot 列保证 audit 追溯到具体管理员，与 V004 login_records 模式一致（CHK176）

CREATE TABLE chat_messages (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    session_id BIGINT NOT NULL,
    seq INT NOT NULL COMMENT '会话内单调序号',
    role VARCHAR(16) NOT NULL COMMENT 'user|assistant|tool|system',
    content TEXT NULL,
    tool_calls JSON NULL COMMENT '[{tool_call_id, name, args}]',
    routed_to_agent_id BIGINT NULL COMMENT '如果该消息触发了路由',
    elapsed_ms INT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE KEY uk_chat_msg_session_seq (session_id, seq),
    INDEX idx_chat_messages_session (session_id, created_at),
    CONSTRAINT fk_chat_msg_session FOREIGN KEY (session_id) REFERENCES chat_sessions(id) ON DELETE CASCADE,
    CONSTRAINT fk_chat_msg_routed FOREIGN KEY (routed_to_agent_id) REFERENCES agents(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
```

### V017 runtime_audit_logs

> **与 V006 `audit_logs`（003 admin 操作审计）的职责边界**（CHK124 / CHK157）：
> - V006 `audit_logs`：**管理员对配置数据的写操作**审计（Plugin/Function/Workflow/Agent CRUD、登录、修改 system_prompt 等）。沿用 003 既有，键字段是 `actor_admin_id` + `entity_type` + `entity_id`。
> - V017 `runtime_audit_logs`：**Agent / Plugin 在运行时对资源的访问**审计（host_call dispatch、Workflow 节点执行、Agent 路由、LLM 调用）。键字段是 `agent_id` + `capability` + `outcome`。
> - 两表互不替代，互不重叠；查同一个 `request_id` 时可 JOIN 两表得到"管理员配置 → 运行时使用"的完整链路。
> - 都假定与 003 V006 在**同一数据库实例**（CHK181 假设），跨实例时 JOIN 不可用。

```sql
CREATE TABLE runtime_audit_logs (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    request_id VARCHAR(64) NULL COMMENT '与 hiveweb request-id 中间件对齐',
    session_id BIGINT NULL,
    agent_id BIGINT NULL,
    plugin_id BIGINT NULL,
    function_id BIGINT NULL,
    capability VARCHAR(64) NULL,
    event_type VARCHAR(32) NOT NULL COMMENT 'capability_call|capability_denied|plugin_invoke|workflow_node|agent_route|llm_invoke',
    outcome VARCHAR(16) NOT NULL COMMENT 'success|error|denied|timeout',
    elapsed_ms INT NULL,
    error_message VARCHAR(512) NULL,
    payload_summary JSON NULL COMMENT '入参 / 出参摘要（脱敏后）',
    occurred_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    INDEX idx_ral_request (request_id),
    INDEX idx_ral_session (session_id),
    INDEX idx_ral_agent (agent_id),
    INDEX idx_ral_occurred_at (occurred_at DESC),
    INDEX idx_ral_capability (capability, outcome)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Runtime 审计日志（保留 ≥ 90 天，同 003 FR-022 精神）';
```

### V018 seed main agent + builtin capabilities 元数据

```sql
-- 1. main Agent（不可删除）
INSERT IGNORE INTO agents (id, identifier, name, description, system_prompt, parent_agent_id, depth)
VALUES (1, 'main', '入口 Agent', '系统的入口 Agent；不可删除',
        'You are the main entry agent. Decide whether to answer directly or route to a sub-agent.',
        NULL, 0);
-- main 默认无任何 capability / tool / skill；管理员在 UI 上后续配置。

-- 2. capabilities 元数据（is_dangerous 标记 + 描述供 UI 渲染）
INSERT IGNORE INTO capabilities (name, description, is_dangerous) VALUES
  ('network.http',  'HTTP/HTTPS access (allowlisted hosts; SSRF-blocked)', 1),
  ('fs.read',       '/tmp/plugin/ 内文件读', 0),
  ('fs.write',      '/tmp/plugin/ 内文件写', 0),
  ('s3.read',       'Rustfs 桶 GET', 0),
  ('s3.write',      'Rustfs 桶 PUT / DELETE', 0),
  ('db.query',      '宿主预注册命名 SELECT 查询', 0),
  ('db.execute',    '宿主预注册命名 DML（永不自由 SQL）', 1),
  ('llm.invoke',    'LLM 调用（走 Agent.model_preset 解析）', 0),
  ('secret.get',    'allowlist 内的密钥读取', 1),
  ('time.now',      '服务器当前时间', 0),
  ('log.emit',      '结构化日志写入（rate-limited）', 0);
```

**Builtin function 不通过 V018 seed**：FR-010 v5 的 5 个 builtin function（`format.template` / `json.parse` / `json.stringify` / `text.regex_match` / `chat.respond`）的 schema 是代码内的常量，**启动期由 service 层 idempotent upsert 到 `functions` 表**（`kind = 1`，`plugin_id IS NULL`）。理由：
1. schema 跟随代码版本演进；DDL seed 一旦写入难以跟踪 schema 变更
2. 启动期 upsert 是 init-once 操作，多实例并发启动时由 MySQL UNIQUE(identifier) 兜底
3. 与 V008 `capabilities` 元数据的"代码 = 真值源，DB = 镜像"原则一致（CHK160）

启动期与 DB 的同步策略（CHK156 / CHK161）：
- 代码常量列表 → upsert 到 DB（INSERT ... ON DUPLICATE KEY UPDATE schema, updated_at）
- DB 中存在但代码已移除的项 → 启动 warn 日志（不自动删除，避免误删历史数据）
- 真值源：代码 > DB

---

## 3. 关系图（简化）

```
Category --< Plugin >--*-- Tag
                |
                v
            Function --< WorkflowNode >--in Workflow
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
              ChatSession --< ChatMessage (routes_to Agent)
                  |
                  v
        RuntimeAuditLog
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
8. 危险 capability（`is_dangerous = 1`）只能由 role=3 Super 授予
9. `chat_messages.seq` 在 `session_id` 内单调（DB UNIQUE 已表达）
10. `agents.model_preset` 取值必须为启动期从 hiveweb `llm_presets.toml` 加载的命名 preset；service 层在保存时校验未知 preset → 5007 `ModelPresetUnknown`。子 Agent 不继承父的 preset；运行时解析顺序：当前 Agent.model_preset → 全局默认 preset
11. `tools.kind=1`（function-wrap）时，`tools.input_schema` / `tools.output_schema` 必须**完全等于**其引用 function 的对应字段；service 层在 PUT/POST tools 时做深度 JSON 等值校验，不一致 → 5002 `Schema mismatch`。`tools.kind=2`（workflow-wrap）时，`tools.input_schema` 必须能赋值给 workflow 入口 function 的 input_schema（至少包含所有 required 字段且类型一致），output_schema 由编辑者声明（默认 = DAG 终点输出）。（CHK170）
12. `chat_sessions.admin_id IS NULL` 时（操作者已被删除），该 session 仅 Super 角色可读 / 可继续对话 / 可删除；普通管理员一律 403。snapshot 列用于审计追溯。service 层强制；DB 不加 CHECK。（CHK142 / CHK148）
13. `functions` 表**不支持软删除**（无 `deleted_at` 列）；任何 Function 的 DELETE 都是物理删除。前置检查：被 `workflow_nodes` / `tools` / `skills` 引用时拒绝（4093）。`plugins` 软删除时，引用其的 Function 仍存在但 service 层在调用时返 `5004 Plugin missing` 或类似错误（FR-007 软删除语义）。（CHK166 / CHK173）

---

## 5. 索引选型 cheat-sheet

- 全文搜索（spec FR-009）：Plugin / Function 的 `FULLTEXT (name, description, identifier)`，MySQL `MATCH ... AGAINST`
- 高频路径：
  - `idx_plugins_category` + 标签经 `taggings (entity_type='plugin')` 关联
  - `idx_taggings_entity` 实现 "某 entity 的全部 tag" 反向查询
  - `idx_chat_sessions_updated_at` 最近会话列表
  - `idx_ral_occurred_at` + `idx_ral_capability` 用于 capability 鉴权审计的反查

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
