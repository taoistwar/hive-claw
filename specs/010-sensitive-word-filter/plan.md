# Implementation Plan: 敏感词过滤

**Branch**: `260516-sensitive-word-filter` | **Date**: 2026-06-07 | **Spec**: [spec.md](./spec.md)

## Summary

为 `/api/assistant` 添加敏感词双向过滤。词库支持精确/正则匹配，变更即时生效。首次部署预置开源敏感词库（funNLP + houbb/sensitive-word 合并去重），管理员可自由管理。

## Technical Context

**Language/Version**: Rust 1.85+, TypeScript 5.x
**Primary Dependencies**: axum, sqlx (MySQL), regex, aho-corasick, serde_json
**Storage**: MySQL (sensitive_words表), 内存缓存 Arc<RwLock<Vec<SensitivePattern>>>, seed JSON文件
**Testing**: cargo test
**Target Platform**: Linux server (backend), Web browser (frontend)
**Project Type**: web-service
**Performance Goals**: < 5ms overhead, 10K条 < 1ms全量匹配
**Constraints**: p95 < 200ms, 不引入新deployable unit
**Scale/Scope**: ≤ 10,000条, 后端+前端各新增1模块, seed脚本预置词库

## Constitution Check

✅ I — 独立模块, api→services→models
✅ II — TDD, tests before impl
✅ III — HTTP 200 + filtered:true 兼容格式
✅ IV — Aho-Corasick + 预编译Regex, benchmark included
✅ V — 无新crate, 无新deployable
✅ VI — 结构化日志, fire-and-forget filter_logs

**Gate**: PASS

## Project Structure

```text
crates/hiveweb/src/
├── api/sensitive_word.rs        # NEW: CRUD API
├── services/sensitive_filter.rs # NEW: engine + cache + DB
├── models/sensitive_word.rs     # NEW: structs
├── runtime/orchestrator.rs      # MODIFY: output filter
├── api/chat_assistant.rs        # MODIFY: input filter
├── api/mod.rs                   # MODIFY: AppState + SensitiveFilter
└── bin/seed_sensitive_words.rs  # NEW: seed预置词库

web-admin/src/
├── pages/SensitiveWordPage.tsx   # NEW
└── services/sensitiveWord.ts     # NEW
```
