use std::env;
use std::io::Read;
use std::path::Path;

mod support;

use support::secret_input::read_private_file;

const DEV_SEED_ADMIN_PASSWORD_FILE_ENV: &str = "HIVEWEB_DEV_SEED_ADMIN_PASSWORD_FILE";
const DEV_SEED_ADMIN_PHONE_ENV: &str = "HIVEWEB_DEV_SEED_ADMIN_PHONE";
const MAX_PASSWORD_INPUT_BYTES: u64 = 4096;

fn required_admin_phone(value: Option<String>) -> anyhow::Result<String> {
    let phone = value.ok_or_else(|| {
        anyhow::anyhow!("{DEV_SEED_ADMIN_PHONE_ENV} must be set to an 11-digit development account")
    })?;
    anyhow::ensure!(
        phone.len() == 11
            && phone.starts_with('1')
            && phone.chars().all(|character| character.is_ascii_digit()),
        "{DEV_SEED_ADMIN_PHONE_ENV} must be 11 digits starting with 1"
    );
    Ok(phone)
}

fn read_password_line(mut reader: impl Read) -> anyhow::Result<String> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(MAX_PASSWORD_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("failed to read development seed password"))?;
    anyhow::ensure!(
        bytes.len() <= MAX_PASSWORD_INPUT_BYTES as usize,
        "development seed password input is too large"
    );

    let mut password = String::from_utf8(bytes)
        .map_err(|_| anyhow::anyhow!("development seed password must be valid UTF-8"))?;
    if password.ends_with('\n') {
        password.pop();
        if password.ends_with('\r') {
            password.pop();
        }
    }
    anyhow::ensure!(
        !password
            .chars()
            .any(|character| matches!(character, '\r' | '\n')),
        "development seed password must be a single line"
    );
    Ok(password)
}

fn read_password_file(path: &Path) -> anyhow::Result<String> {
    let bytes = read_private_file(path, MAX_PASSWORD_INPUT_BYTES, "development seed password")?;
    read_password_line(bytes.as_slice())
}

fn hash_seed_password(password: &str) -> anyhow::Result<String> {
    match hiveweb::utils::password::hash_password(password) {
        Ok(hash) => Ok(hash),
        Err(hiveweb::utils::error::AppError::BadRequest(_)) => anyhow::bail!(
            "development seed password must be 6-20 characters and contain both letters and digits"
        ),
        Err(_) => anyhow::bail!("failed to hash development seed password"),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override()?;

    let password_file = env::var_os(DEV_SEED_ADMIN_PASSWORD_FILE_ENV).ok_or_else(|| {
        anyhow::anyhow!("{DEV_SEED_ADMIN_PASSWORD_FILE_ENV} must name a private password file")
    })?;
    anyhow::ensure!(
        !password_file.is_empty(),
        "{DEV_SEED_ADMIN_PASSWORD_FILE_ENV} must name a private password file"
    );
    let password = read_password_file(Path::new(&password_file))?;
    let phone = required_admin_phone(env::var(DEV_SEED_ADMIN_PHONE_ENV).ok())?;

    let database_url =
        env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL must be set"))?;
    let pool = sqlx::MySqlPool::connect(&database_url)
        .await
        .map_err(|_| anyhow::anyhow!("failed to connect to the development seed database"))?;

    let nickname = "超级管理员";

    let exists: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM admins WHERE role = 3")
        .fetch_one(&pool)
        .await
        .map_err(|_| anyhow::anyhow!("failed to inspect development seed state"))?;

    if exists.0 > 0 {
        println!("Super admin already exists. Skipping seed.");
        return Ok(());
    }

    let password_hash = hash_seed_password(&password)?;

    sqlx::query(
        r#"
        INSERT INTO admins (phone, nickname, password_hash, role, status)
        VALUES (?, ?, ?, 3, 1)
        "#,
    )
    .bind(&phone)
    .bind(nickname)
    .bind(&password_hash)
    .execute(&pool)
    .await
    .map_err(|_| anyhow::anyhow!("failed to create development seed administrator"))?;

    println!("Initial development super admin created successfully.");
    println!("Role: Super Admin");

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{hash_seed_password, read_password_file, read_password_line, required_admin_phone};

    #[test]
    fn administrator_phone_is_required_and_validated() {
        let valid = format!("1{}", "0".repeat(10));
        assert_eq!(required_admin_phone(Some(valid.clone())).unwrap(), valid);
        assert!(required_admin_phone(None).is_err());
        assert!(required_admin_phone(Some(format!("2{}", "0".repeat(10)))).is_err());
        assert!(required_admin_phone(Some(format!("1{}", "0".repeat(9)))).is_err());
        assert!(required_admin_phone(Some(format!("1{}x", "0".repeat(9)))).is_err());
    }

    #[test]
    fn password_reader_accepts_one_line_without_echoing_or_reformatting() {
        assert_eq!(
            read_password_line(Cursor::new(b"safe password\r\n")).unwrap(),
            "safe password"
        );
        assert!(read_password_line(Cursor::new(b"first\nsecond\n")).is_err());
        assert_eq!(read_password_line(Cursor::new(b"short")).unwrap(), "short");
        assert_eq!(
            hash_seed_password("123456").unwrap_err().to_string(),
            "development seed password must be 6-20 characters and contain both letters and digits"
        );
        assert_eq!(
            hash_seed_password("letters").unwrap_err().to_string(),
            "development seed password must be 6-20 characters and contain both letters and digits"
        );
    }

    #[cfg(unix)]
    #[test]
    fn password_file_requires_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let path = std::env::temp_dir().join(format!(
            "hiveweb-dev-seed-password-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&path, b"safe-password\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_password_file(&path).is_err());

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(read_password_file(&path).unwrap(), "safe-password");
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn password_file_rejects_hard_links() {
        use std::os::unix::fs::PermissionsExt;

        let path = std::env::temp_dir().join(format!(
            "hiveweb-dev-seed-password-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let link = path.with_extension("hard-link");
        std::fs::write(&path, b"safe-password\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::hard_link(&path, &link).unwrap();

        assert!(read_password_file(&path).is_err());
        std::fs::remove_file(link).unwrap();
        std::fs::remove_file(path).unwrap();
    }
}
