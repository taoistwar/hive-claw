use std::env;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override()?;

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::MySqlPool::connect(&database_url).await?;

    let phone = "18810154696";
    let password = "admin123";
    let nickname = "超级管理员";

    let exists: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM admins WHERE role = 3")
        .fetch_one(&pool)
        .await?;

    if exists.0 > 0 {
        println!("Super admin already exists. Skipping seed.");
        return Ok(());
    }

    let password_hash = bcrypt::hash(password, bcrypt::DEFAULT_COST)?;

    sqlx::query(
        r#"
        INSERT INTO admins (phone, nickname, password_hash, role, status)
        VALUES (?, ?, ?, 3, 1)
        "#,
    )
    .bind(phone)
    .bind(nickname)
    .bind(&password_hash)
    .execute(&pool)
    .await?;

    println!("Initial super admin created successfully!");
    println!();
    println!("Login credentials:");
    println!("  Phone:    {}", phone);
    println!("  Password: {}", password);
    println!("  Nickname: {}", nickname);
    println!("  Role:     Super Admin");
    println!();
    println!("IMPORTANT: Change the default password after first login!");

    Ok(())
}
