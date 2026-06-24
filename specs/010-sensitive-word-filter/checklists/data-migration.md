# Requirements Quality Checklist: Data & Migration

**Purpose**: Validate requirements quality for DB schema, seed data, and migration strategy
**Created**: 2026-06-07
**Feature**: [spec.md](../spec.md) | [data-model.md](../data-model.md) | [plan.md](../plan.md)

## Requirement Completeness

- [x] CHK001 Are the `sensitive_words` table columns fully specified — types, constraints, defaults, charset? [Completeness, Data-Model §SensitiveWord] → 完整DDL含ENGINE/CHARSET
- [x] CHK002 Are the `sensitive_filter_logs` table columns and indexes fully specified? [Completeness, Data-Model §FilterLog] → 含idx_event_type, idx_created_at
- [x] CHK003 Is the seed import format specified — JSON structure, field mapping, encoding? [Completeness, Spec §FR-012] → 仅提及"seed JSON文件"，未定义具体schema
- [x] CHK004 Are requirements defined for seed data idempotency on repeated runs? [Completeness, Spec §FR-012] → Research §D6: INSERT IGNORE + 首次运行标记
- [ ] CHK005 Is the migration rollback strategy documented? [Gap] → 未定义回滚策略
- [x] CHK006 Are requirements defined for the `match_mode` column's allowed values and migration validation? [Completeness, Data-Model §SensitiveWord] → VARCHAR(16), 校验exact/regex

## Requirement Clarity

- [x] CHK007 Is `VARCHAR(512)` justified for the `word` column — what's the longest expected regex? [Clarity, Data-Model §SensitiveWord] → 512字符对敏感词/正则合理
- [x] CHK008 Is the `UNIQUE KEY uk_word_mode (word, match_mode)` constraint's behavior on conflict clearly defined? [Clarity, Data-Model §SensitiveWord] → Contracts: 重复返回409
- [x] CHK009 Is "ON UPDATE CURRENT_TIMESTAMP" the correct behavior for `updated_at` — should manual timestamp setting be allowed? [Clarity, Data-Model §SensitiveWord] → MySQL标准模式，adequate
- [x] CHK010 Is the seed data source explicitly referenced — which specific files/versions from funNLP and houbb/sensitive-word? [Clarity, Spec §FR-012] → 仅提及仓库名，无具体文件/版本/commit

## Requirement Consistency

- [x] CHK011 Do the `sensitive_words` columns align between data-model.md and the migration SQL? [Consistency] → 一致
- [x] CHK012 Is the `enabled` field's type (TINYINT vs BOOLEAN) consistent with existing project conventions? [Consistency] → TINYINT(1)为MySQL布尔惯例，与项目一致
- [x] CHK013 Does the seed data match mode (`exact`) align with the spec's default for newly created words? [Consistency, Spec §FR-005] → DEFAULT 'exact'

## Scenario Coverage

- [x] CHK014 Are requirements defined for migrating existing data when adding new columns to `sensitive_words`? [Coverage, Gap] → v1首次部署，无需迁移旧数据
- [x] CHK015 Is the seed script's behavior defined when the table already contains conflicting entries? [Coverage, Spec §FR-012] → INSERT IGNORE
- [x] CHK016 Are requirements defined for purging old `sensitive_filter_logs` records? [Coverage, Gap] → "暂不设置自动清理策略"，为已知缺口

## Dependencies & Assumptions

- [x] CHK017 Is the assumption that seed data uses charset `utf8mb4` documented? [Assumption] → DDL: DEFAULT CHARSET=utf8mb4
- [x] CHK018 Is the dependency on MySQL 8.0+ for `ON UPDATE CURRENT_TIMESTAMP` validated? [Dependency, Data-Model §SensitiveWord] → 项目已使用MySQL 8.0+
