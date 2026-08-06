# Research: Agent Runtime — 关键技术决策

> **范围更新（2026-07-23）：** 第 8、10 节记录的管理端测试聊天 SSE/API/admin 表方案已由 `81a84fe` 移除并 superseded；不得恢复 `/api/admin-chat*` 或 `/api/chat/sessions*`。现行普通用户聊天由 `web-user` 调用 `/api/assistant`、`/api/newsession`、`/api/messages`，使用 `chat_sessions_user` / `chat_messages_user`，详细契约属于 `007-external-assistant-api` 与当前实现。

**Created**: 2026-05-26
**Status**: Phase 0 output

---

## 1. WASM Runtime — Extism Rust SDK

**Decision**: 使用 `extism = "1.x"` 作为 Rust 宿主侧 SDK。

**Rationale**:
- 已是事实标准的 Plugin SDK：跨 9+ 语言（Rust/Go/JS/Python/...）的 PDK，Plugin 作者迁移成本最低
- 内部默认用 [Wasmtime](https://wasmtime.dev/)，工业级 sandbox（BytecodeAlliance 维护）
- 显式的 Host Function 机制天然契合 Capability 设计；不需要自造 ABI
- 支持显式 memory limit / timeout 配置（满足 FR-030/031）

**Alternatives considered**:
- **裸 Wasmtime**：灵活度更高，但需要自造 PDK + ABI；Plugin 作者负担过重
- **Wasmer**：成熟度差不多，但 Extism 的 host function 模型更清爽
- **Wasm Components Model + wit**：未来方向更优雅，但目前 wit-bindgen 跨语言体验不及 Extism PDK
- **WAMR**：嵌入式 first，桌面/服务端用例资源比 Wasmtime 弱

**Implementation notes**:
- 使用 `extism::Plugin::new` + `manifest` 控制 timeout / memory / allowed_hosts
- Host Function 通过 `Function::new(name, params, results, ...)` 注册，统一签名为 `(i64, i64) -> i64`（pointer/length 编码字节切片）
- Plugin 的 `manifest.toml` 不在仓内强约束；hive-claw 仅认 Plugin 自身的 metadata（spec FR-005）

---

## 2. Instance Pool 策略

**Decision**: 自研 LRU + 每 Plugin 独立子池；外层 `RwLock<HashMap<PluginId, PluginPool>>`，子池内 `Mutex<VecDeque<Instance>>` + 信号量限流。

**Rationale**:
- bb8 / deadpool 是通用连接池，对 Extism Plugin 的"每个 Plugin 二进制不同 / 编译成本极高"特征不匹配（无法跨 Plugin 复用）
- 每 Plugin 独立池：Plugin A 的实例不会因 Plugin B 流量被驱逐
- 池上限可配（默认每 Plugin 最多 8 实例，全局总数 64）
- 空闲超过 10 分钟（可调）的实例回收

**Alternatives considered**:
- **bb8 + Plugin-id-aware key**：bb8 假设 connection homogeneous，需要包装绕过
- **每次新建**：spec SC-005 要求命中池 p95 ≤ 50ms，每次重新编译 1MB WASM 需 100-500ms，不可接受

**Implementation notes**:
- `PluginPool::acquire(plugin_id)` async 返回 `PluginGuard`，drop 时归还
- 归还前调用 `instance.reset()`（Extism 实例无状态，可重用，但要清掉 instance memory cache 若 plugin 有可变全局）
- 池命中 / miss / wait 各打 metrics

---

## 3. Capability ABI — 单入口 `host_call(name, payload)`

**Decision**: 单一 host function `host_call(capability_name: string, payload: bytes) -> bytes`，payload 用 **JSON**（draft）+ 后续可平滑迁移到 MessagePack。

**Rationale**:
- 单入口：增删 Capability 不改 ABI，Plugin 不用重编译
- JSON：跨语言 PDK 都支持；调试期可读；性能差距在 hot path 才显著（spec SC-004 ≤ 5ms 含序列化测过 1KB payload 是 ~30μs，可接受）
- 鉴权一处实现：spec FR-003 的"未声明 = 拒绝"在 dispatcher 入口完成

**Alternatives considered**:
- **每个 Capability 一个 host function**：ABI 膨胀；新增 capability 需 PDK 跟进；难以做统一鉴权拦截
- **MessagePack from day one**：性能优势不抵 debugging 困难
- **Protobuf**：跨语言强 schema，但要求 Plugin 作者维护 .proto，与"低门槛 PDK"理念冲突

**Implementation notes**:
- Payload 顶层 envelope：
  ```json
  { "capability": "network.http", "args": { ... } }
  ```
  Capability dispatcher 按 `capability` 字段 strip 后转发到具体 handler
- 错误返回也走单一 envelope：
  ```json
  { "ok": false, "code": 4030, "message": "Capability denied: network.http" }
  ```

---

## 4. DAG 编辑器 — reactflow

**Decision**: `reactflow ^11.x`（fka @xyflow/react）。

**Rationale**:
- 业内事实标准；React 集成成熟
- 自带 node/edge interaction（拖拽、连线、缩放、minimap）
- TypeScript 类型完整
- 支持自定义 node renderer（Function selector + 输入端口）

**Alternatives considered**:
- **vis-network**：jQuery 时代风格，难与 React 配合
- **dagre-d3**：仅布局，无交互
- **antv/x6**：功能丰富但中文 doc 多于 English；社区相对小
- **手写 SVG**：成本无穷大

---

## 5. Workflow 执行器

**Decision**: 拓扑 BFS + tokio 并行 + 错误优先短路；节点级超时（继承 Workflow 全局 + 节点 override）。

**Algorithm**:
```
queue := nodes with in_degree == 0
results := {}
while queue not empty:
    parallel for each node in queue:
        inputs := resolve(node.input_mappings, results)
        outputs := invoke(node.function, inputs)
        results[node.id] := outputs
    queue := next layer (children whose all parents done)
return results[terminal_node]
```

**Rationale**:
- 与 spec FR-017 完全对齐：拓扑顺序 + 并行 + 任一失败终止
- tokio::join_all + try_join_all 即可表达
- 不引入第三方 workflow engine（temporal / argo），Principle V

**Alternatives considered**:
- **temporal.io**：分布式工作流引擎，过重
- **petgraph + 手写调度**：可行，但 reactflow 出的 DAG JSON 直接拓扑跑更直接

---

## 6. Agent 路由决策

**Decision**: **LLM tool-calling 自决**为主，结合"hard rule"安全门：

- 系统组装：当前 Agent 的 system_prompt + 子 Agent 列表（id/name/description） + tools/skills 列表
- LLM 输出 tool_call：要么调 `route_to_subagent(agent_id, reason)`，要么调 Tool/Skill，要么直接回复
- "hard rule"安全门：
  - 当前 Agent 没有 `llm.invoke` capability → 不走 LLM，直接回复 "本节点无 LLM 能力，请检查配置"
  - 嵌套深度 ≥ 10 → 拒绝 route_to_subagent
  - 同一对话路径循环 → 拒绝（spec edge case）

**Rationale**:
- 纯规则匹配僵化（关键词触发难维护）；纯 LLM 自决放任风险
- 当前业界事实标准（OpenAI Assistants / LangGraph / Anthropic Agentic）都是 LLM 自决 + 工具描述驱动

**Alternatives considered**:
- **纯规则路由**（关键词 / regex）：维护成本高，效果差
- **embedding 检索 + 最相似子 Agent**：可作为优化路径，本版不上

---

## 7. LLM Client

**Decision**: **复用 workspace 现有 `crates/providers`**。

**Rationale**:
- workspace 已落地完整的多 backend LLM 抽象：`LLMProvider` trait 暴露 chat/tool-calling；factory 用 `make_provider(&cfg) -> Arc<dyn LLMProvider>` 按配置选 backend；含 Fallback、ProviderRegistry、`ToolCallRequest` / `LLMResponse` 完整类型。
- 已实现的 backend 覆盖：Anthropic / Azure OpenAI / Bedrock / OpenAI Compat（含本地 vLLM/Ollama/智谱/通义 / OpenAI 官方）/ OpenAI Codex / GitHub Copilot；含 image / transcription / OAuth token 管理。
- 引入 async-openai 是重复造轮子；与 Principle V（YAGNI）冲突。

**术语澄清**：
`crates/providers::FallbackPreset` 是单个模型配置（model + tokens + temperature + reasoning_effort）；不是"命名链"。004 新增的"命名 preset" 概念是 hiveweb 这一层的**命名 LLM 配置**，每个命名 preset 内部组装出一个 `FallbackProvider`（primary + N 个 `FallbackPreset` 作为 fallback 候选）。为避免词义碰撞，本文档与代码中将其称为 **`LlmPresetName`**（hiveweb 层概念）vs **`providers::FallbackPreset`**（providers 层既有概念）。`agents.model_preset` 字段存储的是 hiveweb `LlmPresetName`。

**Integration**:
- Agent Runtime 新增 `crates/hiveweb-admin/src/runtime/llm.rs` + 配置文件（默认 `crates/hiveweb/llm_presets.toml`）：
  - 启动期解析 TOML → 一组命名 `LlmPresetName → (primary cfg, fallback chain: Vec<FallbackPreset>)` → 用 `providers::make_provider` 构造 primary `Arc<dyn LLMProvider>`，再构造 `FallbackProvider::new(primary, fallback_chain, factory)`，结果挂在 `HashMap<LlmPresetName, Arc<dyn LLMProvider>>` 中
  - 配置中标记一个 preset 为 `default = true`（启动校验：必须正好 1 个）
  - 提供 `runtime/llm::provider_for(agent: &Agent) -> Arc<dyn LLMProvider>`：
    - `agent.model_preset.is_some()` → 查 map；命中 = 返回该 preset 的 provider；未命中 = warn 日志 + fallback 到默认 preset
    - `agent.model_preset.is_none()` → 默认 preset
  - 把 Agent 的 messages + tools 转成 `providers::base` 的请求类型，调用，再把 `LLMResponse`（含 `ToolCallRequest`）转回 Agent 编排层
  - `llm.invoke` capability 默认沿用调用方 Agent 的 preset（Plugin 不能跨 Agent 借用更贵的模型）；Plugin 显式在 args 中传 `model_preset` 字段时，runtime 校验该 preset 是否被当前 Agent 允许（preset allow-list 是可选的，MVP 不上）
  - Agent CRUD 在保存阶段校验 `model_preset ∈ map.keys()`，未命中 → 5007 `ModelPresetUnknown`

**Alternatives considered**:
- **async-openai 直连**：放弃 — 重复造轮子
- **直接在 runtime 里 reqwest 裸调**：放弃 — 失去 fallback / multi-backend / 已有的 tool-calling 规范化
- **包一层 RuntimeLLM trait 反向抽象**：放弃 — 多一层抽象违反 Principle V，`LLMProvider` 已经足够

**Fallback 触发与行为契约**（CHK220 / CHK222 / CHK223 / CHK224）：

| 主调结果 | Fallback 触发 | 说明 |
| --- | --- | --- |
| 2xx 含合法响应 | ❌ | 返回上游 |
| 4xx（除 429）= client error | ❌ | Plugin / 入参问题，retry 无益；直接传错给 caller |
| 401 / 403 | ❌ | 凭据问题，不切换；告警 |
| 429 Rate limit | ✅ | 立即切下一节点 |
| 5xx / network / TLS error | ✅ | 立即切下一节点 |
| 单节点墙钟超时（默认 `LLM_NODE_TIMEOUT_MS = 25000` ≤ FR-030 30s） | ✅ | 立即切下一节点 |
| **整条链总耗时上限** `LLM_CHAIN_TIMEOUT_MS = 45000` | — | 触发即向当前调用方返回错误并停止 fallback（防止 8 个节点 × 25s = 200s 失控）|

**调用方可见性**：FallbackProvider 仍记录切换审计，但原管理端 chat SSE 的 `fallback_used` 客户端事件已 superseded。现行调用方是否暴露 fallback 信息由自身 API 契约决定。

**审计**：每次 fallback 切换写 `runtime_audit_logs` 一行（`event_type=llm_fallback`、`outcome=success`、payload_summary 含 from/to/reason）；整链失败写一行 `event_type=llm_invoke`、`outcome=error`。

**Timeout 优先级**（CHK241）：
- `LLM_NODE_TIMEOUT_MS = 25s`（单节点上限）⊂ `PLUGIN_CALL_TIMEOUT_MS = 30s`（Plugin 外层硬超时）
- `LLM_CHAIN_TIMEOUT_MS = 45s`（整条 fallback 链总上限）**超过** 30s Plugin timeout —— 因此 `llm.invoke` handler 内部必须在 `min(LLM_CHAIN_TIMEOUT_MS, PLUGIN_CALL_TIMEOUT_MS - buffer)` 时主动返回错误，避免被外层 30s 墙钟中断。推荐 handler 内部链上限设为 **25s**（与单节点一致），留 5s buffer 给 Plugin 外层归还实例 / 写审计。

---

## 8. 流式回复（Legacy / Superseded）

原决议选择 axum SSE，为管理端测试聊天提供增量事件。该入口、UI 与协议已移除，不再是 004 的现行传输决策，也不得据此恢复 `/api/chat/sessions*` 或 `/api/admin-chat*`。

现行 `/api/assistant` 对 `web-user` 返回完整 JSON 消息；orchestrator 内部即使使用事件流组织执行，也不构成客户端可见的 SSE 契约。

---

## 9. JSON Schema 校验

**Decision**: `jsonschema = "0.17"` (draft 2020-12)。

**Rationale**:
- 业界主流 Rust 实现
- 性能：编译一次 schema，校验多次
- Function 注册时即编译 schema 并缓存于 service 层

---

## 10. 聊天会话持久化（Legacy 决议与现行边界）

原 `chat_sessions` + `chat_messages` admin 表设计已 superseded，表也已删除。现行普通用户聊天使用 `chat_sessions_user` + `chat_messages_user`，以 `user_id` 绑定所有权；该模型的演进、保留期和 API 查询语义由外部 Assistant API 特性负责，不由 004 重新定义。

---

## 11. 安全相关研究

**WASM Sandbox**：
- Extism / Wasmtime 默认不暴露 wasi-* preview 1/2 系统调用（无文件、无网络）
- 仅通过显式注册的 host function 访问宿主资源 → 与 Capability 模型天然契合
- Resource limits：linear memory 上限 + 调用栈深度 + fuel-based 中断（按指令计数）

**Capability 设计原则**:
- Zero-trust default deny：Agent 不显式列出某 capability，则该 capability 不可调
- 颗粒度按"资源类型 + 动作"：`network.http` / `s3.read` / `s3.write` / `db.query` 等。资源粒度（如允许哪些 URL 域）由 capability handler 内部进一步约束（白名单/正则），不在 Agent permission 字符串里编码
- 危险 capability 标记：`db.execute`（任意 SQL）、`secret.get`（明文密钥访问）必须由 Super admin 才能赋予 Agent

---

## 12. 测试策略（Principle II 前置考虑）

- **Contract tests**：现行 004 HTTP 端点（plugin/function/workflow/tool/skill/agent）形状、错误码、鉴权；原 admin chat 合约测试已 superseded
- **Host ABI contract**：用 Rust PDK 写极小 plugin（仓内 examples/）做集成测试夹具
- **Capability denial**：Agent without capability X → 调 X → 必返特定错误；100% 覆盖每个 capability
- **Workflow topology**：dijkstra-like 校验（含 cycle detection）
- **Agent routing**：构造 mock LLM 返回特定 tool_call，验证 dispatch 正确
- **Instance Pool**：并发 16 个 Plugin 调用，命中率 / 等待时间

---

## 13. 资源约束 & 偏离记录

- LLM 端到端延迟 → plan §Complexity Tracking 偏离 4
- 引入 Extism → plan §Complexity Tracking 偏离 5（需在下次宪法修订加入 canonical 栈）
