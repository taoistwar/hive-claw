# API Contract Quality Checklist: Agent Runtime

**Purpose**: 验证 `specs/004-agent-runtime/contracts/api.md` + `contracts/host-functions.md` 中 HTTP/SSE/host_call ABI 契约的**完整性 / 清晰度 / 一致性 / 可测试性**。
**Created**: 2026-05-26
**Audience / Depth**: 作者自查（轻量）
**Scope**: 10 组 REST + SSE chat + host_call ABI + 14 错误码 + envelope unwrap + 乐观锁

---

## REST 端点完整性

- [ ] CHK183 是否每组资源（Plugin / Function / Workflow / Tool / Skill / Agent / Category / Tag / Chat / Capability）的 CRUD 都列出且响应形态在 contracts 给出示例？[Completeness, contracts/api.md §1..§10]
- [x] CHK184 GET 列表端点是否都接收统一的 `offset` / `limit` 参数？默认值（0 / 20）是否在 contracts 文字明示？[Consistency, Gap] — ✅ contracts §0 公共契约：offset=0 limit=20 max=100 + 超 total 返 200 空数组
- [ ] CHK185 POST 创建端点返回的 `id` 与对象快照（含 server-assigned `created_at` / `updated_at`）是否一致？[Completeness]
- [ ] CHK186 PUT 端点是否在 contracts 明示乐观锁（请求体必含 client 读到的 `updated_at`）的契约？冲突 → 4094？[Clarity, CHK142 v2]
- [ ] CHK187 DELETE 端点是否都明示"被引用阻塞 → 4093"行为？涉及实体：Plugin / Workflow / Function / Tool / Skill / Agent / Tag。[Coverage]
- [ ] CHK188 multipart POST `/api/plugins` 的字段顺序（file vs meta）是否在 contracts 显式说明（顺序很重要：先读 meta 校验大小再读 file）？[Clarity, Gap]

## envelope 与错误一致性

- [ ] CHK189 contracts 是否明示**所有** 2xx 响应都用 `{ code: 0, message, data }` envelope（沿用 003）？[Consistency, contracts/api.md envelope]
- [ ] CHK190 contracts 是否明示**所有**错误响应都用 `{ code, message, data: null }`（data 字段可缺省）？错误码与 HTTP 状态码的映射表是否唯一权威源？[Consistency, contracts/api.md §Errors]
- [x] CHK191 错误响应中的 `data.params`（用于占位符替换）字段结构是否在 contracts 显式定义？示例是否给出？[Clarity, CHK020 v2] — ✅ contracts §0：`data.params` 为对象，可缺省；前端按 code 查模板 + params 替换
- [ ] CHK192 14 个错误码与 003 既有错误码（1001..3004）的命名空间是否无冲突？是否在 contracts 显式列出二者关系？[Consistency, Gap]

## SSE Chat 端点

- [ ] CHK193 contracts 是否完整列出 6 个 SSE 事件（token / tool_call / tool_result / routed / done / error）的 payload schema？[Completeness, contracts/api.md §10 vs FR-028 v7]
- [ ] CHK194 SSE `event:` 与 `data:` 行的格式（每个事件一对 `event:\n data:\n\n` 双换行）是否在 contracts 明示？[Clarity]
- [x] CHK195 SSE keep-alive（comments / pings）的发送频率是否定义（防止 30s 默认 SSE 超时）？[Gap] — ✅ contracts §10 POST messages：每 15s 发 `: ping\n\n` comment
- [x] CHK196 SSE 流的 HTTP headers（`Cache-Control: no-cache`、`Connection: keep-alive`、`X-Accel-Buffering: no`）是否在 contracts 明示？[Coverage, Gap, 防 Nginx 缓冲断流] — ✅ contracts §10 列出全部 4 个 SSE response headers

## host_call ABI

- [ ] CHK197 contracts/host-functions.md 是否对每个 capability 给出"完整的 args schema + data schema + error code"？[Completeness, host-functions.md §4]
- [ ] CHK198 host_call request envelope 的 JSON 编码（UTF-8 + JSON.stringify 严格）是否在 contracts 明示？版本字段是否有？[Clarity, host-functions.md §2]
- [x] CHK199 host_call response envelope 的 `ok: false` 时是否**禁止** 携带 `data` 字段（否则 Plugin 作者可能误读）？[Clarity, Gap] — ✅ contracts §0：ok:false 禁带 data；ok:true 必带 data
- [ ] CHK200 Plugin 的 PDK 实现示例（Rust）是否在 contracts/host-functions.md 给出？[Completeness, host-functions.md §1]

## 鉴权与角色

- [ ] CHK201 Capability Coverage Matrix（contracts/api.md 末尾）是否覆盖所有 10 组端点？[Completeness, contracts/api.md §Capability Coverage Matrix]
- [ ] CHK202 危险 capability 涉及的 Agent permissions 修改路径在 matrix 中是否显式列出 Super-only？[Consistency, CHK063]
- [ ] CHK203 contracts 是否明示"401 / 403 / 422 / 409 等 HTTP 状态码与业务码的映射是单向的"（HTTP 是 transport-level，业务码才权威）？[Clarity]

## 边界值与边缘场景

- [ ] CHK204 Plugin upload 超过 16 MB 时的错误码与文案是否在 contracts 明示？是 4001 还是新错误？[Coverage, CHK002 / CHK104]
- [x] CHK205 GET 列表的 `offset` 超出总数时是返回 200 + 空 items 还是 404？是否在 contracts 明示？[Edge Case, Gap] — ✅ contracts §0：超出 total 返 200 + 空 items（不 404）
- [ ] CHK206 SSE 流的 `done` 事件**必然**会发出（包括成功 / 失败 / 取消）—— 是否在 contracts 显式承诺？[Clarity, FR-028 v7]
- [ ] CHK207 host_call payload 大小 4 MB 上限超出时的错误码是否明示？[Coverage, host-functions.md §5]

## 版本化

- [x] CHK208 是否定义 API 版本化策略（/api/v1 vs query string vs Accept header）？当前是 `/api/...` 无版本前缀，是 by-design 还是待定？[Gap] — ✅ contracts §0：by-design 不引入路径/header 版本；破坏性变更走宪法 + 短期双兼；非破坏性增量原地添加
- [ ] CHK209 host_call ABI 的版本契约（current v1）是否在 host-functions.md §7 明示？[Coverage, host-functions.md §7]

---

## Notes

- 重点关注 **envelope + SSE + host_call ABI 三处**：实现期被反复调用，契约模糊一处就会爆出十处实现分歧。
- 大部分 [Gap] 是"工程默认"（如 SSE no-cache headers），但 spec 没写 = implement 时容易遗漏。
