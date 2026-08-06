# Runtime / Performance Quality Checklist: Agent Runtime

> **历史边界（2026-07-23）：** 涉及 admin Chat/SSE、SC-010 或 per-admin SSE 限流的条目已随管理端测试聊天删除而 superseded，仅保留核对历史，不是现役阻塞项。

**Purpose**: 验证 `specs/004-agent-runtime/research.md` + `plan.md` + `spec.md` FR-029..031 中 runtime / Instance Pool / 性能预算需求的**完整性 / 清晰度 / 可测量性**。
**Created**: 2026-05-26
**Audience / Depth**: 作者自查（轻量）
**Scope**: Instance Pool / WASM timeout & memory / FallbackProvider 链 / SC-001..006/010 预算

---

## Instance Pool 设计

- [x] CHK210 spec / research 是否明示 Pool 的**层级结构**（per-Plugin/global permit + FIFO waiter + `CompiledPlugin` 缓存）？[Clarity, research §2]
- [x] CHK211 池上限（每 Plugin 最多 8 / 全局 64）是否在 spec / env var 中暴露？默认值与 SC-005 命中率假设是否对应？[Coverage, research §2 vs SC-005] — ✅ FR-029 列出 `PLUGIN_POOL_MAX_PER_PLUGIN=8` + `PLUGIN_POOL_MAX_TOTAL=64`
- [x] CHK212 空闲 `CompiledPlugin` 缓存的回收策略（idle > 10 min 淘汰）是否在 spec 明示？reaper 在哪里启动和停止？[Gap, research §2 vs tasks] — ✅ FR-029 列出 `PLUGIN_POOL_IDLE_TIMEOUT_SEC=600`；reaper 随服务启动并在 graceful shutdown 停止
- [x] CHK213 Pool 满时的 acquire 行为（block / 超时 / 立即拒绝）是否在 spec 定义？默认超时（如 5s）是否给出？[Gap, FR-029] — ✅ FR-029：FIFO 等 5s（`PLUGIN_POOL_ACQUIRE_TIMEOUT_MS`）超时返 5009 `PoolBusy`
- [x] CHK214 是否明示 runtime state 的释放行为与兼容 `reset_failures` 字段语义？[Edge Case] — ✅ FR-029：每次调用后丢弃 fresh Store/Instance；`reset_failures` 预期恒为 0

## WASM 资源限制

- [x] CHK215 Plugin 单次调用 30s 超时是否明示通过 Wasmtime fuel + Tokio wall-clock timeout **双层**实现（fuel = 防止纯 CPU 死循环；外层 deadline = 限制整体墙钟时间）？[Clarity, research §1 vs FR-030] — ✅ FR-030：`tokio::time::timeout(JoinHandle)` 到期显式调用 `CancelHandle::cancel()`；fuel 或 deadline 任一触发均返回 5004 并丢弃本次 fresh Store/Instance
- [x] CHK216 128 MiB linear memory 上限是否通过 Extism `memory.max_pages=2048`（`memory_mb × 16` 个 64 KiB page）强制，并明确 trap → 5000 + 安全 tracing + bounded best-effort DB runtime audit + 丢弃本次 fresh Store/Instance？[Clarity, FR-032 vs research §1] — ✅ FR-032 固化单位换算与错误语义；真实 129 MiB WAT 集成测试见 Pending T166
- [x] CHK217 调用栈深度上限是否定义？（防止 Plugin 通过递归触发栈溢出）[Coverage, Gap]
- [x] CHK218 host_call payload 大小（4 MB 上下行）+ network.http body 大小（4 MB）是否在 spec 而非仅 contracts 中？[Consistency, host-functions.md §5]
- [x] CHK219 多个 capability 的并发上限（如 `network.http` 8/Plugin）的共享池 vs 独立池语义是否在 spec 明示？[Clarity, host-functions.md §5]

## FallbackProvider 链

- [x] CHK220 spec / research 是否明示 fallback 触发条件（4xx 永久错误 vs 5xx 重试 vs network 错误 vs timeout）？[Clarity, research §7 v4] — ✅ research §7 新增 "Fallback 触发与行为契约" 表：2xx/4xx/401/429/5xx/timeout 各自行为 + 整链总耗时上限
- [x] CHK221 fallback 之间的等待时间 / 退避策略是否定义？还是即刻切换？[Gap]
- [x] CHK222 一次调用中切换 fallback 是否对调用方可见？[Gap, CHK053] — ✅ research §7：现行结果返回 `actual_model`/`fallback_used`/`reason`；原 admin SSE 事件已 superseded
- [x] CHK223 整链失败的"最长等待时间"上限是否定义（防止串行重试堆积到 90s）？[Coverage, Gap] — ✅ research §7：普通链 45s；`llm.invoke` 链 25s；单节点 25s
- [x] CHK224 fallback 切换是否记入 audit log（`event_type=llm_fallback`）？[Coverage, FR-021 v7] — ✅ research §7：每次 provider 切换写 `llm_fallback`；整链失败写 `llm_invoke/error`；本地文本写 `llm_local_fallback`

## 性能预算可测量性

- [x] CHK225 SC-004 (host_call ≤ 5ms) 的"5ms"度量边界（dispatcher 入口 → handler 出口；不含 capability 内的网络/DB 调用）是否明示？[Measurability, SC-004 + T140] — ✅ SC-004 加 "度量边界" 段：dispatcher 入口 → 调用 handler 前；含 JSON 反序列化 + permissions 查询 + 查表
- [x] CHK226 SC-005 (`CompiledPlugin` 缓存命中 ≤ 50ms / 冷启动 ≤ 300ms) 的"命中" / "冷启动" 操作语义是否在 spec / data-model 一致？[Clarity, CHK177] — ✅ 命中 = 已验证编译缓存 + fresh Store/Instance；冷启动 = S3/SHA 校验 + 编译
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
- [x] CHK235 启动失败的回退是否按类型区分？[Clarity] — ✅ LLM registry 缺失/无效在 listener 前输出静态安全错误并非零退出而非 panic；builtin/runtime config 的确有不可恢复路径才保留 panic；无效非 default preset 静态 warn 后跳过
- [x] CHK236 graceful shutdown 是否覆盖 SIGINT/SIGTERM、停止 accept、30 秒 drain、停止 reaper、drop pool/cache 与关闭 DB pool？[Coverage] — ✅ spec/plan 固定顺序，超时继续清理
- [x] CHK237 在线 reload `llm_presets.toml` 的需求是否定义（避免重启服务）？还是 MVP 不支持？[Clarity, Gap]

## 观测性

- [x] CHK238 Pool metrics 是否明确定义容量与计数语义？[Coverage, quickstart §11] — ✅ capacity=`in_use+reserved` 且 idle cache 不占 permit；`cache_misses` 仅成功编译，`created_total` 仅成功 fresh Store/Instance
- [x] CHK239 Plugin 调用的 tracing span 字段（plugin_id / function_id / capability / outcome / elapsed_ms / pool_hit）是否完整定义？[Completeness, FR-021 v7]
- [x] CHK240 LLM 调用的 tracing 字段（model_preset / actual_model / fallback_used / reason / llm_ms）及 provider/local fallback 区分是否在 spec 明示？[Gap, research §7] — ✅ provider 切换=`llm_fallback`；本地文本=`llm_local_fallback`
- [x] CHK242 registry 是否明确启动期构造并缓存完整 chain，且稳定 fallback identity 不依赖可重复的 model 字符串？[Consistency, approved 30A] — ✅ FR-025 / research §7 / T259 Green
- [x] CHK243 preset 默认参数与调用方合法显式 `max_tokens` / `temperature` 的优先级及跨 fallback 保持语义是否明确，并单列 `llm.invoke max_tokens=0` 的 4001 无效参数例外？[Ambiguity, approved 31A] — ✅ FR-025 / host-functions §4.5 / T260 Green
- [x] CHK244 LLM 结果、tracing 与 audit 是否统一返回 `actual_model` / `fallback_used` / `reason`，并把本地文本兜底与 provider fallback 分开？[Observability, approved 32A] — ✅ FR-004 / research §7 / T261 Green
- [x] CHK245 25s/node、45s/ordinary-chain 与 25s/`llm.invoke` chain 的 deadline 层级是否无歧义？[Timeout, approved 33A] — ✅ FR-025 / research §7 / T262 Green
- [x] CHK246 是否区分 registry/config 的 `None` 语义与 `llm.invoke` 的 DB 行语义，并明确 `llm.invoke` 只有在 Agent 行存在且 `model_preset IS NULL` 时使用 default、缺失行安全失败、显式未知 preset fail closed，且后两者都不请求 default provider？[Security, approved 34A] — ✅ FR-025 / host-functions §4.5 / T263 Green

---

## Notes

- runtime 部分的需求**多数是软约束**（性能 + 资源限制），违反不会立即崩；但在 production 压测中暴露的成本极高。
- SC-004 / SC-005 是 hard latency budget，无 LLM 外部依赖，最值得在 implement 前定义清楚。
- 与 003 perf-evidence.md 模式一致，本特性的 perf-evidence 应同样按"宿主延迟 vs 含 LLM 延迟" 分列记录。

---

## Analyze v6 自动勾选说明（2026-05-28）

| CHK | 覆盖依据 |
| --- | --- |
| CHK210 | research §2 明示 per-Plugin/global permit、FIFO waiter 与 `CompiledPlugin` 缓存；每次调用 fresh Store/Instance |
| CHK221 | research §7 fallback 表：429/5xx/timeout 均"立即切下一节点"，无退避等待 |
| CHK228 | plan §偏离 4 明示"宿主侧 ≤ 200ms"作为内部目标，与端到端 ≤ 8s 分列 |
