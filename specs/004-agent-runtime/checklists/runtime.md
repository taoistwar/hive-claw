# Runtime / Performance Quality Checklist: Agent Runtime

**Purpose**: 验证 `specs/004-agent-runtime/research.md` + `plan.md` + `spec.md` FR-029..031 中 runtime / Instance Pool / 性能预算需求的**完整性 / 清晰度 / 可测量性**。
**Created**: 2026-05-26
**Audience / Depth**: 作者自查（轻量）
**Scope**: Instance Pool / WASM timeout & memory / FallbackProvider 链 / SC-001..006/010 预算

---

## Instance Pool 设计

- [x] CHK210 spec / research 是否明示 Pool 的**层级结构**（外层 `HashMap<PluginId, PluginPool>` + 内层 `VecDeque<Instance>`）？[Clarity, research §2]
- [x] CHK211 池上限（每 Plugin 最多 8 / 全局 64）是否在 spec / env var 中暴露？默认值与 SC-005 命中率假设是否对应？[Coverage, research §2 vs SC-005] — ✅ FR-029 列出 `PLUGIN_POOL_MAX_PER_PLUGIN=8` + `PLUGIN_POOL_MAX_TOTAL=64`
- [x] CHK212 空闲实例的回收策略（idle > 10 min 释放）是否在 spec 明示？回收周期任务在哪个 phase 任务实现？[Gap, research §2 vs tasks] — ✅ FR-029 列出 `PLUGIN_POOL_IDLE_TIMEOUT_SEC=600`；启动 init 顺序第 11 步加 pool idle reaper
- [x] CHK213 Pool 满时的 acquire 行为（block / 超时 / 立即拒绝）是否在 spec 定义？默认超时（如 5s）是否给出？[Gap, FR-029] — ✅ FR-029：FIFO 等 5s（`PLUGIN_POOL_ACQUIRE_TIMEOUT_MS`）超时返 5009 `PoolBusy`
- [x] CHK214 实例 `reset()` 失败时（罕见但可能）的处理（丢弃实例 vs 重试）是否在 spec 明示？[Edge Case, Gap] — ✅ FR-029：丢弃 + 计入 reset_failures 指标 + audit + 下次 cache_miss

## WASM 资源限制

- [x] CHK215 Plugin 单次调用 30s 超时是否明示通过 Wasmtime fuel + tokio timeout **双层**实现（fuel = 防止纯 CPU 死循环；tokio = 防止 await 永远不返回）？[Clarity, research §1 vs FR-030] — ✅ FR-030 加 "双层 timeout 实现" 段：fuel + tokio select；任一触发 5004 + 丢弃实例
- [x] CHK216 128 MB linear memory 上限是否在 Plugin manifest 加载时强制（而非运行时 OOM 才发现）？[Clarity, FR-031 vs research §1]
- [x] CHK217 调用栈深度上限是否定义？（防止 Plugin 通过递归触发栈溢出）[Coverage, Gap]
- [x] CHK218 host_call payload 大小（4 MB 上下行）+ network.http body 大小（4 MB）是否在 spec 而非仅 contracts 中？[Consistency, host-functions.md §5]
- [x] CHK219 多个 capability 的并发上限（如 `network.http` 8/Plugin）的共享池 vs 独立池语义是否在 spec 明示？[Clarity, host-functions.md §5]

## FallbackProvider 链

- [x] CHK220 spec / research 是否明示 fallback 触发条件（4xx 永久错误 vs 5xx 重试 vs network 错误 vs timeout）？[Clarity, research §7 v4] — ✅ research §7 新增 "Fallback 触发与行为契约" 表：2xx/4xx/401/429/5xx/timeout 各自行为 + 整链总耗时上限
- [x] CHK221 fallback 之间的等待时间 / 退避策略是否定义？还是即刻切换？[Gap]
- [x] CHK222 一次 chat 调用中切换 fallback 是否对客户端可见（如 SSE 中插入"已切换备用模型"事件）？还是静默？[Gap, CHK053] — ✅ research §7 + FR-028：SSE 加 `fallback_used` 事件（from/to/reason）；可忽略不影响 token 流
- [x] CHK223 整链失败的"最长等待时间"上限是否定义（防止串行重试堆积到 90s）？[Coverage, Gap] — ✅ research §7 fallback 表：`LLM_CHAIN_TIMEOUT_MS=45000` 整链总耗时上限
- [x] CHK224 fallback 切换是否记入 audit log（`event_type=llm_fallback`）？[Coverage, FR-021 v7] — ✅ research §7：每次切换写 `event_type=llm_fallback` audit；整链失败写 `event_type=llm_invoke outcome=error`

## 性能预算可测量性

- [x] CHK225 SC-004 (host_call ≤ 5ms) 的"5ms"度量边界（dispatcher 入口 → handler 出口；不含 capability 内的网络/DB 调用）是否明示？[Measurability, SC-004 + T140] — ✅ SC-004 加 "度量边界" 段：dispatcher 入口 → 调用 handler 前；含 JSON 反序列化 + permissions 查询 + 查表
- [x] CHK226 SC-005 (Pool 命中 ≤ 50ms / 冷启动 ≤ 300ms) 的"命中" / "冷启动" 操作语义是否在 spec / data-model 一致？[Clarity, CHK177] — ✅ SC-005 加 "度量定义" 段：命中 = acquire 从 idle 队列；冷启动 = 编译 + Plugin::new；reset 后归还的实例仍算命中
- [x] CHK227 SC-006 (Agent routing ≤ 1.5s) 含的 1 次 LLM 决策调用是否假定 mock 固定响应时间？真实环境如何衡量？[Measurability, T156 mock 800ms]
- [x] CHK228 SC-010 (端到端 ≤ 8s) 已登记为偏离 4（LLM 外部延迟）；spec 是否给出"宿主侧仅" 的可控延迟预算（如 ≤ 2s）作为内部目标？[Gap]
- [x] CHK229 perf 测量在 hyper / hey / wrk 任一工具下都成立吗？工具选择是否在 tasks 显式（T140..T156）？[Consistency, tasks Polish]

## 并发与背压

- [x] CHK230 SSE 长连接的并发上限（100 个 active session）是否在 spec 明示？超出时新连接行为？[Gap, plan §Scale]
- [x] CHK231 Plugin 调用速率上限（per Agent / per session / 全局）是否定义？[Gap]
- [x] CHK232 rate limit middleware（沿用 003 的 RATE_LIMIT_MAX）是否覆盖新增的 SSE chat 端点？SSE 长连接不应被普通 rate limit 误杀。[Coverage, Gap] — ✅ FR-027 加 "Rate-limit 兼容" 段：SSE 端点绕过 RateLimit；独立 per-admin 并发上限 2（`CHAT_SSE_MAX_CONCURRENT_PER_ADMIN`），超出 4291
- [x] CHK233 DB 连接池（sqlx pool）的 max_connections 在新增大量 audit 写入后是否需要重新调优？是否在 spec 给出建议？[Gap]

## 启动与生命周期

- [x] CHK234 启动期 init 顺序（DB migration → capability 注册表 upsert → builtin function upsert → llm_presets 加载 → ToolRegistry 装配 → router）是否在 spec / plan 显式？[Coverage, plan vs research] — ✅ plan.md 新增 §Startup Initialization Order：12 步严格顺序
- [x] CHK235 启动失败的回退（如 llm_presets.toml 缺失 / 必需 capability 注册失败）是 panic 还是 warn-and-continue？[Clarity, Gap] — ✅ plan.md §Startup 末尾列出 4 类失败的处理（panic / warn / 跳过）
- [x] CHK236 graceful shutdown 行为（SSE 长连接收尾 / Plugin 实例释放）是否在 spec 提及？[Gap]
- [x] CHK237 在线 reload `llm_presets.toml` 的需求是否定义（避免重启服务）？还是 MVP 不支持？[Clarity, Gap]

## 观测性

- [x] CHK238 Pool metrics（`in_use` / `idle` / `created` / `cache_misses` / `wait_count`）是否在 spec 明示作为可暴露的端点（/api/runtime/pool/stats）？[Coverage, quickstart §11] — ✅ contracts/api.md 新增 §9b Runtime Metrics：GET /api/runtime/pool/stats 返回 global + per_plugin 快照
- [x] CHK239 Plugin 调用的 tracing span 字段（plugin_id / function_id / capability / outcome / elapsed_ms / pool_hit）是否完整定义？[Completeness, FR-021 v7]
- [x] CHK240 LLM 调用的 tracing 字段（model_preset / actual_model / fallback_used / llm_ms）是否在 spec 明示？[Gap, research §7]

---

## Notes

- runtime 部分的需求**多数是软约束**（性能 + 资源限制），违反不会立即崩；但在 production 压测中暴露的成本极高。
- SC-004 / SC-005 是 hard latency budget，无 LLM 外部依赖，最值得在 implement 前定义清楚。
- 与 003 perf-evidence.md 模式一致，本特性的 perf-evidence 应同样按"宿主延迟 vs 含 LLM 延迟" 分列记录。

---

## Analyze v6 自动勾选说明（2026-05-28）

| CHK | 覆盖依据 |
| --- | --- |
| CHK210 | research §2 明示"外层 RwLock<HashMap<PluginId, PluginPool>>，子池内 Mutex<VecDeque<Instance>>" |
| CHK221 | research §7 fallback 表：429/5xx/timeout 均"立即切下一节点"，无退避等待 |
| CHK228 | plan §偏离 4 明示"宿主侧 ≤ 200ms"作为内部目标，与端到端 ≤ 8s 分列 |
