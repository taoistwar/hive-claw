# Performance Evidence — 管理中心

**Tracks**: T098 / T099 / T100 / T101 (Phase 7, tasks.md)
**Spec**: SC-002 (admin list ≤ 2s) · SC-003 (dashboard ≤ 3s) · SC-005 (100+ admins) · Constitution Principle IV (API p95 < 200ms, index-backed queries)

> **现状**：基准脚本与数据集生成器已就绪；实际数据记录（p95 时间序列）由 CI / 本地执行后填入下面的表格。

---

## 1. 数据集生成

```bash
# 100 个 admins × 100 条 login_records each = 10 000 行 login_records
cargo run -p hiveweb --bin seed-bench -- 100 100

# SC-005 验收用：100 admins × 10 000 each = 1 000 000 行（大数据集）
cargo run -p hiveweb --bin seed-bench -- 100 10000
```

清空数据：`docker compose down -v` 后重新 `migrate`。

---

## 2. 跑基准（T098 / T099）

需要后端运行：

```bash
cargo run -p hiveweb &                  # backend on :3300
sleep 2                                 # let it bind
```

使用 wrk 或 hey 直接打 HTTP（不需要 criterion；HTTP-level p95 才是 spec 的契约）：

```bash
# T098 — admin list
TOKEN=$(curl -s -X POST http://localhost:3300/api/auth/login \
  -H 'content-type: application/json' \
  -d '{"phone":"13800138000","password":"admin123"}' | jq -r .data.token)

wrk -t4 -c50 -d30s \
  -H "Authorization: Bearer $TOKEN" \
  'http://localhost:3300/api/admins?offset=0&limit=10'

# T099 — dashboard
wrk -t4 -c50 -d30s \
  -H "Authorization: Bearer $TOKEN" \
  'http://localhost:3300/api/dashboard/stats'

wrk -t4 -c50 -d30s \
  -H "Authorization: Bearer $TOKEN" \
  'http://localhost:3300/api/dashboard/recent-logins?limit=10'
```

> wrk 没有原生 p95，使用 `wrk2` 或 `hey -z 30s -c 50` 取代获得分位数。

填入下表（保留每次跑的 commit hash）：

| 端点 | 数据集 | 工具 | concurrency | p50 (ms) | p95 (ms) | spec 上限 | 通过? | commit |
| ---- | ------ | ---- | ----------- | -------- | -------- | --------- | ----- | ------ |
| `GET /api/admins?page=1&page_size=10` | 100 admins | hey | 50 | _todo_ | _todo_ | 2000 (SC-002) | _ | _ |
| `GET /api/dashboard/stats` | 100 admins / 10k logs | hey | 50 | _todo_ | _todo_ | 3000 (SC-003) | _ | _ |
| `GET /api/dashboard/recent-logins?limit=10` | 同上 | hey | 50 | _todo_ | _todo_ | 3000 (SC-003) | _ | _ |
| `POST /api/auth/login` | — | hey | 20 | _todo_ | _todo_ | 200 (Principle IV) | _ | _ |

---

## 3. EXPLAIN 分析（T100 / Principle IV）

在 100×10k 数据集上对每条热路径查询跑 EXPLAIN，确认使用预期的索引：

```sql
-- admins 列表（按 created_at DESC 分页）
EXPLAIN SELECT * FROM admins ORDER BY created_at DESC LIMIT 10 OFFSET 0;
-- 期望：filesort 或 index by created_at；可考虑 idx_created_at DESC

-- 登录查询（按 phone 唯一）
EXPLAIN SELECT * FROM admins WHERE phone = '13800138000';
-- 期望：type=const 或 ref，使用 phone 的 UNIQUE 索引

-- 仪表盘 recent-logins（按 login_at DESC）
EXPLAIN SELECT id, admin_id, admin_nickname_snapshot AS admin_nickname,
              login_at, ip_address, success
       FROM login_records ORDER BY login_at DESC LIMIT 10;
-- 期望：使用 idx_login_at (V005 改成了 DESC 索引)

-- 仪表盘 stats — today_logins
EXPLAIN SELECT COUNT(*) FROM login_records WHERE success = 1 AND DATE(login_at) = CURDATE();
-- 期望：range scan on idx_login_at（DATE() 包裹 login_at 会让索引失效；
--      若 EXPLAIN 显示 ALL，需要改写为 BETWEEN startOfToday AND endOfToday）

-- 仪表盘 stats — online_admins
EXPLAIN SELECT COUNT(*) FROM admins
        WHERE status = 1 AND last_login_at IS NOT NULL
          AND last_login_at >= DATE_SUB(NOW(), INTERVAL 24 HOUR);
-- 期望：可能 filtered=低；若高频访问，可考虑加 (status, last_login_at) 复合索引
```

填入：

| 查询 | EXPLAIN type | rows | key | extra | 满足 Principle IV? |
| ---- | ------------ | ---- | --- | ----- | ------------------ |
| _todo_ | _todo_ | _todo_ | _todo_ | _todo_ | _todo_ |

---

## 4. SC-005 验收（T101）

数据集：100 admins + 10 000 login_records each（合计 100 万行 login_records）。
跑步 T098 / T099 的 wrk/hey，确认 p95 仍在预算内。如果不在：
1. 先看 EXPLAIN 是否退化
2. 再看连接池 max_connections / Redis 是否饱和
3. 最后才考虑加索引或改查询

---

## 5. 历史结果

记录每次基准跑的环境（dev/staging/prod）+ git sha + 结论。空着，等你 / CI 填入。

| 日期 | 环境 | git sha | 数据集 | 通过? | 备注 |
| ---- | ---- | ------- | ------ | ----- | ---- |
| _todo_ | _todo_ | _todo_ | _todo_ | _todo_ | _todo_ |
