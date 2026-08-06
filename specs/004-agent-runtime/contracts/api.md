# HTTP API Contract — Agent Runtime

> **范围更新（2026-07-23）：** 原管理端测试聊天 API（`/api/admin-chat*`、`/api/chat/sessions*`）已由 `81a84fe` 移除并 superseded，本契约不得用于恢复它们。现行普通用户聊天是独立的外部 Assistant API：`/api/assistant`、`/api/newsession`、`/api/messages`，由 `web-user` 使用且不属于下述管理中心 JWT 契约。

**Created**: 2026-05-26
**Auth**: 全部端点都需要管理中心 JWT（沿用 003）。`POST /api/auth/login` 之外的所有端点都走 `Authorization: Bearer <jwt>`。Capability / 资源 CRUD 默认要求 role ≥ System（2），Agent permission 修改要求 Super（3）（与 spec FR-022 / FR-026 对齐）。
**Envelope**: 沿用 003 的 `{ code, message, data }`。错误码沿用 003 的 1001/1004/2001/3001/3002 等，新增本特性专属在 §Errors 列出。
**Base URL**: `/api`

---

## 0. 公共契约（CHK184 / CHK191 / CHK199 / CHK208）

### `X-Request-Id` 信任边界

客户端值仅在它是固定 36 字节 canonical non-nil UUID 时接受，并统一规范化为
小写；其他形状（含 simple/braced UUID、任意 trace 文本、nil UUID、超长或
包含控制/分隔字符的值）均丢弃并生成新的 UUID v4。边界 middleware 在调用
任何下游 middleware/handler 前，必须以该值覆盖请求 `HeaderMap` 中的
`X-Request-Id`。handler 读取、响应 header、`RuntimeExecutionContext`、
runtime audit 与 Hook tracing 只使用同一规范化值，不得记录、暴露或安全化后
继续传播原始不可信 header。

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
软删除。事务内先以 `SELECT ... FOR UPDATE` 锁定 Plugin，再校验
`SELECT COUNT(*) FROM functions WHERE plugin_id = ?` = 0；否则 4093
（PluginInUse）。`functions` 是硬删除模型，没有 `deleted_at` 列，因此这里
必须统计全部现存 Function。并发创建 Function 会在自己的事务中以
`SELECT ... FOR SHARE` 锁定同一 Plugin，并持锁至 Function 与 tagging
写入提交，保证创建和软删除只能有一方成功。

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

创建 custom Function 时，服务端必须在同一事务内以
`SELECT id, deleted_at FROM plugins WHERE id = ? FOR SHARE` 锁定目标
Plugin，并持锁至 Function 与 tagging 写入提交。若 Plugin 不存在或已软删除，
返回 `409 + code 4093`；不得创建指向已软删除 Plugin 的 Function。

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

### GET /api/workflows

metadata 列表（不含节点/边）。除公共分页参数外，支持 `id`、`identifier`、
`name`、`search`、`category_id`、`tag_id`、`required_capabilities`、
`timeout_ms_from` / `timeout_ms_to`、`created_at_from` / `created_at_to`、
`updated_at_from` / `updated_at_to` 过滤。列表项包含 Workflow metadata 和
`tags`。

### POST /api/workflows

Body：

```json
{
  "identifier": "ingest",
  "name": "Ingest pipeline",
  "description": "Fetch and normalize input",
  "end_description": "Normalized output is ready",
  "timeout_ms": 33000,
  "category_id": 3,
  "required_capabilities": ["network.http", "db.query"],
  "tag_ids": [2, 7]
}
```

- `identifier`、`name` 必填。
- `description`、`end_description`、`timeout_ms`、`category_id`、
  `required_capabilities` 可选；
  `tag_ids` 可省略，默认空数组。
- `timeout_ms` 必须在 **1000..=330000** 毫秒；省略时服务端使用 **33000**
  毫秒。越界返回 `400 + code 4000`，且不写入 Workflow。
- `required_capabilities` 是 capability name 字符串数组。

成功响应 data 为完整 Workflow metadata；`tags` 只在列表项中返回。

### GET /api/workflows/:id

返回完整 Workflow metadata：
`id`、`identifier`、`name`、`description`、`timeout_ms`、`category_id`、
`input_schema`、`start_description`、`output_schema`、
`end_description`、`required_capabilities`、`created_at`、`updated_at`。

### PUT /api/workflows/:id

Body：

```json
{
  "name": "Ingest pipeline v2",
  "description": "Updated pipeline",
  "end_description": "Updated output is ready",
  "timeout_ms": 45000,
  "category_id": 4,
  "required_capabilities": ["network.http"],
  "tag_ids": [8],
  "updated_at": "2026-07-24T00:00:00Z"
}
```

- `updated_at` 必填，用于乐观锁；冲突返回 `409 + code 4094`。
- 其余字段均可选；`timeout_ms` 仍必须在 **1000..=330000** 毫秒。
- `tag_ids` 省略表示保持现有关联，空数组表示清空全部标签。
- 当前实现对 metadata 的 nullable 字段使用 `COALESCE`：字段省略或传
  `null` 都表示保持原值，不表示清空。

### DELETE /api/workflows/:id

硬删除无 Tool 引用的 Workflow；被 Tool 引用时返回
`409 + code 4093`。

### GET /api/workflows/:id/graph
Returns full DAG:
```json
{
  "workflow": {"id": 5, "identifier": "ingest", "name": "Ingest pipeline", "timeout_ms": 33000},
  "nodes": [
    {
      "id": null,
      "node_key": "start",
      "node_type": "start_node",
      "function_id": null,
      "position": {
        "x": 100,
        "y": 300,
        "input_schema": {"type": "object", "properties": {}},
        "start_description": "Provide the source URL"
      },
      "node_config": null
    },
    {
      "id": 11,
      "node_key": "fetch",
      "node_type": "function_node",
      "function_id": 21,
      "position": {"x": 100, "y": 50},
      "node_config": null
    },
    {
      "id": 12,
      "node_key": "parse",
      "node_type": "function_node",
      "function_id": 22,
      "position": {"x": 300, "y": 50},
      "node_config": null
    },
    {
      "id": null,
      "node_key": "end",
      "node_type": "end_node",
      "function_id": null,
      "position": {
        "x": 100,
        "y": 600,
        "output_schema": {"type": "object", "properties": {"result": {"type": "string"}}},
        "end_description": "Normalized output is ready"
      },
      "node_config": null
    }
  ],
  "edges": [
    {"id": null, "src_node_key": "start", "dst_node_key": "fetch", "mapping": {}},
    {"id": 31, "src_node_key": "fetch", "dst_node_key": "parse",
     "mapping": {"parse.input.raw_text": "fetch.output.body"}},
    {"id": null, "src_node_key": "parse", "dst_node_key": "end", "mapping": {}}
  ]
}
```

服务端始终合成不入库的 `start` / `end` 虚拟节点。`start` 节点的
`position.input_schema` 与 `position.start_description` 分别来自
Workflow metadata 的 `input_schema` 与 `start_description`；`end` 节点的
`position.output_schema` 与 `position.end_description` 分别来自
`output_schema` 与 `end_description`。

### PUT /api/workflows/:id/graph
Full replacement of nodes+edges. Server validates:
- 所有 `function_id` 存在且未删除
- 无环（DFS cycle detection）
- 入参 mapping 引用的 src.output.* 存在于上游 function.output_schema
- `end.position.output_schema.properties` 不得声明内部保留输出键
  `_agent_context_updates`；违反时返回 `5005 / HTTP 422` 和固定安全消息

Body shape mirrors GET response (sans ids — server reassigns).
虚拟 `start` / `end` 节点不写入 `workflow_nodes`；`start` 节点的
`position.input_schema`、`position.start_description` 与 `end` 节点的
`position.output_schema`、`position.end_description` 会写回 Workflow
metadata，随后由响应中的对应虚拟节点原样返回。省略任一字段时保留对应的
现有 metadata 值。

Workflow 节点可用顶层 `_agent_context_updates` 申请受控的运行时
`AgentContext` 更新，但该键不是公共 Workflow 输出。运行时在所有 end-output
合成路径中剥离它，即使数据库中存在旧的错误 schema 也不得泄露。

未声明 `output_schema` 时，单一终端持久化节点的输出直接作为 Workflow
公共输出；存在多个终端节点时，按 `node_key` 字典序返回稳定的
`{ "<node_key>": <output> }` 对象。两种路径都必须从每个终端输出的顶层
剥离 `_agent_context_updates`。

`web-admin` 的 DAG 编辑器必须分别从虚拟 `start` / `end` 节点恢复
`position.start_description` / `position.end_description`（Workflow metadata
仅作兼容回退），在对应节点与节点详情抽屉中展示，并在保存时写回同一字段；
不得另建仅存在于前端的描述字段。服务端合成节点/边可返回 `id: null`，客户端
Graph 类型必须接受可空 ID。DAG 本地缓存从 v2 升级到 v3 时必须保留本地节点、
位置、连线和运行结果，仅补齐缺失的 start/end 描述；只有 Graph 刷新成功且 v3
写入成功后才能删除 v2。Graph 拉取失败时必须保留并继续展示 v2，且不得写 v3。

### POST /api/workflows/:id/execute
Body: `{ "input": { ... } }` — 注入到 DAG 入口 node 的 input
Response data:
```json
{
  "workflow_id": 5,
  "outputs": { "result": "ok" },
  "node_results": { "fetch": {}, "parse": {} },
  "node_inputs": {},
  "node_agent_contexts": {},
  "elapsed_ms": 423,
  "agent_context": null
}
```

`outputs` 始终等于执行器计算出的 Workflow 公共结果并保留其 JSON 形状：显式
scalar schema 仍返回 scalar；无 schema 的单终端直接返回该终端值，多终端返回
稳定的 `node_key -> output` 对象。`node_results` 是逐节点诊断数据，不能代替
`outputs`；其每个节点值顶层也必须剥离 `_agent_context_updates`。管理端 End
节点只展示 `outputs`，不得把 `node_results` 混入公共结果。

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

`model_preset` 为 NULL 时表示该 Agent 使用启动期全局默认 preset。只有 NULL
具有此语义；显式非 NULL 名称在运行期若因配置变更/无效 preset 被跳过而未知，
必须 fail closed 并返回 5007（或内部 typed `ModelPresetUnknown`），不得静默
改用 default。

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
metadata + tool/skill/permission 重置（全替换语义）。修改 permissions 含危险 capability 时要求 Super。PUT 中显式 `model_preset` 与 POST 使用相同的存在性校验；NULL 才表示选择 default。

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
  ],
  "audit": {
    "enqueued": 120,
    "persisted": 119,
    "tracing_only": 7,
    "dropped_queue_full": 0,
    "dropped_writer_closed": 0,
    "dropped_no_writer": 0,
    "persist_failures": 1
  }
}}
```

`cache_misses` 仅在没有可用 `CompiledPlugin` 缓存、缓存失效或被 idle reaper
淘汰后发生真实且成功的 Wasmtime 编译时增长；编译失败或从现有编译缓存创建
fresh runtime state 不增加。`created_total` 仅统计成功创建的 fresh
Store/Instance，失败的编译/实例化尝试不计数。容量只计算内部
`in_use + reserved`；`idle` 是编译缓存数，不占 per-Plugin/global permit。
`reset_failures` 仅为响应兼容保留；fresh Store/Instance 设计不执行 runtime
instance reset，因此该字段预期恒为 0。
`audit` 是进程启动以来的单调计数；任一 `dropped_*` 或
`persist_failures` 增长都必须在 Dashboard 告警。`tracing_only` 包含 Hook
事件，属于预期值而不是丢失。运维通过此端点同时诊断 Plugin Pool 与审计
持久化健康度。

Plugin 调用从 DB lookup 之前即进入 exactly-once 审计生命周期，因此 Plugin
不存在、pool acquire、S3 load、sha256 校验和 WASM 编译等提前失败也会增加
对应的 tracing/入队计数。Agent permissions 查询失败同样写一条安全 capability
事件。上述失败只记录静态错误分类，不记录请求 payload、URL、连接信息、对象
存储/Extism 原始错误或哈希值；Hook 派生上下文仍只增加 `tracing_only`。

Plugin call 每次使用 fresh Store/Instance；调用结束后直接丢弃该 runtime
state，只有 `CompiledPlugin` 可以继续缓存。调用方取消整个 Invoker future 时，
checked-out slot 必须释放，`in_use` 回落；取消 guard 与正常
release/timeout 清理互斥，不得重复递减，且被取消调用的 memory/global/table
不得被后续调用观察。

SIGINT/SIGTERM shutdown 时，listener 先停止 accept；已接受请求最多 drain 30 秒。
随后停止 idle reaper、drop Pool/编译缓存并关闭 DB pool。drain 超时后剩余请求
被取消，服务仍继续清理，不无限维持旧连接。

---

## 9c. Runtime Audit Logs

以下端点仅允许 Super 管理员读取；Normal / System 返回 403 + 2001。
API 是只读的，不提供 POST / PUT / PATCH / DELETE，也不接受任意 audit
payload。Hook 执行保持 tracing-only，因此不会出现在本资源中。

### GET /api/runtime-audit-logs

分页参数：`offset`（默认 0）、`limit`（默认 20，服务端硬上限 100）。
可选精确过滤：`event_type`、`outcome`、`capability`、`request_id`、
`session_id`、`agent_id`；`occurred_at_start` / `occurred_at_end` 接受
RFC3339 或 `YYYY-MM-DD HH:MM:SS`。管理端 RangePicker 必须发送带 `Z`
的 RFC3339 UTC（JavaScript `toISOString()`），避免浏览器本地时区被后端
当成 UTC 而产生偏移；无时区格式仅作为兼容输入保留。

```json
{ "code": 0, "data": {
  "items": [{
    "id": 42,
    "request_id": "request-123",
    "session_id": 10,
    "agent_id": 1,
    "plugin_id": 2,
    "function_id": 3,
    "capability": "network.http",
    "event_type": "capability_call",
    "outcome": "success",
    "elapsed_ms": 18,
    "error_message": null,
    "payload_summary": {"ok": true, "method": "PATCH", "host": "api.example.com"},
    "occurred_at": "2026-07-23T00:00:00Z"
  }],
  "total": 1,
  "offset": 0,
  "limit": 20
}}
```

### GET /api/runtime-audit-logs/:id

返回单条同结构记录；不存在时返回 404 + 4040。

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
| 4093 | 409 | 资源正在被引用，或创建 Function 时目标 Plugin 已软删除（删除/绑定阻塞） | 删除：`{resource_type} 被 {N} 个对象引用，无法删除`；创建：`目标 Plugin 不存在或已删除` |
| 4094 | 409 | OptimisticLockConflict（PUT 请求携带的 updated_at 与 DB 不一致） | `内容已被他人修改，请刷新后重试` |
| 4292 | 429 | Capability rate limited（`network.http` 或 `log.emit` 宿主策略拒绝） | `能力调用过于频繁，请稍后重试` |
| 5000 | 500 | Plugin runtime failure（含 linear-memory 超限；原始 trap 不记录，写安全 tracing + best-effort DB runtime audit） | `Plugin 执行失败或超过内存上限` |
| 5001 | 403 | Cannot delete main agent | `入口 Agent「main」不可删除` |
| 5002 | 422 | Schema mismatch（tool/function schema 不一致） | `Tool 与 Function 的输入/输出结构不匹配，请重新选择或调整` |
| 5003 | 403 | Capability denied during Agent invocation | `当前 Agent 没有执行本次请求所需的能力（{capability}）` |
| 5004 | 408 | Plugin invocation timeout | `Plugin 执行超过 {timeout_ms} 毫秒已被中止` |
| 5005 | 422 | Workflow node input mapping 或 output schema invalid | mapping：`节点「{node_key}」的输入映射「{field}」无效`；保留输出键：`Workflow output_schema 不得声明内部保留字段「_agent_context_updates」` |
| 5006 | 422 | Agent depth exceeded（> 10） | `Agent 层级已达最大深度 10，无法继续添加子 Agent` |
| 5007 | 422 | Unknown model preset | `模型 preset「{preset}」不存在，请重新选择` |
| 5008 | 403 | Builtin skill cannot be deleted | `内置技能「{identifier}」不可删除` |
| 5009 | 503 | Plugin instance pool busy（acquire 等待超时） | `Plugin 实例池繁忙，请稍后重试` |

> `5004` 只表示 timeout；`PLUGIN_CALL_MAX_MEMORY_MB` 超限使用 `5000`。Extism 内存配置单位为 64 KiB page，默认 128 MiB 必须配置为 2048 pages。

> 历史错误码 `4291` 曾用于 admin SSE 并发上限，现已随管理端 Chat
> superseded，不是 004 现役错误契约，且不得与现役 `4292` 混用。

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
