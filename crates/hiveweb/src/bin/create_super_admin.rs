use std::env;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override()?;

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");

    let pool = sqlx::MySqlPool::connect(&database_url).await?;

    // Parse command line arguments
    let args: Vec<String> = env::args().collect();

    let mut phone = String::new();
    let mut password = String::new();
    let mut nickname = String::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--phone" => {
                if i + 1 < args.len() {
                    phone = args[i + 1].clone();
                    i += 2;
                } else {
                    eprintln!("Error: --phone requires a value");
                    std::process::exit(1);
                }
            }
            "--password" => {
                if i + 1 < args.len() {
                    password = args[i + 1].clone();
                    i += 2;
                } else {
                    eprintln!("Error: --password requires a value");
                    std::process::exit(1);
                }
            }
            "--nickname" => {
                if i + 1 < args.len() {
                    nickname = args[i + 1].clone();
                    i += 2;
                } else {
                    eprintln!("Error: --nickname requires a value");
                    std::process::exit(1);
                }
            }
            "--help" | "-h" => {
                println!(
                    "Usage: create-super-admin --phone <PHONE> --password <PASSWORD> --nickname <NICKNAME>"
                );
                println!();
                println!("Options:");
                println!("  --phone      Phone number (11 digits)");
                println!("  --password   Password (6-20 characters)");
                println!("  --nickname   Nickname (2-20 characters)");
                println!("  --help, -h   Show this help message");
                std::process::exit(0);
            }
            _ => {
                eprintln!("Unknown option: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    // Validate inputs
    if phone.is_empty() || password.is_empty() || nickname.is_empty() {
        eprintln!("Error: --phone, --password, and --nickname are required");
        eprintln!(
            "Usage: create-super-admin --phone <PHONE> --password <PASSWORD> --nickname <NICKNAME>"
        );
        std::process::exit(1);
    }

    // Validate phone format (11 digits, starts with 1)
    if phone.len() != 11 || !phone.chars().all(|c| c.is_ascii_digit()) || !phone.starts_with('1') {
        eprintln!("Error: Phone number must be 11 digits starting with 1");
        std::process::exit(1);
    }

    // Validate password length
    if password.len() < 6 || password.len() > 20 {
        eprintln!("Error: Password must be 6-20 characters");
        std::process::exit(1);
    }

    // Validate nickname length
    if nickname.len() < 2 || nickname.len() > 20 {
        eprintln!("Error: Nickname must be 2-20 characters");
        std::process::exit(1);
    }

    // Hash password
    let password_hash = bcrypt::hash(&password, bcrypt::DEFAULT_COST)?;

    // Insert super admin
    println!("Creating super admin account...");
    let result = sqlx::query(
        r#"
        INSERT INTO admins (phone, nickname, password_hash, role, status)
        VALUES (?, ?, ?, 3, 1)
        ON DUPLICATE KEY UPDATE nickname = VALUES(nickname)
        "#,
    )
    .bind(&phone)
    .bind(&nickname)
    .bind(&password_hash)
    .execute(&pool)
    .await;

    match result {
        Ok(_) => {
            println!("Super admin created successfully!");
            println!();
            println!("Login credentials:");
            println!("  Phone:    {}", phone);
            println!("  Password: {}", password);
            println!("  Role:     Super Admin");
            println!();
            println!("You can now login at http://localhost:5173");
        }
        Err(e) => {
            eprintln!("Error creating super admin: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}
