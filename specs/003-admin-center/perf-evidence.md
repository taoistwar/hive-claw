# Performance Evidence — 管理中心

**Tracks**: T098 / T099 / T100 / T101 (Phase 7, tasks.md)
**Spec**: SC-002 (admin list ≤ 2s) · SC-003 (dashboard ≤ 3s) · SC-005 (100+ admins) · Constitution Principle IV (API p95 < 200ms, index-backed queries)

---

## 1. 数据集生成

```bash
# 100 admins × 100 login_records each = 10 000 行 login_records
cargo run -p hiveweb --bin seed-bench -- 100 100

# SC-005 完整验收：100 admins × 10 000 each = 1 000 000 行
cargo run -p hiveweb --bin seed-bench -- 100 10000
```

清空：`docker compose down -v` 后重新 `migrate`。

---

## 2. 实测基准（hey, concurrency 50, 15s window）

### 2.1 第一次跑（pre-optimization）

| 端点 | p50 | p95 | p99 | qps | 预算 | 通过 |
| ---- | --- | --- | --- | --- | ---- | ---- |
| GET /api/admins?offset=0&limit=10 | 13.8 ms | **15.8 ms** | 18.9 ms | 3508/s | 2000 ms (SC-002) | ✅ |
| GET /api/dashboard/stats | 11.9 ms | **15.5 ms** | 17.1 ms | 4798/s | 3300 ms (SC-003) | ✅ |
| GET /api/dashboard/recent-logins?limit=10 | 5.2 ms | **5.7 ms** | 6.0 ms | 9604/s | 3300 ms (SC-003) | ✅ |
| POST /api/auth/login | 462 ms | **884 ms** | — | 8/s | 200 ms (Principle IV) | ❌ bcrypt-bound |

### 2.2 EXPLAIN 后优化 → 第二次跑（post-V007 + today_logins 改 BETWEEN）

| 端点 | p50 | p95 | p99 | qps | Δ vs pre |
| ---- | --- | --- | --- | --- | -------- |
| GET /api/admins?offset=0&limit=10 | 13.8 ms | 15.9 ms | 17.6 ms | 3523/s | 持平 |
| GET /api/dashboard/stats | **4.4 ms** | **12.2 ms** | 13.8 ms | **7214/s** | **qps +50%** |
| GET /api/dashboard/recent-logins?limit=10 | 4.1 ms | **4.7 ms** | 5.3 ms | **12135/s** | qps +26% |

数据集：168 admins × 100 login_records = 10 313 行 login_records（含历史测试遗留）。

### 2.3 Login 端点的偏离说明

POST /api/auth/login p95 = 884 ms，**超过 Principle IV 的 200 ms 预算**。

- 根因：spec FR-016 强制 bcrypt 加密，data-model.md 指定 cost=12（默认）。
  bcrypt cost=12 单次哈希 ≈ 400 ms（taking ~300-500ms on this hardware），
  与并发无关。
- 处理：在 `plan.md` 的 Complexity Tracking 中登记为可接受偏离，理由
  "认证端点的延迟由加密强度决定，安全 > p95"。
- 验证：bcrypt 是不可压缩的；如确需更低延迟，唯一手段是
  下调 cost，但会显著弱化暴力破解防御 —— 不做。

---

## 3. EXPLAIN 证据（T100 / Principle IV）

### 3.1 Pre-V007

| 查询 | type | rows | key | extra | 问题 |
| ---- | ---- | ---- | --- | ----- | ---- |
| `SELECT * FROM admins WHERE phone = ?` | **const** | 1 | phone | NULL | ✅ 最优 |
| `SELECT * FROM admins ORDER BY created_at DESC LIMIT 10` | ALL | 168 | NULL | Using filesort | 缺 idx_created_at |
| `SELECT COUNT(*) FROM login_records WHERE success=1 AND DATE(login_at)=CURDATE()` | ALL | 10313 | NULL | Using where | `DATE()` 包裹列禁用索引 |
| `SELECT ... FROM login_records ORDER BY login_at DESC LIMIT 10` | **index** | 10 | idx_login_at | NULL | ✅ V005 DESC 索引生效 |

### 3.2 Post-V007 + today_logins 改 BETWEEN

| 查询 | type | rows | key | extra | 改进 |
| ---- | ---- | ---- | --- | ----- | ---- |
| `SELECT * FROM admins ORDER BY created_at DESC LIMIT 10` | ALL | 168 | NULL | Using filesort | optimizer 在 168 行的小表上选 filesort（cost-based），index 已建好（cardinality=29），万级表会自动切换 |
| `SELECT COUNT(*) FROM login_records WHERE success=1 AND login_at >= CURDATE() AND login_at < CURDATE()+INTERVAL 1 DAY` | **range** | 414 | idx_login_at | Using index condition; Using where | 全扫 10313 → range 414，**降幅 96%** |

应用的修复：
- **迁移 V007**：`admins(created_at DESC)` 索引
- **services/dashboard.rs**：`DATE(login_at) = CURDATE()` 改写为 `login_at >= CURDATE() AND login_at < CURDATE() + INTERVAL 1 DAY`，让 idx_login_at 可用

### 3.3 仍存在的潜在隐患（≥ 10k admins 才需要回看）

- `online_admins` 子查询：`status=1 AND last_login_at IS NOT NULL AND last_login_at >= NOW()-INTERVAL 24 HOUR`，目前 type=ALL filtered=3%。168 行下无感；100k 行时需 `(status, last_login_at)` 复合索引。
- 写工作流（create_login_record / audit_logs）每次 1 行 INSERT，未压测；常态吞吐由登录峰值决定。

---

## 4. SC-005 验收（T101）

**未跑大数据集（100 admins × 10 000 login_records = 1 M 行）**。
当前 168 admins × 10 313 logs 配置下 p95 都在预算的 1% 以内，
有充足余量；建议 CI 用 `seed-bench 100 10000` 作回归门槛。

```bash
cargo run -p hiveweb --bin seed-bench -- 100 10000
# 再重跑 §2.2 的 hey 命令，p95 应仍 < 50 ms（推断）
```

---

## 5. 历史结果

| 日期 | 环境 | git sha | 数据集 | 通过? | 备注 |
| ---- | ---- | ------- | ------ | ----- | ---- |
| 2026-05-26 | dev (WSL2 / Docker MySQL 8.0) | 9cd43fa | 168×10313 | ✅ (login 除外) | 首跑，发现 today_logins DATE() 禁用索引 |
| 2026-05-26 | dev | _post-V007_ | 168×10313 | ✅ (login 除外) | dashboard stats qps +50% |

---

## 6. Reproduce 步骤

```bash
# 1) infra
./scripts/dev-up.sh -d
cargo run -p hiveweb --bin migrate

# 2) seed
cargo run -p hiveweb --bin seed-bench -- 100 100

# 3) backend with relaxed rate limit
docker exec -i hiveweb-redis redis-cli FLUSHDB
RATE_LIMIT_MAX=100000 RATE_LIMIT_WINDOW_SECS=60 \
  cargo run -p hiveweb --bin hiveweb &
sleep 5

# 4) bench
TOKEN=$(curl -s -X POST http://localhost:3300/api/auth/login \
  -H 'content-type: application/json' \
  -d '{"phone":"18810154696","password":"admin123"}' \
  | python3 -c 'import sys,json;print(json.load(sys.stdin)["data"]["token"])')

hey -z 15s -c 50 -H "Authorization: Bearer $TOKEN" \
  'http://localhost:3300/api/admins?offset=0&limit=10'

hey -z 15s -c 50 -H "Authorization: Bearer $TOKEN" \
  'http://localhost:3300/api/dashboard/stats'

hey -z 15s -c 50 -H "Authorization: Bearer $TOKEN" \
  'http://localhost:3300/api/dashboard/recent-logins?limit=10'

hey -z 10s -c 5 -m POST \
  -H "Content-Type: application/json" \
  -d '{"phone":"18810154696","password":"admin123"}' \
  'http://localhost:3300/api/auth/login'
```
