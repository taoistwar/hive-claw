//! US2 Red public-boundary and source contract for MySQL safety.
//!
//! T035 owns these tests, T037 approves the observed Red result, and T038
//! implements the real MySQL service boundary. The Foundation gate deliberately
//! does not compile or close these external-datasource contracts.

use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use hivegui::datasource::mysql_client::{
    IdentifierCatalog, IdentifierContext, MysqlClient, MysqlConnectionError, MysqlMetadata,
};

const OWNER_PHASE: &str = "US2";
const TEST_TASK: &str = "T035";
const APPROVAL_TASK: &str = "T037";
const IMPLEMENTATION_TASK: &str = "T038";
const DATASOURCE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/datasource");
const TEST_MYSQL_URL_ENV: &str = "HIVEGUI_TEST_MYSQL_URL";
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);
const CANARY_PASSWORD: &str = "P1aintext-C4n4ry-T035-do-not-rotate-2026-07-30";

#[test]
fn mysql_identifier_accepts_only_exact_server_metadata_and_serializes_by_context() {
    assert_eq!(OWNER_PHASE, "US2");
    assert_eq!(TEST_TASK, "T035");
    assert_eq!(APPROVAL_TASK, "T037");
    assert_eq!(IMPLEMENTATION_TASK, "T038");

    let catalog = IdentifierCatalog::from_server_metadata(MysqlMetadata {
        databases: vec!["FixtureDb".into()],
        tables: vec![("FixtureDb".into(), "AgentRuns".into())],
        columns: vec![("FixtureDb".into(), "AgentRuns".into(), "DisplayName".into())],
    })
    .expect("trusted server metadata builds an exact allowlist");

    let database = catalog
        .database("FixtureDb")
        .expect("exact database metadata match");
    let table = catalog
        .table("FixtureDb", "AgentRuns")
        .expect("exact table metadata match");
    let column = catalog
        .column("FixtureDb", "AgentRuns", "DisplayName")
        .expect("exact column metadata match");

    assert_eq!(database.to_sql(IdentifierContext::Database), "`FixtureDb`");
    assert_eq!(table.to_sql(IdentifierContext::Table), "`AgentRuns`");
    assert_eq!(column.to_sql(IdentifierContext::Column), "`DisplayName`");
    assert_eq!(
        catalog.qualified_table_sql(&database, &table),
        "`FixtureDb`.`AgentRuns`"
    );
}

#[test]
fn mysql_identifier_rejects_unknown_case_changed_dotted_and_malicious_text() {
    let catalog = IdentifierCatalog::from_server_metadata(MysqlMetadata {
        databases: vec!["FixtureDb".into()],
        tables: vec![("FixtureDb".into(), "AgentRuns".into())],
        columns: vec![("FixtureDb".into(), "AgentRuns".into(), "DisplayName".into())],
    })
    .expect("trusted metadata");

    for input in [
        "fixturedb",
        "FIXTUREDB",
        "UnknownDb",
        "FixtureDb.AgentRuns",
        "FixtureDb`",
        "FixtureDb --",
        "FixtureDb/*comment*/",
        "FixtureDb; DROP DATABASE mysql",
        " FixtureDb",
        "FixtureDb ",
        "Fixture\nDb",
        "Fixture\0Db",
    ] {
        assert!(
            catalog.database(input).is_err(),
            "database input {input:?} must not be normalized or escaped"
        );
    }

    for input in [
        "agentruns",
        "UnknownTable",
        "FixtureDb.AgentRuns",
        "AgentRuns` WHERE 1=1 --",
        "AgentRuns\t",
    ] {
        assert!(catalog.table("FixtureDb", input).is_err(), "{input:?}");
    }

    for input in [
        "displayname",
        "UnknownColumn",
        "DisplayName, password",
        "DisplayName DESC",
        "DisplayName` OR 1=1 --",
    ] {
        assert!(
            catalog.column("FixtureDb", "AgentRuns", input).is_err(),
            "{input:?}"
        );
    }
}

#[test]
fn mysql_has_exactly_one_identifier_type_and_no_distributed_escape_helper() {
    let files = rust_files(Path::new(DATASOURCE_ROOT));
    let definitions = occurrences_in_files(&files, "pub struct MysqlIdentifier");
    assert_eq!(
        definitions.len(),
        1,
        "there must be one and only one MysqlIdentifier implementation: {definitions:?}"
    );
    assert!(
        definitions[0].contains("/mysql_client.rs:"),
        "MysqlIdentifier must be owned by mysql_client.rs: {}",
        definitions[0]
    );

    let mysql = read(Path::new(DATASOURCE_ROOT).join("mysql_client.rs"));
    for forbidden in [
        "fn escape(",
        "fn escape_identifier(",
        "fn quote_identifier(",
        "Self::escape(",
        "escape_mysql",
    ] {
        assert!(
            !mysql.contains(forbidden),
            "distributed identifier escape path remains: {forbidden}"
        );
    }
}

#[test]
fn mysql_values_are_bound_and_raw_user_fragments_never_build_sql() {
    let mysql = read(Path::new(DATASOURCE_ROOT).join("mysql_client.rs"));

    for raw_fragment in [
        "where_clause",
        "order_by",
        "push_str(\" ORDER BY ",
        "push_str(&format!",
        "conn.query(&",
        "conn.query_drop(&",
    ] {
        assert!(
            !mysql.contains(raw_fragment),
            "MySQL query construction contains forbidden raw path {raw_fragment:?}"
        );
    }

    for raw_identifier_parameter in [
        "db_name: &str",
        "table_name: &str",
        "column_name: &str",
        "database_name: &str",
    ] {
        assert!(
            !mysql.contains(raw_identifier_parameter),
            "dynamic identifiers must cross MysqlIdentifier, not {raw_identifier_parameter}"
        );
    }

    let interpolated_sql = sql_format_sites(&mysql);
    assert!(
        interpolated_sql.is_empty(),
        "SQL must not be built with format!: {interpolated_sql:?}"
    );
    assert!(
        mysql.contains(".exec(") || mysql.contains(".exec_iter("),
        "dynamic MySQL values must use prepared execution with params"
    );
    assert!(
        mysql.contains("MysqlIdentifier"),
        "database/table/column serialization must use the one identifier type"
    );
}

fn sql_format_sites(source: &str) -> Vec<String> {
    let lines = source.lines().collect::<Vec<_>>();
    let mut sites = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.contains("format!(") {
            continue;
        }
        let end = (index + 25).min(lines.len());
        let window = lines[index..end].join(" ");
        let uppercase = window.to_ascii_uppercase();
        if [
            "SELECT ", "INSERT ", "UPDATE ", "DELETE ", "SHOW ", "ALTER ",
        ]
        .iter()
        .any(|keyword| uppercase.contains(keyword))
        {
            sites.push(format!("line {}: {}", index + 1, line.trim()));
        }
    }
    sites
}

fn occurrences_in_files(files: &[PathBuf], needle: &str) -> Vec<String> {
    let mut occurrences = Vec::new();
    for path in files {
        for (index, line) in read(path).lines().enumerate() {
            if line.contains(needle) {
                occurrences.push(format!("{}:{}", path.display(), index + 1));
            }
        }
    }
    occurrences
}

fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).expect("read datasource source directory") {
            let entry = entry.expect("datasource source entry");
            let path = entry.path();
            if entry.file_type().expect("source entry type").is_dir() {
                pending.push(path);
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path.as_ref())
        .unwrap_or_else(|error| panic!("read {}: {error}", path.as_ref().display()))
}

// ---------------------------------------------------------------------------
// §T035.1 — Real-MySQL connection integration tests.
// ---------------------------------------------------------------------------

/// Parse `HIVEGUI_TEST_MYSQL_URL` (mysql://user:pass@host:port/db) into a
/// `MysqlEndpoint` and reject any value that comes from `.env` or shares
/// credentials with HiveWeb's `TEST_DATABASE_URL`. The test environment
/// must inject this variable through the dedicated HiveGUI CI job.
#[derive(Debug, Clone)]
struct MysqlEndpoint {
    host: String,
    port: u16,
    username: String,
    password: String,
    database: Option<String>,
}

impl MysqlEndpoint {
    fn from_env() -> Option<Self> {
        let url = env::var(TEST_MYSQL_URL_ENV).ok()?;
        let endpoint =
            Self::parse(&url).expect("HIVEGUI_TEST_MYSQL_URL must be a well-formed mysql URL");
        assert!(
            !env::var("TEST_DATABASE_URL").is_ok_and(|existing| existing == url),
            "HIVEGUI_TEST_MYSQL_URL must not alias HiveWeb's TEST_DATABASE_URL"
        );
        Some(endpoint)
    }

    fn parse(url: &str) -> Result<Self, String> {
        let stripped = url
            .strip_prefix("mysql://")
            .ok_or_else(|| "URL must start with mysql://".to_string())?;
        let (authority, database) = match stripped.split_once('/') {
            Some((authority, db)) => (authority, Some(db.to_string())),
            None => (stripped, None),
        };
        let (userinfo, hostport) = authority
            .split_once('@')
            .ok_or_else(|| "URL must contain userinfo@host".to_string())?;
        let (username, password) = userinfo
            .split_once(':')
            .ok_or_else(|| "URL must contain user:password".to_string())?;
        let (host, port) = match hostport.split_once(':') {
            Some((host, port)) => (
                host.to_string(),
                port.parse().map_err(|error| format!("port: {error}"))?,
            ),
            None => (hostport.to_string(), 3306),
        };
        Ok(Self {
            host,
            port,
            username: username.to_string(),
            password: password.to_string(),
            database,
        })
    }
}

fn live_endpoint() -> Option<MysqlEndpoint> {
    MysqlEndpoint::from_env()
}

fn skip_unless_live() -> Option<MysqlEndpoint> {
    match live_endpoint() {
        Some(endpoint) => Some(endpoint),
        None => {
            eprintln!(
                "skipping live MySQL test: {TEST_MYSQL_URL_ENV} not set; \
                 the HiveGUI CI job is the only sanctioned source"
            );
            None
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn live_test_connection_succeeds_within_five_seconds() {
    let Some(endpoint) = skip_unless_live() else {
        return;
    };
    let password = endpoint.password.clone();
    let started = Instant::now();
    let outcome = tokio::time::timeout(
        CONNECTION_TIMEOUT,
        MysqlClient::test_connection(
            &endpoint.host,
            endpoint.port,
            &endpoint.username,
            password.as_bytes(),
        ),
    )
    .await;
    let elapsed = started.elapsed();
    assert!(
        elapsed <= CONNECTION_TIMEOUT + Duration::from_millis(500),
        "live connection must respect the 5s budget; took {elapsed:?}"
    );
    let inner = outcome.expect("test_connection did not exceed the 5s budget");
    inner.expect("HIVEGUI_TEST_MYSQL_URL must reach a healthy 8.0+ service");
}

#[tokio::test(flavor = "current_thread")]
async fn live_test_connection_rejects_wrong_password_and_sanitises_error() {
    let Some(endpoint) = skip_unless_live() else {
        return;
    };
    let started = Instant::now();
    let outcome = MysqlClient::test_connection(
        &endpoint.host,
        endpoint.port,
        &endpoint.username,
        CANARY_PASSWORD.as_bytes(),
    )
    .await;
    let elapsed = started.elapsed();
    let error = outcome.expect_err("wrong password must fail-closed");
    assert!(
        elapsed <= CONNECTION_TIMEOUT + Duration::from_millis(500),
        "auth-failure must fall inside the 5s budget; took {elapsed:?}"
    );
    let rendered = format!("{error:?} {error}");
    assert!(
        !rendered.contains(CANARY_PASSWORD),
        "error must not leak the attempted password; got {rendered}"
    );
    assert!(
        matches!(error, MysqlConnectionError::AuthenticationFailed { .. }),
        "auth failure must surface a structured variant; got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_test_connection_times_out_for_unreachable_endpoint() {
    // 198.51.100.0/24 is reserved for documentation (RFC 5737), so the
    // OS will black-hole the SYN and produce a deterministic timeout.
    let started = Instant::now();
    let outcome = tokio::time::timeout(
        CONNECTION_TIMEOUT,
        MysqlClient::test_connection("198.51.100.7", 3306, "hivegui", b"canary-no-network-needed"),
    )
    .await;
    let elapsed = started.elapsed();
    assert!(
        outcome.is_err(),
        "unreachable endpoint must trigger the 5s timeout"
    );
    assert!(
        elapsed <= CONNECTION_TIMEOUT + Duration::from_millis(500),
        "unreachable endpoint must abort within the 5s budget; took {elapsed:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_test_connection_is_async_cancellable() {
    // The unreachable endpoint also exercises cancellation: the
    // `tokio::select!` must let the test abandon the in-flight attempt
    // without waiting for the 5s budget to elapse.
    let started = Instant::now();
    let cancelled = tokio::select! {
        result = MysqlClient::test_connection(
            "198.51.100.7",
            3306,
            "hivegui",
            b"canary-no-network-needed",
        ) => {
            panic!("unreachable endpoint must not resolve; got {result:?}")
        }
        _ = tokio::time::sleep(Duration::from_millis(100)) => true,
    };
    let elapsed = started.elapsed();
    assert!(cancelled, "test must win the select arm");
    assert!(
        elapsed < Duration::from_millis(500),
        "cancellation must be near-instant; took {elapsed:?}"
    );
}

#[test]
fn no_env_file_is_read_for_mysql_credentials() {
    // The T035 contract forbids reading the repository `.env`; the
    // connection code must take its URL strictly from the env var the
    // CI job sets, never from a dotenv file.
    let repository_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("workspace root");
    let env_path = repository_root.join(".env");
    if env_path.exists() {
        let contents = fs::read_to_string(&env_path).expect("read .env");
        for forbidden in ["HIVEGUI_TEST_MYSQL_URL", "MYSQL_PASSWORD", "MYSQL_USER"] {
            assert!(
                !contents.contains(forbidden),
                "repository .env must not pre-load MySQL credentials ({forbidden})"
            );
        }
    }
    let mysql_client = read(Path::new(DATASOURCE_ROOT).join("mysql_client.rs"));
    for forbidden in [
        "dotenv",
        "load_dotenv",
        "from_dotenv",
        "read_to_string(\".env\")",
        "read_to_string(\".env.local\")",
    ] {
        assert!(
            !mysql_client.contains(forbidden),
            "mysql_client must not consult any dotenv path: {forbidden}"
        );
    }
}
