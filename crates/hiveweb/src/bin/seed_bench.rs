//! T101 / SC-005 性能基准的数据集生成器。
//!
//! 用法：
//!   cargo run -p hiveweb --bin seed_bench -- 100 10000
//!
//! 参数：admins 数量（默认 100）、每个 admin 的 login_records 数量
//! （默认 100，总计 100×100=10 000）。
//!
//! 注意：会清空既有的 admins / login_records（保留 role=3 的 Super 管理员
//! 以避免锁掉系统），适合在专用基准库上跑。

use std::env;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override()?;
    let args: Vec<String> = env::args().collect();
    let n_admins: usize = args
        .get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);
    let logins_per_admin: usize = args
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);

    let url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::MySqlPool::connect(&url).await?;

    println!(
        "Seeding bench dataset: {} admins × {} login_records each ({} total rows)",
        n_admins,
        logins_per_admin,
        n_admins * logins_per_admin
    );

    // Clean non-super admins + their login records (FK SET NULL retains
    // historic rows; we delete them explicitly here for a clean baseline).
    sqlx::query("DELETE FROM login_records WHERE admin_id IS NULL OR admin_id IN (SELECT id FROM admins WHERE role <> 3)")
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM admins WHERE role <> 3").execute(&pool).await?;

    let password_hash = bcrypt::hash("bench-pass-1", bcrypt::DEFAULT_COST)?;

    for i in 0..n_admins {
        // 11-digit phone in 139xxxxxxxx range, padded with index.
        let phone = format!("139{:08}", 10_000_000 + i);
        let nickname = format!("bench-{}", i);
        let role: i8 = if i % 10 == 0 { 2 } else { 1 };

        let result = sqlx::query(
            r#"
            INSERT INTO admins (phone, nickname, password_hash, role, status, created_at, updated_at, last_login_at)
            VALUES (?, ?, ?, ?, 1, NOW(), NOW(), NOW() - INTERVAL FLOOR(RAND()*48) HOUR)
            "#,
        )
        .bind(&phone)
        .bind(&nickname)
        .bind(&password_hash)
        .bind(role)
        .execute(&pool)
        .await?;
        let admin_id = result.last_insert_id() as i64;

        // Batch INSERT login records — split into chunks to stay under the
        // max_allowed_packet limit. 200 rows per statement is conservative.
        for chunk in (0..logins_per_admin).collect::<Vec<_>>().chunks(200) {
            let mut sql = String::from(
                "INSERT INTO login_records \
                 (admin_id, admin_phone_snapshot, admin_nickname_snapshot, \
                  login_at, ip_address, success, failure_reason) VALUES ",
            );
            let mut first = true;
            for _ in chunk {
                if !first {
                    sql.push(',');
                }
                first = false;
                sql.push_str(
                    "(?, ?, ?, NOW() - INTERVAL FLOOR(RAND()*720) HOUR, '127.0.0.1', 1, NULL)",
                );
            }
            let mut q = sqlx::query(&sql);
            for _ in chunk {
                q = q.bind(admin_id).bind(&phone).bind(&nickname);
            }
            q.execute(&pool).await?;
        }

        if (i + 1) % 25 == 0 {
            println!("  inserted {}/{} admins", i + 1, n_admins);
        }
    }

    println!("Seed complete.");
    Ok(())
}
