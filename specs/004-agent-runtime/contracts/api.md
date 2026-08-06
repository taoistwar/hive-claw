# HTTP API Contract — Agent Runtime

> **范围更新（2026-07-23）：** 原管理端测试聊天 API（`/api/admin-chat*`、`/api/chat/sessions*`）已由 `81a84fe` 移除并 superseded，本契约不得用于恢复它们。现行普通用户聊天是独立的外部 Assistant API：`/api/assistant`、`/api/newsession`、`/api/messages`，由 `web-user` 使用且不属于下述管理中心 JWT 契约。

**Created**: 2026-05-26
**Auth**: 全部端点都需要管理中心 JWT（沿用 003）。`POST /api/auth/login` 之外的所有端点都走 `Authorization: Bearer <jwt>`。Capability / 资源 CRUD 默认要求 role ≥ System（2），Agent permission 修改要求 Super（3）（与 spec FR-022 / FR-026 对齐）。
**Envelope**: 沿用 003 的 `{ code, message, data }`。错误码沿用 003 的 1001/1004/2001/3001/3002 等，新增本特性专属在 §Errors 列出。
**Base URL**: `/api`

---

## 0. 公共契约（CHK184 / CHK191 / CHK199 / CHK208）

### 分页参数（所有 GET 列表端点）

`offset`（默认 0）+ `limit`（默认 20，最大 100）。`offset` 超出 `total` 时返回 `200 + { items: [], total, offset, limit }`（**不**返 404）。

### 响应 envelope（沿用 003）

```json
{ "code": <u16>, "message": <string>, "data": <object | array | null> }
```

`code = 0` = 成功；非 0 见 §Errors 表。

### 错误响应的 `data.params`（用于占位符替换，CHK191）

```json
{ "code": 4030, "message": "Capability denied", "data": { "params": { "capability": "network.http" } } }
```

前端 `error_messages.ts` 根据 `code` 查模板（如 `当前 Agent 未授权调用能力「{capability}」`）并用 `params` 替换占位符。`params` 字段固定为对象，可缺省（部分错误码无参数）。

### host_call 响应（CHK199）

`ok: false` 时**禁止**携带 `data` 字段；`ok: true` 时**必须**携带 `data`。Plugin 作者无需做 `data` 存在性判断 — 只看 `ok` 即可。

### API 版本化（CHK208）

MVP 不引入路径或 header 版本化，所有端点直接挂 `/api/...`。任何破坏性变更需走"宪法 v 升级 + 短期双兼"流程而非简单引版本前缀；非破坏性增量（新字段、新端点、新错误码）原地添加。

---

## 1. Capabilities

### GET /api/capabilities

List static registry of capabilities.

Response 200:
```json
{ "code": 0, "message": "success", "data": [
  { "name": "network.http", "description": "HTTP/HTTPS access (allowlisted hosts)", "is_dangerous": true },
  { "name": "llm.invoke", "description": "Call configured LLM", "is_dangerous": false }
]}
```

---

## 2. Categories

| Method | Path | Description |
| --- | --- | --- |
| GET | `/api/categories` | tree（支持 `?flat=1` 拉平） |
| POST | `/api/categories` | `{ parent_id?, name, slug, description? }` |
| PUT | `/api/categories/:id` | 同上 |
| DELETE | `/api/categories/:id` | 软删除？此处选硬删（FK SET NULL） |

---

## 3. Tags

| Method | Path | Description |
| --- | --- | --- |
| GET | `/api/tags` | flat list；`?q=keyword` 过滤 |
| POST | `/api/tags` | `{ name, color? }` |
| PUT | `/api/tags/:id` | |
| DELETE | `/api/tags/:id` | 若被引用，返回 4091 + 引用计数 |

---

## 4. Plugins

### GET /api/plugins
Query:
- `offset` / `limit`（默认 0 / 20）
- `search` 全文（name / description / identifier）
- `category_id`
- `tag_ids` 逗号分隔
- `include_deleted` 默认 false

Response data:
```json
{
  "items": [{
    "id": 1,
    "identifier": "weather-tool",
    "version": "1.0.0",
    "name": "Weather Tool",
    "description": "Get weather by city",
    "category_id": 3,
    "tags": [{"id": 2, "name": "official"}],
    "author": "alice",
    "size_bytes": 184320,
    "created_at": "2026-05-26T10:00:00Z",
    "deleted_at": null
  }],
  "total": 1,
  "offset": 0,
  "limit": 20
}
```

### POST /api/plugins
`multipart/form-data`:
- `file`: WASM 文件
- `meta`: JSON `{ identifier, name, version, manifest?, runtime: "extism", description?, author?, repository_url?, category_id?, tag_ids?: [int] }`

Server-side:
- 校验 manifest（不必完整解析，至少检查不是空二进制）
- 计算 sha256 + size，校验唯一 `(identifier, version)`
- 上传至 Rustfs `plugins/<identifier>/<version>.wasm`
- DB 插入

Response 201：plugin 对象。

### PUT /api/plugins/:id
仅允许修改元数据（name / description / category_id / tags / author / repository_url）；不允许换 WASM 文件（要换则上传新 version）。

### DELETE /api/plugins/:id
软删除。先校验 `SELECT COUNT(*) FROM functions WHERE plugin_id = ? AND deleted_at IS NULL` = 0；否则 4093（PluginInUse）。

---

## 5. Functions

| Method | Path | Description |
| --- | --- | --- |
| GET | `/api/functions` | filter: `?kind=builtin|custom`、`?search=`、`?category_id=`、`?tag_ids=` |
| POST | `/api/functions` | builtin 由代码注册不开放 API；只允许 kind=custom |
| PUT | `/api/functions/:id` | 仅允许修改 name / description / category / tags / schemas（注意：改 schema 是破坏性的，前端二次确认） |
| DELETE | `/api/functions/:id` | builtin 拒绝；custom 软删除？暂硬删（无引用时） |
| POST | `/api/functions/:id/invoke` | RPC 式调用单个 Function（builtin 走宿主 handler，custom 走 WASM Plugin invoker） |

Body for POST:
```json
{
  "identifier": "weather.lookup",
  "name": "查天气",
  "description": "by city name",
  "plugin_id": 1,
  "plugin_export": "lookup",
  "input_schema": { "type": "object", "properties": { "city": { "type": "string" }}, "required": ["city"] },
  "output_schema": { "type": "object", "properties": { "temp_c": { "type": "number" }}, "required": ["temp_c"] },
  "category_id": 3,
  "tag_ids": [2]
}
```

### POST /api/functions/:id/invoke

将 Function 当作单一 RPC 调用执行。builtin（kind=1）直接走宿主 handler，custom（kind=2）走 WASM Plugin invoker。

Body:
```json
{
  "input": { "template": "Hello {name}", "vars": { "name": "World" } },
  "agent_id": 1
}
```
- `input`: 对应 Function 的 `input_schema`，必须是 JSON object
- `agent_id`: 可选，默认 1（main agent），用于 Capability 鉴权

Response 200:
```json
{
  "code": 0,
  "data": {
    "output": "Hello World",
    "elapsed_ms": 3
  }
}
```

---

## 6. Workflows

### GET /api/workflows / POST / PUT / DELETE
metadata CRUD（不含节点/边）；其中 POST/PUT 只接受 `{identifier, name, description?, timeout_ms?}`。

### GET /api/workflows/:id/graph
Returns full DAG:
```json
{
  "workflow": {"id": 5, "identifier": "ingest", "name": "Ingest pipeline", "timeout_ms": 33000},
  "nodes": [
    {"id": 11, "node_key": "fetch", "function_id": 21, "position": {"x": 100, "y": 50}},
    {"id": 12, "node_key": "parse", "function_id": 22, "position": {"x": 300, "y": 50}}
  ],
  "edges": [
    {"id": 31, "src_node_id": 11, "dst_node_id": 12,
     "mapping": {"raw_text": "fetch.body"}}
  ]
}
```

### PUT /api/workflows/:id/graph
Full replacement of nodes+edges. Server validates:
- 所有 `function_id` 存在且未删除
- 无环（DFS cycle detection）
- 入参 mapping 引用的 src.output.* 存在于上游 function.output_schema

Body shape mirrors GET response (sans ids — server reassigns).

### POST /api/workflows/:id/execute
Body: `{ "input": { ... } }` — 注入到 DAG 入口 node 的 input
Response data:
```json
{ "workflow_id": 5, "outputs": { ... }, "node_results": { "fetch": {...}, "parse": {...} }, "elapsed_ms": 423 }
```

---

## 7. Tools

| Method | Path | Description |
| --- | --- | --- |
| GET | `/api/tools` | |
| POST | `/api/tools` | `{identifier, name, description, kind: 1\|2, function_id?, workflow_id?, input_schema, output_schema}` |
| PUT | `/api/tools/:id` | |
| DELETE | `/api/tools/:id` | |

Server enforces：
- `kind=1` → 必须 function_id；schemas 必须等于 function 的 schemas（拒绝不一致）
- `kind=2` → 必须 workflow_id；input_schema 必须能赋值给 workflow 入口节点

---

## 8. Skills

Skill 是 **markdown 文档**（沿用 `crates/agent::SkillsLoader` 模式）；调用 Skill = 把 `content` 拼到当前 Agent 的 system prompt。Skill **不**出现在 LLM 的 OpenAI tool-calling 列表中。

### GET /api/skills

Query:
- `offset` / `limit`
- `search` 在 name / description / content 中全文匹配
- `source` 过滤 `workspace | builtin`

Response data:
```json
{
  "items": [{
    "id": 1,
    "identifier": "code-review",
    "name": "Code Review",
    "description": "Review code changes for correctness and style",
    "frontmatter": { "tags": ["coding", "review"], "version": "1.0" },
    "content": "# Code Review\n\nWhen reviewing code, focus on:\n- Correctness\n- ...",
    "source": "workspace",
    "created_at": "2026-05-26T10:00:00Z",
    "updated_at": "2026-05-26T10:00:00Z"
  }],
  "total": 1,
  "offset": 0,
  "limit": 20
}
```

### POST /api/skills

Body：
```json
{
  "identifier": "code-review",
  "name": "Code Review",
  "description": "Review code changes for correctness and style",
  "frontmatter": { "tags": ["coding", "review"], "version": "1.0" },
  "content": "# Code Review\n\nWhen reviewing code, focus on:\n- Correctness\n- ..."
}
```

`source` 由 server 设为 `workspace`（手工创建）；`builtin` 仅由代码注册时插入。

Server-side：
- `identifier` 唯一性校验
- `content` 大小上限 64 KB（超出 → 4001 / BadRequest）
- 解析 `frontmatter` 校验 JSON 结构（如果包含）

### PUT /api/skills/:id

允许修改：name、description、frontmatter、content。`identifier` 与 `source` 不可改。**`updated_at` 乐观锁**：请求体须携带 client 读到的 `updated_at`，与 DB 不一致 → 409 + 4094 `OptimisticLockConflict`。

### DELETE /api/skills/:id

- `source = "builtin"` → 拒绝（5008 `BuiltinSkillProtected`）
- 否则硬删除（Skill 不被其它实体强引用 — Agent.skills 是多对多，CASCADE 走 `agent_skills`）

### Errors

| Code | 含义 |
| --- | --- |
| 4001 | content 超 64 KB / frontmatter 不是合法 JSON |
| 4094 | OptimisticLockConflict（与 Plugin/Workflow/Tool/Agent 同 — 见 §Errors） |
| 5008 | Builtin skill cannot be deleted |

---

## 9. Agents

### GET /api/agents
Returns tree:
```json
{ "code": 0, "data": [{
  "id": 1, "identifier": "main", "name": "入口 Agent",
  "depth": 0, "parent_agent_id": null,
  "children": [{
    "id": 2, "identifier": "coding-expert", "depth": 1, "parent_agent_id": 1, "children": []
  }]
}]}
```

### GET /api/agents/:id
单 Agent 详情，含 tools / skills / permissions / model_preset：
```json
{
  "id": 2, "identifier": "coding-expert", "name": "编程专家", "description": "...",
  "system_prompt": "...", "parent_agent_id": 1, "depth": 1,
  "model_preset": "code-expert",
  "tools": [{"id": 10, "identifier": "search", "name": "搜索"}],
  "skills": [],
  "permissions": ["llm.invoke", "network.http"]
}
```

`model_preset` 为 NULL 时表示该 Agent fallback 到启动期全局默认 preset。

### GET /api/agents/model-presets
返回 hiveweb 启动时从 `llm_presets.toml` 加载的命名 preset 列表，供 Agent 编辑表单的下拉选项使用。每个 preset 内部封装一个 `providers::FallbackProvider`（primary + fallback 链）：
```json
{ "code": 0, "data": [
  {"name": "cheap-fast", "description": "GPT-4o-mini primary, Claude Haiku fallback", "is_default": true},
  {"name": "code-expert", "description": "Claude Opus primary, GPT-4o fallback", "is_default": false}
]}
```

### POST /api/agents
Body：上述结构（不含 id / depth / children）。server 推导 depth = parent.depth + 1，拒绝 depth > 10。若包含 `model_preset` 且该名不存在于 hiveweb 启动加载的 preset 集合 → 422 + 5007 `ModelPresetUnknown`。

### PUT /api/agents/:id
metadata + tool/skill/permission 重置（全替换语义）。修改 permissions 含危险 capability 时要求 Super。

### DELETE /api/agents/:id
- 若 identifier = 'main' → 403 + code 5001（CannotDeleteMain）
- 若有子 Agent → 4093
- 否则硬删

---

## 9b. Runtime Metrics

### GET /api/runtime/pool/stats

Role: System+. 返回 Instance Pool 当前快照（CHK238 / quickstart §11）：

```json
{ "code": 0, "data": {
  "global": { "in_use": 3, "idle": 12, "created_total": 15, "cache_misses": 4, "wait_count": 0, "reset_failures": 0 },
  "per_plugin": [
    { "plugin_id": 1, "identifier": "weather", "version": "1.0.0", "in_use": 1, "idle": 7, "cache_misses": 1 },
    { "plugin_id": 2, "identifier": "translator", "version": "2.0.1", "in_use": 2, "idle": 5, "cache_misses": 3 }
  ]
}}
```

`cache_misses` 仅在冷启动或归还失败丢弃实例时增长；稳态应为常数。运维通过此端点诊断"为什么 Plugin 调用慢"。

---

## 10. Legacy 管理端 Chat（已 Superseded；无现役端点）

原 admin session CRUD + SSE 契约已经删除。本节不定义任何可调用端点；特别是：

- 不存在 `/api/admin-chat*`
- 不存在 `/api/chat/sessions*`
- `web-admin` 不提供 `ChatPage` / `ChatStream`
- 不存在 admin chat 的 EventSource、keep-alive、事件 payload 或 per-admin 并发契约

现行用户聊天由 `/api/assistant`、`/api/newsession`、`/api/messages` 提供，使用其自己的签名/用户隔离契约；`/api/assistant` 返回 JSON，不继承本节已删除的 SSE 设计。详细定义见 `007-external-assistant-api` 与当前实现。

---

## Errors (本特性专属)

| Code | HTTP | 内部含义 | 用户可见消息（zh-CN，前端展示文本必须用此版本） |
| --- | --- | --- | --- |
| 4030 | 403 | Capability denied（运行时；写在 audit + 返回给 caller） | `当前 Agent 未授权调用能力「{capability}」` |
| 4045 | 400 | Capability unknown（plugin 调用了不存在的 capability；原 4040，因与 003 NOT_FOUND 冲突改 4045） | `Plugin 试图调用未知能力「{capability}」` |
| 4091 | 409 | Tag in use（删除阻塞） | `标签被 {N} 个对象引用，无法删除` |
| 4092 | 409 | DAG cycle detected | `工作流中存在环，请检查节点 {node_keys} 之间的连线` |
| 4093 | 409 | Plugin/Workflow/Function/Agent in use（删除阻塞） | `{resource_type} 被 {N} 个对象引用，无法删除` |
| 4094 | 409 | OptimisticLockConflict（PUT 请求携带的 updated_at 与 DB 不一致） | `内容已被他人修改，请刷新后重试` |
| 5001 | 403 | Cannot delete main agent | `入口 Agent「main」不可删除` |
| 5002 | 422 | Schema mismatch（tool/function schema 不一致） | `Tool 与 Function 的输入/输出结构不匹配，请重新选择或调整` |
| 5003 | 403 | Capability denied during Agent invocation | `当前 Agent 没有执行本次请求所需的能力（{capability}）` |
| 5004 | 408 | Plugin invocation timeout | `Plugin 执行超过 {timeout_ms} 毫秒已被中止` |
| 5005 | 422 | Workflow node input mapping invalid | `节点「{node_key}」的输入映射「{field}」无效` |
| 5006 | 422 | Agent depth exceeded（> 10） | `Agent 层级已达最大深度 10，无法继续添加子 Agent` |
| 5007 | 422 | Unknown model preset | `模型 preset「{preset}」不存在，请重新选择` |
| 5008 | 403 | Builtin skill cannot be deleted | `内置技能「{identifier}」不可删除` |
| 5009 | 503 | Plugin instance pool busy（acquire 等待超时） | `Plugin 实例池繁忙，请稍后重试` |

> 历史错误码 `4291` 曾用于 admin SSE 并发上限，现已随管理端 Chat superseded，不是 004 现役错误契约。

**前端实现要求**（CHK020 决议）：
1. 所有错误展示组件（toast / message / inline）必须**只**使用上表"用户可见消息"列的文案；不得直接展示后端 `message` 字段的英文原文。
2. 占位符 `{name}` 由 API 在响应的 `data.params` 中提供（如 `{ "capability": "network.http" }`），前端做模板替换。
3. 错误码 → 文案的映射在 `web-admin/src/utils/error_messages.ts` 集中维护；新增错误码必须同步本表 + 文件。

---

## Capability Coverage Matrix

| Endpoint | 所需 Role | 备注 |
| --- | --- | --- |
| GET /capabilities | ≥ Normal | 只读元数据 |
| Plugin/Function/Workflow/Tool/Skill/Category/Tag CRUD | ≥ System | 写操作 |
| Agent CRUD（含 permissions 中不含 dangerous） | ≥ System | |
| Agent permissions 含 dangerous capability | Super | |
| Workflow execute | ≥ System | 直接执行 |
