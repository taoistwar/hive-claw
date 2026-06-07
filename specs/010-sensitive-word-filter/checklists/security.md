# Requirements Quality Checklist: Security

**Purpose**: Validate security requirements quality for sensitive word filtering
**Created**: 2026-06-07
**Feature**: [spec.md](../spec.md) | [plan.md](../plan.md) | [research.md](../research.md)

## Requirement Completeness

- [x] CHK001 Are ReDoS protection requirements specified with concrete limits (DFA size, time bound)? [Completeness, Spec §FR-005] → Research §D3: size_limit(1024*1024), dfa_size_limit(256*1024)
- [x] CHK002 Is the admin API authentication mechanism explicitly referenced — which existing middleware? [Completeness, Contracts §3] → admin_auth_middleware
- [x] CHK003 Are requirements defined for protecting the seed script execution — who can run it, where? [Gap, Spec §FR-012] → seed脚本为bin，无访问控制定义
- [x] CHK004 Are data protection requirements specified for `sensitive_filter_logs` — does `triggered_word` contain sensitive content that needs masking? [Gap, Spec §FR-009] → 日志中triggered_word即为敏感词本身，其可见性未定义

## Requirement Clarity

- [x] CHK005 Is "ReDoS 攻击防范" expressed as a falsifiable requirement — can we prove it's mitigated? [Clarity, Spec §FR-005] → 保存时校验拒绝 + DFA size limit双保险，可证伪
- [x] CHK006 Is the regex `size_limit` value explicitly specified or derived from a documented rationale? [Clarity, Research §D3] → size_limit(1024*1024), dfa_size_limit(256*1024)
- [x] CHK007 Are the `sensitive_filter_logs` retention and access control requirements defined? [Clarity, Spec §FR-009] → 未定义保留策略和访问控制

## Requirement Consistency

- [x] CHK008 Are authentication requirements consistent between the CRUD API (admin auth) and the assistant API (MD5 sign)? [Consistency, Contracts §1, §3] → 不同接口不同认证方式，设计合理
- [x] CHK009 Is the ReDoS defense strategy consistent between spec (backtrack limit + timeout) and research (DFA size_limit only)? [Consistency, Spec §FR-005, Research §D3] → spec提到"限制回溯深度"，但regex crate不使用回溯；Research的DFA size_limit是正确的实现方案

## Scenario Coverage

- [x] CHK010 Are requirements defined for a malicious admin injecting a catastrophic backtracking regex? [Coverage, Spec §FR-005] → 保存时校验正则合法性
- [x] CHK011 Is the behavior defined when the filter cache is poisoned by a compromised DB entry? [Coverage, Exception Flow] → DB被篡改后缓存加载失败的恢复行为未定义
- [x] CHK012 Are requirements defined for rate limiting on the CRUD API to prevent abuse? [Gap] → CRUD API无限流定义

## Dependencies & Assumptions

- [x] CHK013 Is the assumption that Rust `regex` crate is ReDoS-safe by construction validated against known edge cases? [Assumption, Research §D3] → "Rust regex crate自身是ReDoS-safe（不使用回溯）"
- [x] CHK014 Is the dependency on admin auth middleware documented — what happens if middleware is misconfigured? [Dependency, Contracts §3] → 复用现有admin_auth_middleware，非本特征特有
