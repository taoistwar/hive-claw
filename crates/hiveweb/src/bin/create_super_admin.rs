use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::{ArgGroup, Parser};

mod support;

use support::secret_input::read_private_file;

const MAX_PASSWORD_INPUT_BYTES: u64 = 4096;
const DUPLICATE_BOOTSTRAP_MESSAGE: &str =
    "super administrator phone already exists; use the authenticated password-change flow";
const BOOTSTRAP_INSERT_SQL: &str = r#"
    INSERT INTO admins (phone, nickname, password_hash, role, status)
    VALUES (?, ?, ?, 3, 1)
"#;

#[derive(Debug, Parser)]
#[command(
    about = "Bootstrap the HiveWeb super administrator with INSERT-only semantics; duplicate phones fail",
    group(
        ArgGroup::new("password_source")
            .required(true)
            .multiple(false)
            .args(["password_stdin", "password_file"])
    )
)]
struct Cli {
    #[arg(long)]
    phone: String,
    #[arg(long)]
    nickname: String,
    /// Read the password from piped stdin. Interactive terminals are rejected
    /// because a normal stdin read would echo the password.
    #[arg(long)]
    password_stdin: bool,
    /// Read the password from a regular file. On Unix it must have mode 0600
    /// or stricter and must not be a symbolic link.
    #[arg(long, value_name = "PATH")]
    password_file: Option<PathBuf>,
}

fn read_password_line(mut reader: impl Read) -> anyhow::Result<String> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(MAX_PASSWORD_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("failed to read administrator password"))?;
    anyhow::ensure!(
        bytes.len() <= MAX_PASSWORD_INPUT_BYTES as usize,
        "administrator password input is too large"
    );

    let mut password = String::from_utf8(bytes)
        .map_err(|_| anyhow::anyhow!("administrator password must be valid UTF-8"))?;
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
        "administrator password must be a single line"
    );
    Ok(password)
}

fn read_password_file(path: &Path) -> anyhow::Result<String> {
    let bytes = read_private_file(path, MAX_PASSWORD_INPUT_BYTES, "administrator password")?;
    read_password_line(bytes.as_slice())
}

fn read_password(cli: &Cli) -> anyhow::Result<String> {
    if cli.password_stdin {
        anyhow::ensure!(
            !std::io::stdin().is_terminal(),
            "--password-stdin requires piped input; use a mode-0600 --password-file for interactive use"
        );
        read_password_line(std::io::stdin().lock())
    } else {
        read_password_file(
            cli.password_file
                .as_deref()
                .context("--password-file is required")?,
        )
    }
}

fn hash_bootstrap_password(password: &str) -> anyhow::Result<String> {
    match hiveweb::utils::password::hash_password(password) {
        Ok(hash) => Ok(hash),
        Err(hiveweb::utils::error::AppError::BadRequest(_)) => anyhow::bail!(
            "administrator password must be 6-20 characters and contain both letters and digits"
        ),
        Err(_) => anyhow::bail!("failed to hash administrator password"),
    }
}

fn map_bootstrap_insert_error(error: sqlx::Error) -> anyhow::Error {
    if error
        .as_database_error()
        .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
    {
        anyhow::anyhow!(DUPLICATE_BOOTSTRAP_MESSAGE)
    } else {
        anyhow::anyhow!("failed to create super administrator")
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override()?;
    let cli = Cli::parse();
    let password = read_password(&cli)?;

    anyhow::ensure!(
        cli.phone.len() == 11
            && cli
                .phone
                .chars()
                .all(|character| character.is_ascii_digit())
            && cli.phone.starts_with('1'),
        "phone number must be 11 digits starting with 1"
    );
    anyhow::ensure!(
        (2..=20).contains(&cli.nickname.len()),
        "nickname must be 2-20 characters"
    );

    let password_hash = hash_bootstrap_password(&password)?;
    let database_url =
        std::env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL must be set"))?;
    let pool = sqlx::MySqlPool::connect(&database_url)
        .await
        .map_err(|_| anyhow::anyhow!("failed to connect to the administrator database"))?;

    sqlx::query(BOOTSTRAP_INSERT_SQL)
        .bind(&cli.phone)
        .bind(&cli.nickname)
        .bind(&password_hash)
        .execute(&pool)
        .await
        .map_err(map_bootstrap_insert_error)?;

    println!("Super admin created successfully.");
    println!("Role: Super Admin");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::error::Error;
    use std::fmt::{Display, Formatter};
    use std::io::Cursor;

    use clap::{CommandFactory, Parser};
    use sqlx::error::{DatabaseError, ErrorKind};

    use super::{
        BOOTSTRAP_INSERT_SQL, Cli, DUPLICATE_BOOTSTRAP_MESSAGE, hash_bootstrap_password,
        map_bootstrap_insert_error, read_password_file, read_password_line,
    };

    #[derive(Debug)]
    struct UniqueViolation;

    impl Display for UniqueViolation {
        fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("SECRET_DATABASE_ERROR_MUST_NOT_ESCAPE")
        }
    }

    impl Error for UniqueViolation {}

    impl DatabaseError for UniqueViolation {
        fn message(&self) -> &str {
            "SECRET_DATABASE_ERROR_MUST_NOT_ESCAPE"
        }

        fn code(&self) -> Option<Cow<'_, str>> {
            Some(Cow::Borrowed("1062"))
        }

        fn as_error(&self) -> &(dyn Error + Send + Sync + 'static) {
            self
        }

        fn as_error_mut(&mut self) -> &mut (dyn Error + Send + Sync + 'static) {
            self
        }

        fn into_error(self: Box<Self>) -> Box<dyn Error + Send + Sync + 'static> {
            self
        }

        fn kind(&self) -> ErrorKind {
            ErrorKind::UniqueViolation
        }
    }

    #[test]
    fn bootstrap_sql_is_insert_only() {
        assert!(BOOTSTRAP_INSERT_SQL.contains("INSERT INTO admins"));
        assert!(!BOOTSTRAP_INSERT_SQL.contains("ON DUPLICATE KEY"));
        assert!(!BOOTSTRAP_INSERT_SQL.contains("UPDATE"));
    }

    #[test]
    fn help_describes_insert_only_bootstrap_and_duplicate_failure() {
        let help = Cli::command().render_long_help().to_string();

        assert!(help.contains("INSERT-only"));
        assert!(help.contains("duplicate phones fail"));
        assert!(!help.contains("Create or update"));
    }

    #[test]
    fn duplicate_phone_maps_to_a_static_bootstrap_failure() {
        let mapped = map_bootstrap_insert_error(sqlx::Error::Database(Box::new(UniqueViolation)));
        assert_eq!(mapped.to_string(), DUPLICATE_BOOTSTRAP_MESSAGE);
        assert!(
            !mapped
                .to_string()
                .contains("SECRET_DATABASE_ERROR_MUST_NOT_ESCAPE")
        );
    }

    #[test]
    fn bootstrap_password_uses_the_shared_letter_and_digit_policy() {
        assert_eq!(
            hash_bootstrap_password("123456").unwrap_err().to_string(),
            "administrator password must be 6-20 characters and contain both letters and digits"
        );
        assert_eq!(
            hash_bootstrap_password("letters").unwrap_err().to_string(),
            "administrator password must be 6-20 characters and contain both letters and digits"
        );
    }

    #[test]
    fn parser_rejects_plaintext_password_argument() {
        assert!(
            Cli::try_parse_from([
                "create-super-admin",
                "--phone",
                "13900000000",
                "--nickname",
                "super",
                "--password",
                "SECRET_SENTINEL",
            ])
            .is_err()
        );
    }

    #[test]
    fn parser_requires_exactly_one_safe_password_source() {
        assert!(
            Cli::try_parse_from([
                "create-super-admin",
                "--phone",
                "13900000000",
                "--nickname",
                "super",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "create-super-admin",
                "--phone",
                "13900000000",
                "--nickname",
                "super",
                "--password-stdin",
                "--password-file",
                "/run/secrets/hiveweb-admin",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "create-super-admin",
                "--phone",
                "13900000000",
                "--nickname",
                "super",
                "--password-stdin",
            ])
            .is_ok()
        );
    }

    #[test]
    fn password_reader_accepts_one_line_without_echoing_or_reformatting() {
        assert_eq!(
            read_password_line(Cursor::new(b"safe password\r\n")).unwrap(),
            "safe password"
        );
        assert!(read_password_line(Cursor::new(b"first\nsecond\n")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn password_file_requires_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let path = std::env::temp_dir().join(format!(
            "hiveweb-admin-password-{}",
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
            "hiveweb-admin-password-{}",
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
