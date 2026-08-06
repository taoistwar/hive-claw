# Security / Capability Requirements Quality Checklist: Agent Runtime

> **历史边界（2026-07-23）：** 涉及 admin Chat 会话所有权、SSE 错误或聊天 PII/保留期的条目已随管理端测试聊天删除而 superseded；现行用户聊天安全契约属于外部 Assistant API。

**Purpose**: 验证 `specs/004-agent-runtime/spec.md` 中 Security / Capability 相关需求的**完整性 / 清晰度 / 一致性 / 可测量性**。004 的核心创新在 capability-based zero-trust，需求层一个漏洞 = 生产环境的一类漏洞。
**Created**: 2026-05-26
**Feature**: [specs/004-agent-runtime/spec.md](file:///home/developer/agent/hive-claw/specs/004-agent-runtime/spec.md)
**Scope**: spec + contracts/host-functions.md + data-model 的安全相关需求；不评估实现。
**Audience / Depth**: 作者自查（轻量）—— 仅列高影响项；阻塞 `/speckit-implement` 的硬限制以"⚠"标注。

---

## Zero-Trust Capability 模型

- [x] CHK055 spec 是否明确"未声明的 capability = 拒绝"的语义（不是"未声明 = 默认允许某些常用项"）？[Completeness, Spec §FR-003] ⚠ — ✅ FR-003 已声明 "未声明 = 拒绝"
- [x] CHK056 spec 是否定义"Capability 集合是 Agent 字段而非 Plugin 字段"——同一个 Plugin 在不同 Agent 下能力不同？[Clarity, Spec §FR-003]
- [x] CHK057 spec 是否定义子 Agent **不继承父** Agent 的 permissions（最小权限）？[Clarity, Spec §FR-003 v3]
- [x] CHK058 capability dispatcher 是否明确"鉴权在 capability handler 之前执行"，避免 handler 已经访问资源后再拒绝？[Clarity, Spec §FR-003]
- [x] CHK059 spec 是否列出"在任何 host_call 之前可能逃逸 capability 鉴权"的所有路径并各自有 mitigations？[Coverage, Threat model gap]
- [x] CHK060 当 capability 注册表运行时被外部修改（如管理员 hot reload）时的安全语义是否定义？[Edge Case, Gap]

## 危险 Capability 授予

- [x] CHK061 哪些 capability 标记为 `is_dangerous=1` 在 spec / data-model 是否明确列出（不是分散提到）？[Completeness, Spec §Capability]
- [x] CHK062 spec 是否规定危险 capability 的赋予动作（PUT /api/agents）要求 Super；且**调用动作**也要求该 Agent 显式拥有该 capability？[Consistency, Spec §FR-022]
- [x] CHK063 危险 capability 的撤销动作是否要求 Super？（避免 System 创建 Agent → Super 误授 → 后续 System 改 system_prompt 但保留权限的隐性升级）[Coverage, Gap] — ✅ FR-022 补强：赋予 / 撤销 / 调用 三处都明确 Super-only 校验
- [x] CHK064 "审计每次危险 capability 调用"是否在 spec 中明示（而不仅是"audit log 写入"模糊话）？[Completeness, Spec §FR-004]
- [x] CHK065 危险 capability 的列表是否允许后续修改（schema migration）？修改流程的安全审批是否定义？[Coverage, Gap]

## db.execute / db.query — 自由 SQL 防护

- [x] CHK066 spec 是否明示"自由 SQL **永不** 通过 db.execute 接受 Plugin payload"？[Clarity, Spec §FR-002 + Clarify v1.2] ⚠ — ✅ FR-002 + §Clarifications v1.2 已明示
- [x] CHK067 named query 的注册流程（谁能注册、是否需要 PR review）是否在 spec 定义？[Gap]
- [x] CHK068 named query 的入参绑定方式（参数化 placeholder vs 字符串插值）是否在 spec 强制？[Clarity, Spec §contracts/host-functions.md §4.4]
- [x] CHK069 spec 是否定义"named query 自身可能含的危险 SQL（如 DROP TABLE）"的审批流程？[Gap]
- [x] CHK070 db 连接是否分账户（plugin 用只读账户 + DML 专用账户分离）？这一隔离要求是否在 spec 中明示？[Gap]

## secret.get — 密钥访问

- [x] CHK071 哪些 secret key 可被 Plugin 通过 `secret.get` 拿到，是否在 spec 中显式 allowlist？[Completeness, contracts/host-functions.md §4.6]
- [x] CHK072 secret 的 enumerate（"列出所有 secret key"）操作是否被明确禁止？[Coverage, Threat]
- [x] CHK073 secret 的访问审计（哪个 Agent 在哪个 session 读了哪个 key）是否被明确要求？[Completeness, Spec §FR-004]
- [x] CHK074 secret 在内存中的生命周期（不缓存到 Plugin's WASM 内存、不写入 logs）是否在 spec 明确？[Gap, Threat]

## network.http — 出站调用控制

- [x] CHK075 spec 是否定义"白名单域名" 的来源（per-capability 配置 / per-Agent 配置 / 全局）？[Clarity, contracts/host-functions.md §4.1]
- [x] CHK076 SSRF 防护要求（拒绝 127.0.0.1 / 10.0.0.0/8 / 169.254.169.254 等内网）是否在 spec 显式声明？[Coverage, Gap, Threat] — ✅ FR-001 增 "`network.http` SSRF 防护" 段：私网段 + 云元数据端点 + DNS rebinding 防护 + per-Agent allowlist
- [x] CHK077 出站请求是否需要"代理隔离"（所有 http 通过指定代理，便于审计 + 拦截）？是否在 spec 提及？[Gap]
- [x] CHK078 出站 body 大小上限（4 MB）和并发上限（8/Plugin）是否在 spec 而非仅 contracts/host-functions.md 中？[Consistency]

## WASM Sandbox 边界

- [x] CHK079 spec 是否明示"WASM 默认无 WASI 系统调用"——只有显式注册的 host function 可达？[Completeness, plan §research §11]
- [x] CHK080 Plugin 上传时是否对 WASM 二进制做静态检查（拒绝 import 未注册 host function）？是否在 spec 定义？[Gap] — ✅ FR-005 增 "上传期静态校验" 段：magic bytes / 大小 / 未注册 import 拒绝 / 忽略 manifest 自提权字段 / sha256 计算
- [x] CHK081 Plugin manifest 中 `allowed_hosts` / `allowed_paths` 字段是否被宿主 **忽略**（防止 Plugin 通过 manifest 自我提权）？[Gap, Threat]
- [x] CHK082 Plugin 的 linear memory 上限（128 MB）+ 调用栈深度 + fuel timeout 是否在 spec 而非仅 contracts 中？[Consistency, Spec §FR-029..031]

## Instance Pool 状态隔离

- [x] CHK083 spec 是否明确"归还前 reset linear memory"是 hard requirement 而非 best-effort？[Clarity, Spec §FR-029 + Clarify v3.2] ⚠ — ✅ FR-029 已明示 hard requirement
- [x] CHK084 spec 是否定义"Plugin 之间通过 Instance Pool 复用是否可能共享状态"——以及为何在 reset 后**不可能**？[Coverage]
- [x] CHK085 跨 Agent 复用 Plugin 实例时，capability dispatcher 用的是当前调用 Agent 的 permissions 而非首次创建实例时的 Agent —— 是否在 spec / research 明示？[Clarity, Gap]
- [x] CHK086 Plugin 实例创建期间发生 panic 时，宿主是否丢弃实例（不入池）？这一异常处理是否在 spec 定义？[Edge Case, Gap]
- [x] CHK087 高并发场景下 Pool 满 → 等待 → 超时的语义是否在 spec 定义？等待期间 Agent permission 变更是否生效？[Coverage, Gap]

## Audit Trail 完整性

- [x] CHK088 spec 是否定义"capability 鉴权失败也必须 audit"（不仅成功调用）？[Completeness, Spec §FR-004] ⚠ — ✅ FR-004 补强：4030 / 4040 拒绝路径同样必须 audit（`event_type = capability_denied`）
- [x] CHK089 audit 字段 `payload_summary` 的脱敏规则（截断、字段黑名单）是否在 spec 中定义？[Clarity, data-model §V017]
- [x] CHK090 audit 不可篡改的要求是否在 spec 中明示（如只可 INSERT，不可 UPDATE / DELETE 除超期清理）？[Coverage, Gap]
- [x] CHK091 90 天保留期满后，归档 vs 直接清理的策略是否在 spec 定义？[Edge Case, Spec §FR-022]
- [x] CHK092 request_id 在跨 Plugin / Agent / SubAgent 边界传递的完整性要求是否定义？[Completeness, Spec §FR-021]

## Main Agent 与超管控制

- [x] CHK093 spec 是否定义 main Agent 的 system_prompt 修改也走 Super-only？（不仅 delete）[Clarity, Spec §FR-022]
- [x] CHK094 main Agent 的 model_preset 是否允许为空？为空时回退到全局默认 — 全局默认变更是否走 Super 审批？[Gap]
- [x] CHK095 spec 是否禁止"创建另一个 identifier 为 'main' 的 Agent"？[Coverage, Spec §FR-022]
- [x] CHK096 Super 角色被错误降级（如全部 Super 被禁用）时的恢复路径是否在 spec 定义？[Edge Case, 沿用 003 SC-008]

## 子 Agent 路由 & 越权

- [x] CHK097 Agent 嵌套 ≤ 10 是否在 spec 中给出**why 是 10**（防止指数级 LLM 调用？防止思考链过长？）？[Ambiguity, Spec §FR-023]
- [x] CHK098 路由循环（A→B→A）检测的窗口是否在 spec 明确（同一 chat_session 范围 / 全局）？[Clarity, Spec §FR-025 + §Edge Cases]
- [x] CHK099 子 Agent 抛错 / 超时如何向父 Agent 上报？错误是否泄漏内部信息（system_prompt / 内部 tool 名）？[Gap, Threat] — ✅ FR-025 增 "子 Agent 错误的脱敏上报" 段：仅暴露 `{code, message}` envelope，详情写 audit
- [x] CHK100 LLM 输出 `route_to_subagent(agent_id, reason)` 时，spec 是否要求验证 agent_id 必须是直接子 Agent（不能跨级跳）？[Clarity, Gap] — ✅ FR-025 hard-rule 第 ④ 项已加

## 输入校验（Boundary）

- [x] CHK101 spec 是否定义所有 capability 入参在 dispatcher 层做 schema 校验（拒绝非法 payload）？[Completeness, Gap]
- [x] CHK102 Plugin identifier / Function identifier / Skill identifier 的字符集限制（防止 SQL injection / log injection）是否定义？[Gap]
- [x] CHK103 上传的 WASM 文件 magic bytes 校验（0x00 0x61 0x73 0x6d）是否要求？[Gap]
- [x] CHK104 multipart upload 的文件类型 / 大小 / mime 校验顺序是否明确（先拒绝大文件再读内容，避免内存炸）？[Gap]

## 认证 / 授权 — 跨端点一致性

- [x] CHK105 spec 是否定义所有 004 新增端点都走管理中心 JWT（沿用 003）？无遗漏？[Consistency, Spec §Assumptions]
- [x] CHK106 GET /api/capabilities / /api/agents/model-presets 这类只读元数据端点，Normal 是否能访问？[Clarity, contracts/api.md]
- [x] CHK107 SSE chat 端点是否绑定到 admin_id —— 防止用户 A 用 token 拉用户 B 的 session？[Coverage, Gap] — ✅ FR-027 增 "会话所有权与隔离" 段：JWT.admin_id == session.admin_id；Super 例外但仍走显式判定
- [x] CHK108 Plugin 上传到 S3 的 key 是否包含 sha256（防止恶意覆盖同名）？是否在 spec 明确？[Coverage, data-model §plugins]

## 乐观锁 & TOCTOU

- [x] CHK109 updated_at 乐观锁是否覆盖"先读 schema_migrations 后改"的元数据修改路径（防止竞态导致旧 schema 写入）？[Coverage, Gap]
- [x] CHK110 Plugin 软删除 → 重新创建同 identifier+version 的 race condition 是否在 spec 定义？[Edge Case, Gap]
- [x] CHK111 Agent permissions 修改与正在执行的 chat session 之间的可见性（已发出的 host_call 用哪个版本的 permissions）？[Clarity, Spec §FR-003]

## 威胁模型 & 失败响应

- [x] CHK112 spec 是否文档化 Threat Model（恶意 Plugin / 越权管理员 / 凭据泄漏 / DoS / 数据外泄）以及对应的 spec 应对？[Gap, Threat] — ✅ spec.md 新增 §Threat Model：TM-1 恶意 Plugin / TM-2 越权管理员 / TM-3 凭据泄漏 / TM-4 DoS / TM-5 数据外泄 + LLM 提示词注入 + Out of Scope 列表
- [x] CHK113 当 capability handler 内部抛错（如 S3 不可达）时，错误信息是否会泄漏内部 stack trace？是否在 spec 定义过滤？[Coverage, Gap]
- [x] CHK114 Plugin 反复触发 4030 capability denied —— 是否有 fail-fast 熔断 / Plugin 一段时间内禁用？[Edge Case, Gap]
- [x] CHK115 失败的 host_call 是否会触发 SSE error 事件传给 client（潜在信息泄漏）—— 错误 message 是否做脱敏？[Coverage, contracts/api.md §Errors]

## 加密 / 完整性

- [x] CHK116 WASM 文件的 sha256 是否在每次加载前验证（防止 S3 桶被篡改）？[Coverage, Gap, data-model §plugins] — ✅ FR-029 增 "加载前 sha256 校验" hard requirement：宿主每次 GET WASM 后重算 sha256 比对 DB 值，不一致即拒绝实例化 + audit + 通知运维
- [x] CHK117 audit_logs 是否要求"append-only"语义？是否允许加 hash chain 防篡改？[Gap]
- [x] CHK118 JWT_SECRET 与 LLM API KEY 的存储 / 轮换流程是否在 spec / DEPLOYMENT 定义？[Gap, 沿用 003]
- [x] CHK119 数据库连接字符串中的密码是否在日志中遮码（沿用 003）？[Consistency, 003 inheritance]

## 合规 / 数据保护

- [x] CHK120 admin / chat 用户的 PII（手机号、对话内容）保留与删除是否定义？[Coverage, 沿用 003 / 004 FR]
- [x] CHK121 spec 是否要求审计日志可被合规导出（CSV / JSON）+ 时间范围过滤？[Gap]
- [x] CHK122 LLM 调用是否需要客户端 SDK 级的"不发送敏感字段"过滤？是否在 spec 定义？[Gap, Threat]

---

## Notes

- 标 ⚠ 的 4 项（CHK055/CHK066/CHK083/CHK088）是 **HARD REQUIREMENT**，未明确解决前不应进 `/speckit-implement`。
- 大部分 [Gap] 项是"威胁建模过程中容易遗漏的角度"，应在 implement 前至少**显式选择 accept** （记入 Complexity Tracking 或 §Edge Cases），不要默认忽略。
- 与 003-admin-center 已落地的"密码 bcrypt cost=12 / 危险操作 Super-only / 审计日志结构化"约定保持一致是最便宜的做法。
- 真正的安全 review 应在 implement 完成后再走一遍 `/security-review`，本 checklist 只是 spec 层"需求是否充分定义安全"的预筛。

---

## Analyze v6 自动勾选说明（2026-05-28）

| CHK | 覆盖依据 |
| --- | --- |
| CHK056 | FR-003 "按当前正在执行的 Agent 的 permissions 集合鉴权" — permissions 是 Agent 字段而非 Plugin 字段 |
| CHK057 | FR-003 "子 Agent 不继承父 Agent 的 permissions" — 最小权限显式声明 |
| CHK058 | FR-003 "不在集合内的 capability 立刻拒绝" — 鉴权在 handler 之前 |
| CHK064 | FR-004 "每次 Capability 调用都必须写入结构化审计日志" — 含危险 capability 调用 |
| CHK081 | FR-005 v7 "忽略 Plugin manifest 中的 allowed_hosts / allowed_paths 字段" |
