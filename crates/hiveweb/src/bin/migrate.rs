use sqlx::{MySql, Pool};
use std::env;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv()?;
    
    let database_url = env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set");
    
    println!("Connecting to database...");
    let pool = sqlx::MySqlPool::connect(&database_url).await?;
    
    println!("Running migrations...");
    
    // Create admins table
    println!("Creating admins table...");
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS admins (
            id BIGINT PRIMARY KEY AUTO_INCREMENT,
            phone VARCHAR(11) NOT NULL UNIQUE,
            nickname VARCHAR(20) NOT NULL,
            password_hash VARCHAR(60) NOT NULL,
            role TINYINT NOT NULL DEFAULT 1 COMMENT '1=Normal, 2=System, 3=Super',
            status TINYINT NOT NULL DEFAULT 1 COMMENT '1=Active, 0=Disabled',
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
            last_login_at DATETIME DEFAULT NULL,
            INDEX idx_phone (phone)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='管理员表'
        "#
    ).execute(&pool).await?;
    
    // Create login_records table
    println!("Creating login_records table...");
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS login_records (
            id BIGINT PRIMARY KEY AUTO_INCREMENT,
            admin_id BIGINT NOT NULL,
            login_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            ip_address VARCHAR(45) NOT NULL COMMENT 'IPv4 or IPv6',
            success TINYINT(1) NOT NULL COMMENT '1=true, 0=false',
            failure_reason VARCHAR(50) DEFAULT NULL COMMENT 'WRONG_PASSWORD, ACCOUNT_DISABLED, etc.',
            INDEX idx_admin_id (admin_id),
            INDEX idx_login_at (login_at),
            CONSTRAINT fk_login_admin FOREIGN KEY (admin_id) REFERENCES admins(id) ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='登录记录表'
        "#
    ).execute(&pool).await?;
    
    println!("Migrations completed successfully!");
    
    Ok(())
}
