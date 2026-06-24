# API Requirements Quality Checklist: 对外 Assistant API

**Purpose**: Validate API requirements completeness, clarity, and consistency
**Created**: 2026-06-02 | **Verified**: 2026-06-02
**Feature**: [spec.md](../spec.md)
**Focus**: API contract quality — signature auth, rate limiting, error handling, external dependencies

## Requirement Completeness

- [x] CHK001 - Are error response codes and formats specified for every failure path? PASS. All failure paths covered in contracts.md including newly added Content-Type mismatch entry. [Completeness, contracts/api.md §Business/Validation/System Errors]
- [x] CHK002 - Are all validation error messages explicitly enumerated? PASS. user_id≤0, message empty/whitespace, JSON bad, sign mismatch/missing, Content-Type mismatch — all enumerated. [Completeness, contracts/api.md §Validation Errors]
- [x] CHK003 - Is the `sign` parameter format unambiguously specified? PASS. Query param `?sign={md5}` with algorithm and example clearly documented. [Completeness, contracts/api.md §Signature]
- [x] CHK004 - Are requirements defined for `ASSISTANT_SECRET` empty/not configured? PASS. FR-014 now states: "未配置或为空时，跳过签名校验（适用于开发/测试环境）". [Gap, Spec §FR-014]
- [x] CHK005 - Is `user_id` to `cloud_user.ID` mapping explicitly documented? PASS. FR-003 + Assumptions both state 1:1 mapping. [Completeness, Spec §FR-003, Assumptions]
- [x] CHK006 - Are `vip_ask_times`/`normal_ask_times` defaults and fallback documented? PASS. FR-006/FR-007 state defaults (50/5), contracts confirm. [Completeness, Spec §FR-006/FR-007]

## Requirement Clarity

- [x] CHK007 - Is "10 秒内" (SC-001) quantified as timeout or p95? PARTIAL. "在 10 秒内获得" reads as upper-bound timeout, not explicitly as p50/p95/p99. Plan notes LLM deviation. [Clarity, Spec §SC-001]
- [x] CHK008 - Is "1 秒内" (SC-002) specified as p50/p95/worst-case? PARTIAL. Same issue as CHK007 — reads as timeout upper-bound but not statistically defined. [Clarity, Spec §SC-002]
- [x] CHK009 - Is VIP boundary `effective_end_time >= now` explicitly stated? PASS. Clarify Q3 + contracts §VIP Membership Rules both state `>=` with "当天全天有效". [Clarity, Clarify Q3]
- [x] CHK010 - Are Chinese/English error messages both specified? PARTIAL. FR-017 uses "服务繁忙，请稍后重试"; contracts.md uses "Service busy, please retry later". No bilingual policy defined. [Clarity, Spec §FR-017 vs contracts/api.md]
- [x] CHK011 - Is the body serialization format unambiguous for MD5 computation? PASS. "不含空格，保持紧凑格式" explicitly stated in contracts.md. [Clarity, contracts/api.md §Signature]

## Requirement Consistency

- [x] CHK012 - Do error message strings match spec.md vs contracts.md? PARTIAL. FR-017 "服务繁忙" vs contracts "Service busy" — language mismatch but same semantics. [Consistency, Spec §FR-017 vs contracts]
- [x] CHK013 - Is `reply` field optional/nullable consistently specified? PASS. FR-013 + Key Entities + contracts all show `reply?: string`. [Consistency, Spec §FR-013]
- [x] CHK014 - Are field names consistent (snake_case)? PASS. `user_id`, `message` consistently used across spec/contracts/code. [Consistency, Spec §FR-002]
- [x] CHK015 - Is VIP membership ID chain consistent? PASS. `cc_user_membership.id = cloud_user.ID = user_id` confirmed in Key Entities + Assumptions. [Consistency, Spec §Key Entities]

## Acceptance Criteria Quality

- [x] CHK016 - Can SC-003 "100% 准确" be verified given INCR+check non-transactional? PASS. SC-003 now acknowledges: "回滚失败等极端情况最大误差 +1 次/天，属于可接受范围". [Measurability, Spec §SC-003]
- [x] CHK017 - Can SC-005 "对调用方完全透明" be objectively measured? PASS. SC-005 now defines: "响应时间不受影响，无需额外交互，同步成功或失败均不影响该次请求的正常处理". [Measurability, Spec §SC-005]
- [x] CHK018 - Is SC-006 load-testing infrastructure documented? PARTIAL. "100 QPS 下 95% 成功率" specified but no test harness or environment documented. [Measurability, Spec §SC-006]
- [x] CHK019 - Are Agent processing acceptance criteria measurable? PASS. FR-011 now defines: "最多 5 轮迭代内返回 final_content（不含 tool error stop）；超时或多轮耗尽视为失败". [Measurability, Spec §FR-011]

## Scenario Coverage

- [x] CHK020 - Are requirements defined for Agent tool errors mid-loop? PARTIAL. FR-017 covers LLM timeout/failure but not tool-execution errors within AgentRunner iterations. [Coverage, Exception Flow, Spec §FR-017]
- [x] CHK021 - Are requirements defined for global_config table unavailable? PASS. FR-019 added: "读取失败时使用硬编码默认值（50/5）作为兜底". [Coverage, Spec §FR-019]
- [x] CHK022 - Are requirements defined for Main Agent (id=1) not existing? PASS. FR-020 added: "Main Agent 不存在时返回服务不可用错误". [Coverage, Spec §FR-020]
- [x] CHK023 - Are concurrent request race conditions addressed? PARTIAL. Edge Case "同时发送 3 条—限流计数是否准确?" raised but no explicit requirement to handle. [Coverage, Edge Case]
- [x] CHK024 - Is quota consumed/refunded for mid-request failures? PARTIAL. Contracts specify DECR on LLM fail. DECR failure itself and other mid-flow failures (user sync fail) not fully specified. [Coverage, Edge Case]

## Edge Case Coverage

- [x] CHK025 - Is 23:59:59→00:00:00 counter reset specified? PASS. "TTL 为到次日 00:00:00 的秒数" in contracts. [Edge Case, contracts/api.md §Daily Rate Limits]
- [x] CHK026 - Is multi-row cc_user_membership behavior specified? PASS. "取第一条有效记录" (LIMIT 1) in Assumptions. [Edge Case, Spec §Assumptions]
- [x] CHK027 - Is whitespace-only `message` behavior specified? PASS. Contracts updated: "空字符串或仅含空白". Code: `.trim().is_empty()`. [Edge Case, Spec §FR-016, contracts/api.md]
- [x] CHK028 - Is `effective_end_time = NULL` behavior specified? PASS. "无 effective_end_time 但有记录 → 永久有效" in contracts.md. [Edge Case, Clarify Q3, contracts §VIP Membership]

## Non-Functional Requirements

- [x] CHK029 - Are replay attack prevention requirements defined? PASS. Security assumptions documented: "v1 接受重放风险，依赖 HTTPS 传输层安全 + 预共享密钥机密性。后续版本可升级签名算法". [Security Gap, Spec §Assumptions]
- [x] CHK030 - Are request body size limits defined? PASS. Assumptions state "无硬性上限，由 Agent/LLM 上下文窗口自然约束" — explicit deferral to LLM capacity. [Security, Spec §Assumptions]
- [x] CHK031 - Is MD5 algorithm choice documented with collision risk? PASS. Assumptions now state: "MD5 用于来源验证非加密；已知碰撞风险，v1 接受，后续可升级". [Security, Spec §Assumptions]
- [x] CHK032 - Are observability/logging requirements specified? PARTIAL. Plan mentions `tracing::info` but no spec-level logging requirements (fields, levels, PII masking). [Observability Gap]

## Dependencies & Assumptions

- [x] CHK033 - Is "外部 DB 网络可达" assumption validated by failure mode? PASS. FR-012 covers "不可用或未配置时返回明确信息". [Assumption, Spec §FR-012]
- [x] CHK034 - Is "LLM 已正常配置" assumption validated? PARTIAL. Assumption stated but no startup check or health probe requirement. [Assumption, Spec §Assumptions]
- [x] CHK035 - Is "users.id 支持指定值插入" assumption verified? PARTIAL. Assumption documented but no schema validation requirement. [Assumption, Spec §Assumptions]
