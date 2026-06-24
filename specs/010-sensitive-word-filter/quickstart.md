# Quickstart: 敏感词过滤

**Feature**: 010-sensitive-word-filter | **Date**: 2026-06-07

## Prerequisites

- Running hiveweb server with MySQL
- Database migrations applied: `cargo run -p hiveweb --bin migrate`
- ADMIN_SECRET 环境变量已设置（用于管理 API 认证）

## 1. 数据库迁移

```bash
# 新迁移文件已在 crates/hiveweb/src/db/ 中
cargo run -p hiveweb --bin migrate
```

验证：`SHOW TABLES LIKE 'sensitive_words';` 和 `sensitive_filter_logs` 存在。

## 2. 添加测试敏感词

```bash
# 通过管理 API 添加精确匹配敏感词
curl -X POST http://localhost:3000/api/sensitive-words \
  -H "Authorization: Bearer ${ADMIN_TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{"word": "测试敏感词", "match_mode": "exact"}'

# 添加正则匹配敏感词
curl -X POST http://localhost:3000/api/sensitive-words \
  -H "Authorization: Bearer ${ADMIN_TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{"word": "\\d{15,19}", "match_mode": "regex"}'
```

## 3. 测试过滤

```bash
# 输入拦截 — 应返回 {"reply": "抱歉...", "filtered": true}
curl -X POST 'http://localhost:3000/api/assistant?sign=...' \
  -H "Content-Type: application/json; charset=UTF-8" \
  -d '{"user_id": 1, "message": "包含测试敏感词的消息", "channel": "web", "platform": "web", "app_version": "1.0.0"}'

# 正常消息 — 应正常返回 Agent 回复
curl -X POST 'http://localhost:3000/api/assistant?sign=...' \
  -H "Content-Type: application/json; charset=UTF-8" \
  -d '{"user_id": 1, "message": "你好", "channel": "web", "platform": "web", "app_version": "1.0.0"}'
```

## 4. 运行测试

```bash
# Unit tests
cargo test -p hiveweb --lib -- sensitive_filter

# Integration tests (需要测试数据库)
cargo test -p hiveweb --test it_sensitive_filter

# 特定测试
cargo test -p hiveweb --test it_sensitive_filter -- test_input_block_exact_match
cargo test -p hiveweb --test it_sensitive_filter -- test_regex_match
cargo test -p hiveweb --test it_sensitive_filter -- test_cache_refresh
```

## 5. 管理后台

登录 web-admin (`http://localhost:5173`)，导航到「敏感词管理」页面，可进行 CRUD 操作。

## 开发顺序

1. **models/sensitive_word.rs** — DB struct + request/response types
2. **DB migration** — `sensitive_words` + `sensitive_filter_logs` 表
3. **services/sensitive_filter.rs** — 匹配引擎 + 缓存 + DB 查询
4. **api/sensitive_word.rs** — CRUD API + 缓存刷新
5. **api/chat_assistant.rs** — 输入拦截（在 message 校验之后、Agent 之前插入）
6. **runtime/orchestrator.rs** — 输出替换（Agent 回复后、返回前）
7. **前端** — `SensitiveWordPage.tsx` + `sensitiveWord.ts` service
8. **测试** — `it_sensitive_filter.rs` integration tests
