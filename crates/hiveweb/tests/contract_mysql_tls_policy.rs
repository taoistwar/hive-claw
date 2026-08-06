//! Red contract for HiveWeb's production MySQL TLS boundary.
//!
//! `StrictMysqlConnectOptions` is deliberately the only input accepted by
//! `create_pool`: callers cannot pass an unchecked URL to the production pool
//! constructor.  SQLx `VERIFY_IDENTITY` then makes an invalid CA, a hostname
//! mismatch, or unavailable TLS terminal instead of retrying in plaintext.

use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use hiveweb::db::connection::{
    MysqlConnectFailureReason, MysqlPoolTransport, MysqlTlsConfigErrorReason, MysqlTransportError,
    StrictMysqlConnectOptions, create_pool, create_pool_with_transport,
};
use sqlx::{
    ConnectOptions, MySqlPool,
    mysql::{MySqlConnectOptions, MySqlSslMode},
};

const SENSITIVE_SENTINEL: &str = "redaction-sentinel-4f16";
const DATABASE_HOST: &str = "mysql.production.test";

// Public root certificate used only as parseable test data. It is not a key or
// credential and does not identify a real HiveWeb deployment.
const TEST_CA_PEM: &str = r#"-----BEGIN CERTIFICATE-----
MIIDXzCCAkegAwIBAgILBAAAAAABIVhTCKIwDQYJKoZIhvcNAQELBQAwTDEgMB4G
A1UECxMXR2xvYmFsU2lnbiBSb290IENBIC0gUjMxEzARBgNVBAoTCkdsb2JhbFNp
Z24xEzARBgNVBAMTCkdsb2JhbFNpZ24wHhcNMDkwMzE4MTAwMDAwWhcNMjkwMzE4
MTAwMDAwWjBMMSAwHgYDVQQLExdHbG9iYWxTaWduIFJvb3QgQ0EgLSBSMzETMBEG
A1UEChMKR2xvYmFsU2lnbjETMBEGA1UEAxMKR2xvYmFsU2lnbjCCASIwDQYJKoZI
hvcNAQEBBQADggEPADCCAQoCggEBAMwldpB5BngiFvXAg7aEyiie/QV2EcWtiHL8
RgJDx7KKnQRfJMsuS+FggkbhUqsMgUdwbN1k0ev1LKMPgj0MK66X17YUhhB5uzsT
gHeMCOFJ0mpiLx9e+pZo34knlTifBtc+ycsmWQ1z3rDI6SYOgxXG71uL0gRgykmm
KPZpO/bLyCiR5Z2KYVc3rHQU3HTgOu5yLy6c+9C7v/U9AOEGM+iCK65TpjoWc4zd
QQ4gOsC0p6Hpsk+QLjJg6VfLuQSSaGjlOCZgdbKfd/+RFO+uIEn8rUAVSNECMWEZ
XriX7613t2Saer9fwRPvm2L7DWzgVGkWqQPabumDk3F2xmmFghcCAwEAAaNCMEAw
DgYDVR0PAQH/BAQDAgEGMA8GA1UdEwEB/wQFMAMBAf8wHQYDVR0OBBYEFI/wS3+o
LkUkrk1Q+mOai97i3Ru8MA0GCSqGSIb3DQEBCwUAA4IBAQBLQNvAUKr+yAzv95ZU
RUm7lgAJQayzE4aGKAczymvmdLm6AC2upArT9fHxD4q/c2dKg8dEe3jgr25sbwMp
jjM5RcOO5LlXbKr8EpbsU8Yt5CRsuZRj+9xTaGdWPoO4zzUhw8lo/s7awlOqzJCK
6fBdRoyV3XpYKBovHd7NADdBj+1EbddTKJd+82cEHhXXipa0095MJ6RMG3NzdvQX
mcIfeg7jLQitChws/zyrVQ4PkX4268NXSb7hLi18YIvDQVETI53O9zJrlAGomecs
Mx86OyXShkDOOyyGeMlhLxS67ttVb9+E7gUJTb0o2HLO02JQZR7rkpeDMdmztcpH
WD9f
-----END CERTIFICATE-----
"#;

static TEMP_FILE_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

struct TempCa {
    path: PathBuf,
}

impl TempCa {
    fn with_contents(contents: &str) -> Self {
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "hiveweb-mysql-contract-ca-{}-{sequence}.pem",
            std::process::id()
        ));
        fs::write(&path, contents).expect("write isolated MySQL contract CA");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempCa {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[test]
fn strict_options_require_ca_hostname_and_verify_identity() {
    let ca = TempCa::with_contents(TEST_CA_PEM);
    let database_url = database_url(Some("VERIFY_IDENTITY"));
    let strict =
        StrictMysqlConnectOptions::try_from_database_url(&database_url, ca.path(), DATABASE_HOST)
            .expect("strict production MySQL configuration should be accepted");

    let options = strict.as_ref();
    assert_eq!(options.get_ssl_mode(), MySqlSslMode::VerifyIdentity);
    assert_eq!(options.get_host(), DATABASE_HOST);
    assert!(!strict.allows_plaintext_fallback());

    let serialized = options.to_url_lossy();
    assert!(
        serialized.query_pairs().any(|(key, value)| {
            key == "ssl-ca" && value == ca.path().to_string_lossy().as_ref()
        })
    );

    // Compile-time boundary check only: constructing the future performs no
    // network I/O, but proves create_pool cannot receive an unchecked URL.
    let pool_future = create_pool(strict);
    drop(pool_future);
}

#[test]
fn every_weaker_or_implicit_tls_mode_is_rejected_without_credentials() {
    let ca = TempCa::with_contents(TEST_CA_PEM);
    for query in [
        None,
        Some("DISABLED"),
        Some("PREFERRED"),
        Some("REQUIRED"),
        Some("VERIFY_CA"),
    ] {
        let url = database_url(query);
        assert_rejected(
            &url,
            ca.path(),
            DATABASE_HOST,
            MysqlTlsConfigErrorReason::TlsModeNotVerifyIdentity,
        );
    }
}

#[test]
fn missing_bad_ca_and_missing_or_mismatched_hostname_fail_closed() {
    let valid_ca = TempCa::with_contents(TEST_CA_PEM);
    let invalid_ca = TempCa::with_contents("this is not a PEM certificate\n");
    let database_url = database_url(Some("VERIFY_IDENTITY"));
    let absent_ca = std::env::temp_dir().join(format!(
        "hiveweb-mysql-contract-absent-ca-{}.pem",
        std::process::id()
    ));

    assert_rejected(
        &database_url,
        Path::new(""),
        DATABASE_HOST,
        MysqlTlsConfigErrorReason::MissingCa,
    );
    assert_rejected(
        &database_url,
        &absent_ca,
        DATABASE_HOST,
        MysqlTlsConfigErrorReason::InvalidCa,
    );
    assert_rejected(
        &database_url,
        invalid_ca.path(),
        DATABASE_HOST,
        MysqlTlsConfigErrorReason::InvalidCa,
    );
    assert_rejected(
        &database_url,
        valid_ca.path(),
        "",
        MysqlTlsConfigErrorReason::MissingHostname,
    );
    assert_rejected(
        &database_url,
        valid_ca.path(),
        "different.production.test",
        MysqlTlsConfigErrorReason::HostnameMismatch,
    );
}

fn assert_rejected(
    database_url: &str,
    ca_path: &Path,
    expected_hostname: &str,
    expected_reason: MysqlTlsConfigErrorReason,
) {
    let error =
        StrictMysqlConnectOptions::try_from_database_url(database_url, ca_path, expected_hostname)
            .expect_err("insecure production MySQL configuration must be rejected");

    assert_eq!(error.reason(), expected_reason);
    let rendered = format!("{error:?}\n{error}");
    assert!(!rendered.contains(SENSITIVE_SENTINEL));
    assert!(!rendered.contains("mysql://"));
}

#[derive(Clone, Debug, Default)]
struct ProbeCounters {
    attempts: Arc<AtomicUsize>,
    plaintext_attempts: Arc<AtomicUsize>,
}

#[derive(Debug)]
struct SimulatedTlsError {
    reason: MysqlConnectFailureReason,
    sensitive_detail: String,
}

impl fmt::Display for SimulatedTlsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "simulated {:?}: {}",
            self.reason, self.sensitive_detail
        )
    }
}

impl Error for SimulatedTlsError {}

struct FailingMysqlTransport {
    reason: MysqlConnectFailureReason,
    counters: ProbeCounters,
}

#[async_trait]
impl MysqlPoolTransport for FailingMysqlTransport {
    async fn connect(
        &self,
        options: &MySqlConnectOptions,
    ) -> Result<MySqlPool, MysqlTransportError> {
        self.counters.attempts.fetch_add(1, Ordering::SeqCst);
        if matches!(
            options.get_ssl_mode(),
            MySqlSslMode::Disabled | MySqlSslMode::Preferred
        ) {
            self.counters
                .plaintext_attempts
                .fetch_add(1, Ordering::SeqCst);
        }

        Err(MysqlTransportError::new(
            self.reason,
            SimulatedTlsError {
                reason: self.reason,
                sensitive_detail: format!(
                    "mysql connection for {DATABASE_HOST} contained {SENSITIVE_SENTINEL}"
                ),
            },
        ))
    }
}

#[tokio::test]
async fn tls_transport_failures_are_terminal_sanitized_and_never_retry_plaintext() {
    for reason in [
        MysqlConnectFailureReason::TlsUnavailable,
        MysqlConnectFailureReason::TlsHandshakeFailed,
        MysqlConnectFailureReason::CertificateChainRejected,
        MysqlConnectFailureReason::CertificateHostnameMismatch,
    ] {
        let counters = ProbeCounters::default();
        let ca = TempCa::with_contents(TEST_CA_PEM);
        let strict = StrictMysqlConnectOptions::try_from_database_url(
            &database_url(Some("VERIFY_IDENTITY")),
            ca.path(),
            DATABASE_HOST,
        )
        .expect("strict options are required before transport invocation");
        let transport = FailingMysqlTransport {
            reason,
            counters: counters.clone(),
        };

        let error = create_pool_with_transport(strict, &transport)
            .await
            .expect_err("every TLS failure must abort pool creation");

        assert_eq!(error.reason(), reason);
        assert_eq!(
            counters.attempts.load(Ordering::SeqCst),
            1,
            "TLS failure {reason:?} must not be retried"
        );
        assert_eq!(
            counters.plaintext_attempts.load(Ordering::SeqCst),
            0,
            "TLS failure {reason:?} must never trigger a plaintext attempt"
        );
        let rendered = format!("{error:?}\n{error}");
        assert!(!rendered.contains(SENSITIVE_SENTINEL));
        assert!(!rendered.contains("mysql://"));
    }
}

fn database_url(ssl_mode: Option<&str>) -> String {
    let query = ssl_mode
        .map(|mode| format!("?ssl-mode={mode}"))
        .unwrap_or_default();
    format!("mysql://hiveweb:{SENSITIVE_SENTINEL}@{DATABASE_HOST}:3306/hiveweb{query}")
}
