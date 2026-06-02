# Deployment Guide: 管理中心 (Admin Center)

**Feature**: `003-admin-center`
**Audience**: Operators deploying the admin center backend (`hiveweb`) and frontend (`web-admin/`) to a server.

---

## 1. Prerequisites

| Component   | Version    | Notes                                                |
| ----------- | ---------- | ---------------------------------------------------- |
| Rust        | 1.85+      | `rustup install stable`                              |
| Node.js     | 18+        | needed only for frontend build (`npm run build`)     |
| MySQL       | 8.0+       | InnoDB, utf8mb4                                      |
| Redis       | 7+         | used for login-failure tracking and rate limiting    |
| MinIO / S3  | any        | object storage (Rustfs-compatible)                   |
| Reverse proxy | Nginx / Caddy | TLS termination + static frontend hosting        |

For local development, `scripts/docker-compose.yml` provisions all infrastructure dependencies (see §3).

---

## 2. Environment Variables

### 2.1 Backend `crates/hiveweb/.env`

```env
# Server
HIVEWEB_HOST=0.0.0.0
HIVEWEB_PORT=3300

# MySQL
DATABASE_URL=mysql://hiveweb:hiveweb@127.0.0.1:3306/hiveweb

# Redis
REDIS_URL=redis://127.0.0.1:6379

# JWT — REQUIRED to be a strong random value in production
JWT_SECRET=replace-with-long-random-string

# CORS
CORS_ALLOWED_ORIGINS=https://admin.example.com

# Logging
RUST_LOG=info,hiveweb=info

# S3 / Rustfs / MinIO
AWS_REGION=us-east-1
AWS_ENDPOINT_URL=http://127.0.0.1:9000
AWS_ACCESS_KEY_ID=minioadmin
AWS_SECRET_ACCESS_KEY=minioadmin
```

Generate a `JWT_SECRET`:

```bash
openssl rand -base64 48
```

### 2.2 Frontend `web-admin/.env.production`

```env
VITE_API_BASE_URL=https://admin.example.com/api
```

---

## 3. Provisioning Infrastructure

### 3.1 Local (docker-compose)

```bash
./scripts/dev-up.sh -d            # start MySQL + Redis + MinIO in background
./scripts/dev-up.sh ps            # show status
./scripts/dev-up.sh logs          # follow logs
./scripts/dev-up.sh down          # stop everything
```

Defaults wired into `scripts/docker-compose.yml`:

- MySQL: `mysql://hiveweb:hiveweb@127.0.0.1:3306/hiveweb` (root password `rootpassword`)
- Redis: `redis://127.0.0.1:6379`
- MinIO: `http://127.0.0.1:9000` (console `:9001`, `minioadmin` / `minioadmin`), bucket `hiveweb` auto-created

### 3.2 Managed / production

Provision MySQL, Redis, and an S3-compatible bucket through your platform of choice (RDS, ElastiCache, S3, etc.). Update `crates/hiveweb/.env` with the production endpoints. Network policy should allow only the backend host to reach MySQL, Redis, and the bucket.

---

## 4. Database Migration

Migrations are embedded in `crates/hiveweb` and executed by a dedicated binary.

```bash
cd crates/hiveweb
cargo run --bin migrate                # apply pending migrations
```

Migration files live under `crates/hiveweb/migrations/` (e.g. `V001__create_admins_table.sql`, `V002__create_login_records_table.sql`).

To roll back, restore the prior database snapshot — migrations are forward-only.

---

## 5. Bootstrap Super Admin

After migrating, create the initial super admin (role = 3):

```bash
cargo run --bin create-super-admin -- \
  --phone 18810154696 \
  --password 'change-me-now' \
  --nickname 'Super Admin'
```

> 提示：二进制名使用连字符 `create-super-admin`（与文件名 `create_super_admin.rs` 不同；后者是 Rust 标识符风格，前者是 `[[bin]] name` 字段）。

Re-running with the same phone is rejected (uniqueness). To seed test data, use `cargo run --bin seed`.

---

## 6. Build & Run

### 6.1 Backend (release)

```bash
cd crates/hiveweb
cargo build --release
./target/release/migrate                     # apply migrations
./target/release/hiveweb                     # start API on $HIVEWEB_HOST:$HIVEWEB_PORT
```

Recommended systemd unit (`/etc/systemd/system/hiveweb.service`):

```ini
[Unit]
Description=Hive Admin Center API
After=network.target mysql.service redis.service

[Service]
User=hiveweb
WorkingDirectory=/opt/hiveweb
EnvironmentFile=/opt/hiveweb/.env
ExecStart=/opt/hiveweb/hiveweb
Restart=on-failure
RestartSec=5s

[Install]
WantedBy=multi-user.target
```

### 6.2 Frontend (static build)

```bash
cd web-admin
npm ci
npm run build                                # outputs to web-admin/dist
```

Copy `web-admin/dist/` to the web server document root (e.g. `/var/www/admin-center/`).

---

## 7. Reverse Proxy (Nginx)

```nginx
server {
    listen 443 ssl http2;
    server_name admin.example.com;

    ssl_certificate     /etc/letsencrypt/live/admin.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/admin.example.com/privkey.pem;

    # Static SPA
    root /var/www/admin-center;
    index index.html;
    location / {
        try_files $uri $uri/ /index.html;
    }

    # API
    location /api/ {
        proxy_pass http://127.0.0.1:3300;
        proxy_set_header Host              $host;
        proxy_set_header X-Real-IP         $remote_addr;
        proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}

server {
    listen 80;
    server_name admin.example.com;
    return 301 https://$host$request_uri;
}
```

---

## 8. Production Hardening Checklist

- [ ] `JWT_SECRET` is a freshly-generated random value (≥ 32 bytes) — never use the example value.
- [ ] `CORS_ALLOWED_ORIGINS` is set to the explicit frontend origin (not `*`).
- [ ] MySQL user `hiveweb` has only the privileges it needs on the `hiveweb` schema (no `SUPER`, no cross-DB access).
- [ ] Redis is bound to a private network or protected with `requirepass`.
- [ ] S3/MinIO bucket policy restricts read/write to the backend's IAM credentials.
- [ ] TLS terminates at the reverse proxy; HTTP redirects to HTTPS.
- [ ] Default super admin password is rotated immediately after first login.
- [ ] `RUST_LOG=info` (not `debug`) in production; structured logs forwarded to your log aggregator.
- [ ] Rate-limiting middleware (`crates/hiveweb-admin/src/middleware/rate_limit.rs`) tuned for your traffic profile.
- [ ] Backups of the MySQL `hiveweb` database scheduled and tested.

---

## 9. Smoke Test

After deployment:

```bash
# 1. Backend health
curl -fsS https://admin.example.com/api/health

# 2. Login (returns JWT)
curl -fsS -X POST https://admin.example.com/api/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"phone":"18810154696","password":"<new-password>"}'

# 3. Authenticated request
TOKEN=...   # from previous response
curl -fsS https://admin.example.com/api/auth/me -H "Authorization: Bearer $TOKEN"
```

---

## 10. Rollback

1. Stop the `hiveweb` service: `systemctl stop hiveweb`.
2. Deploy the previous release binary and `web-admin/dist/` artefacts.
3. If a migration must be reverted, restore the pre-migration MySQL snapshot.
4. Restart: `systemctl start hiveweb`.

---

## 11. Troubleshooting

| Symptom                                  | Where to look                                                     |
| ---------------------------------------- | ----------------------------------------------------------------- |
| 500 on `/api/auth/login`                 | Backend logs (`journalctl -u hiveweb`); check `DATABASE_URL`      |
| `401 Token invalid`                      | Verify `JWT_SECRET` matches the value at the time of token issue  |
| CORS preflight blocked                   | `CORS_ALLOWED_ORIGINS` must contain the frontend origin           |
| `Too many failed attempts`               | Redis key `login:fail:<phone>` — clear with `DEL` to unlock       |
| Object upload fails                      | `AWS_ENDPOINT_URL` reachable from backend; bucket `hiveweb` exists |

Additional context: see `specs/003-admin-center/quickstart.md` and `crates/hiveweb/README.md` (if present).
