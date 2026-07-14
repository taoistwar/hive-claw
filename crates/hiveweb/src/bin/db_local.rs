use sqlx::Row;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    let url = std::env::var("EXTERNAL_DB_URL")?;
    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(5)
        .min_connections(5)
        .connect(&url)
        .await?;

    let identity = sqlx::query("SELECT DATABASE() db, @@hostname host, NOW() db_now")
        .fetch_one(&pool)
        .await?;
    println!(
        "db={} host={} now={}",
        identity.try_get::<String, _>("db")?,
        identity.try_get::<String, _>("host")?,
        identity.try_get::<chrono::NaiveDateTime, _>("db_now")?
    );

    let row = sqlx::query(
        "SELECT id, membership_level, effective_end_time FROM cc_user_membership \
         WHERE user_id = 1006419 AND effective_end_time > NOW() \
         AND effective_start_time <= NOW() LIMIT 1",
    )
    .fetch_optional(&pool)
    .await?;
    println!("literal_row={:?}", row.map(|r| r.try_get::<i64, _>("id")));

    let row = sqlx::query(
        "SELECT id, membership_level, effective_end_time FROM cc_user_membership \
         WHERE user_id = ? AND effective_end_time > NOW() \
         AND effective_start_time <= NOW() LIMIT 1",
    )
    .bind(1_006_419_i64)
    .fetch_optional(&pool)
    .await?;
    println!("bound_row={:?}", row.map(|r| r.try_get::<i64, _>("id")));
    Ok(())
}
