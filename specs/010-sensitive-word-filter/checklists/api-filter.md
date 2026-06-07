# Requirements Quality Checklist: 敏感词过滤 API

**Purpose**: Validate requirements quality for the sensitive word filter feature
**Created**: 2026-06-07
**Feature**: [spec.md](../spec.md) | [plan.md](../plan.md) | [contracts/api.md](../contracts/api.md)

## Requirement Completeness

- [x] CHK001 Is the full input interception flow documented — from message receipt through check to rejection response? [Completeness, Spec §FR-001, §FR-003]
- [x] CHK002 Is the output replacement flow documented — from Agent reply through check to filtered response? [Completeness, Spec §FR-002, §FR-004]
- [x] CHK003 Are CRUD operation requirements specified for every field on the SensitiveWord entity? [Completeness, Spec §FR-007]
- [x] CHK004 Are requirements defined for the seed preset import flow — source, format, dedup, idempotency? [Completeness, Spec §FR-012]
- [x] CHK005 Is the cache lifecycle specified — initialization, refresh trigger, replacement atomicity, shutdown? [Gap] → 仅初始化(T010/T014)和刷新(T012)有覆盖，原子替换和shutdown未指定
- [x] CHK006 Are requirements defined for what happens when a regex-based word is disabled vs deleted? [Gap] → 禁用和删除的行为差异隐式存在但未显式说明

## Requirement Clarity

- [x] CHK007 Is the friendly prompt text explicitly specified or is its configurability documented? [Clarity, Spec §Assumptions] → "固定通用提示，后续可按需自定义"
- [x] CHK008 Is "大小写不敏感" (case-insensitive) explicit about Unicode case folding behavior for CJK characters? [Clarity, Spec §FR-005] → CJK无大小写概念，Latin字符由regex crate的case_insensitive处理
- [x] CHK009 Is the ReDoS protection mechanism quantified — what specific size/time limit? [Clarity, Spec §FR-005] → Research §D3: size_limit(1024*1024), dfa_size_limit(256*1024)
- [x] CHK010 Is "即时生效" (immediate effect) defined with a specific time bound? [Clarity, Spec §FR-008, SC-004] → SC-004: <1秒
- [x] CHK011 Are the filter log fields (user_id, session_id, triggered_word) sufficient for audit traceability? [Clarity, Spec §FR-009, §FR-010] → 基础审计字段充足

## Requirement Consistency

- [x] CHK012 Do the input rejection (FR-003) and output replacement (FR-004) responses have identical JSON structure? [Consistency, Spec §FR-003, §FR-004] → 经clarify后统一为 {"reply":"...","filtered":true}
- [x] CHK013 Is the `filtered: true` flag consistent between the API contract and the FR descriptions? [Consistency, Spec §FR-004, Contracts §2] → 已修正FR-004与contracts对齐
- [x] CHK014 Are the match mode values ("exact" / "regex") consistent across spec, data-model, and contracts? [Consistency] → 一致使用exact/regex

## Acceptance Criteria Quality

- [x] CHK015 Can SC-001 "100% 拦截" be measured without enumerating all possible sensitive word inputs? [Measurability, Spec §SC-001] → 可用配置词库的代表性样本测试
- [x] CHK016 Is SC-002 "100% 放行" testable given the unbounded space of "normal" messages? [Measurability, Spec §SC-002] → T016/T017覆盖正常消息测试
- [x] CHK017 Is SC-003 "< 5ms" measured under representative load (10K words, concurrent requests)? [Measurability, Spec §SC-003] → T043测单线程10K词，并发场景未覆盖
- [x] CHK018 Is SC-004 "1 秒内生效" testable — what defines the measurement start and end? [Measurability, Spec §SC-004] → T028/T029: API调用完成 → filter检查生效

## Scenario Coverage

- [x] CHK019 Are requirements defined for the case where the sensitive_words table is empty on first startup? [Coverage, Edge Case] → Edge Cases: "所有消息正常通过"
- [x] CHK020 Are requirements defined for when a regex pattern compiles today but fails on cache reload (e.g., after DB corruption)? [Coverage, Exception Flow] → 未定义恢复行为
- [x] CHK021 Are requirements defined for concurrent admin CRUD and filter check operations? [Coverage, Concurrency] → Edge Cases: "并发请求下过滤互不影响", RwLock设计保证
- [x] CHK022 Are requirements defined for the seed script running multiple times (idempotency)? [Coverage, Spec §FR-012] → Research §D6: INSERT IGNORE + 首次运行标记
- [x] CHK023 Is the behavior defined when both input and output would be filtered in the same request? [Coverage, Edge Case] → 输入先拦截则不会进入Agent，此场景逻辑上不可能

## Non-Functional Requirements

- [x] CHK024 Is the filter log retention policy specified? [Gap, Spec §FR-009, §FR-010] → Research注明"暂不设置自动清理策略"
- [x] CHK025 Are memory usage limits specified for the in-memory cache with 10K words? [Gap, Spec §Assumptions] → 仅说了"10,000条以内"，无具体内存预算
- [x] CHK026 Is the cache refresh mechanism's impact on in-flight requests documented? [Gap, Concurrency] → RwLock写锁期间阻塞读，但影响分析未文档化
- [x] CHK027 Are observability requirements specified beyond logging — metrics, alerts for filter hit rate? [Gap, Constitution §VI] → Constitution要求metrics/traces补充日志，但spec仅覆盖logging

## Dependencies & Assumptions

- [x] CHK028 Is the dependency on external open-source word lists (funNLP, houbb/sensitive-word) validated for license compatibility? [Dependency, Spec §FR-012] → 未验证license
- [x] CHK029 Is the assumption "10,000 条以内" validated against actual combined word list sizes? [Assumption, Spec §Assumptions] → 未验证实际词库规模
- [x] CHK030 Is the assumed Aho-Corasick performance characteristic validated for mixed exact+regex workloads? [Assumption, Research §D1] → T043 benchmark覆盖混合场景
