//! Fail-closed MySQL connection setup for HiveWeb.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use async_trait::async_trait;
use sqlx::{
    MySqlPool,
    mysql::{MySqlConnectOptions, MySqlPoolOptions, MySqlSslMode},
};

/// Stable category for a rejected MySQL TLS configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MysqlTlsConfigErrorReason {
    /// The connection URL did not request `VERIFY_IDENTITY`.
    TlsModeNotVerifyIdentity,
    /// No CA path was configured.
    MissingCa,
    /// The configured CA was absent, unreadable, or not PEM encoded.
    InvalidCa,
    /// No expected server hostname was configured.
    MissingHostname,
    /// The expected hostname did not exactly match the URL host.
    HostnameMismatch,
}

/// Sanitized MySQL TLS configuration error.
#[derive(Debug)]
pub struct MysqlTlsConfigError {
    reason: MysqlTlsConfigErrorReason,
}

impl MysqlTlsConfigError {
    fn new(reason: MysqlTlsConfigErrorReason) -> Self {
        Self { reason }
    }

    /// Returns the stable rejection category without exposing credentials.
    pub fn reason(&self) -> MysqlTlsConfigErrorReason {
        self.reason
    }
}

impl std::fmt::Display for MysqlTlsConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "mysql tls config rejected: {:?}", self.reason)
    }
}

impl std::error::Error for MysqlTlsConfigError {}

/// Fully validated options accepted by HiveWeb's production pool boundary.
///
/// Construction requires a readable PEM CA, an exact hostname match, and
/// SQLx `VERIFY_IDENTITY`. The inner options cannot be replaced by callers,
/// so production pool creation cannot retry with a weaker TLS mode.
#[derive(Debug, Clone)]
pub struct StrictMysqlConnectOptions {
    options: MySqlConnectOptions,
    ca_path: PathBuf,
}

impl StrictMysqlConnectOptions {
    /// Parses and validates one MySQL URL plus its explicit trust boundary.
    ///
    /// The URL may contain credentials, but errors never retain or render it.
    /// The CA is attached through SQLx's typed `ssl_ca` setter rather than by
    /// concatenating an unescaped query string.
    pub fn try_from_database_url(
        database_url: &str,
        ca_path: &Path,
        expected_hostname: &str,
    ) -> Result<Self, MysqlTlsConfigError> {
        let options = database_url
            .parse::<MySqlConnectOptions>()
            .map_err(|_| MysqlTlsConfigError::new(MysqlTlsConfigErrorReason::InvalidCa))?;

        if expected_hostname.is_empty() {
            return Err(MysqlTlsConfigError::new(
                MysqlTlsConfigErrorReason::MissingHostname,
            ));
        }
        if options.get_host() != expected_hostname {
            return Err(MysqlTlsConfigError::new(
                MysqlTlsConfigErrorReason::HostnameMismatch,
            ));
        }
        if !matches!(options.get_ssl_mode(), MySqlSslMode::VerifyIdentity) {
            return Err(MysqlTlsConfigError::new(
                MysqlTlsConfigErrorReason::TlsModeNotVerifyIdentity,
            ));
        }
        if ca_path.as_os_str().is_empty() {
            return Err(MysqlTlsConfigError::new(
                MysqlTlsConfigErrorReason::MissingCa,
            ));
        }
        if !ca_path.is_file() {
            return Err(MysqlTlsConfigError::new(
                MysqlTlsConfigErrorReason::InvalidCa,
            ));
        }
        let certificate = fs::read_to_string(ca_path)
            .map_err(|_| MysqlTlsConfigError::new(MysqlTlsConfigErrorReason::InvalidCa))?;
        if !certificate.contains("-----BEGIN CERTIFICATE-----")
            || !certificate.contains("-----END CERTIFICATE-----")
        {
            return Err(MysqlTlsConfigError::new(
                MysqlTlsConfigErrorReason::InvalidCa,
            ));
        }

        Ok(Self {
            options: options.ssl_ca(ca_path),
            ca_path: ca_path.to_owned(),
        })
    }

    /// Loads the CA path and expected hostname from explicit environment keys.
    pub fn try_from_environment(
        database_url: &str,
        ca_environment_key: &str,
        hostname_environment_key: &str,
    ) -> Result<Self, MysqlTlsConfigError> {
        let ca_path = std::env::var_os(ca_environment_key)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| MysqlTlsConfigError::new(MysqlTlsConfigErrorReason::MissingCa))?;
        let hostname = std::env::var(hostname_environment_key)
            .ok()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| MysqlTlsConfigError::new(MysqlTlsConfigErrorReason::MissingHostname))?;
        Self::try_from_database_url(database_url, &ca_path, &hostname)
    }

    /// Returns the validated CA path without depending on SQLx URL formatting.
    pub fn ca_path(&self) -> &Path {
        &self.ca_path
    }

    /// Returns whether this value would allow a plaintext fallback.
    pub fn allows_plaintext_fallback(&self) -> bool {
        !matches!(self.options.get_ssl_mode(), MySqlSslMode::VerifyIdentity)
    }
}

impl AsRef<MySqlConnectOptions> for StrictMysqlConnectOptions {
    fn as_ref(&self) -> &MySqlConnectOptions {
        &self.options
    }
}

/// Stable category for a terminal MySQL transport failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MysqlConnectFailureReason {
    /// TLS support was unavailable.
    TlsUnavailable,
    /// The TLS handshake failed.
    TlsHandshakeFailed,
    /// The certificate chain was rejected.
    CertificateChainRejected,
    /// The certificate hostname did not match.
    CertificateHostnameMismatch,
}

/// Sanitized terminal error returned by the strict pool transport.
#[derive(Debug)]
pub struct MysqlTransportError {
    reason: MysqlConnectFailureReason,
}

impl MysqlTransportError {
    /// Creates a sanitized error while deliberately discarding provider text.
    pub fn new(reason: MysqlConnectFailureReason, _source: impl std::fmt::Display) -> Self {
        Self { reason }
    }

    /// Returns the stable failure category.
    pub fn reason(&self) -> MysqlConnectFailureReason {
        self.reason
    }
}

impl std::fmt::Display for MysqlTransportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "mysql transport failed: {:?}", self.reason)
    }
}

impl std::error::Error for MysqlTransportError {}

/// Injectable single-attempt MySQL transport used by the production boundary.
#[async_trait]
pub trait MysqlPoolTransport: Send + Sync {
    /// Connects once using already validated options.
    async fn connect(
        &self,
        options: &MySqlConnectOptions,
    ) -> Result<MySqlPool, MysqlTransportError>;
}

#[derive(Debug, Default)]
struct SqlxMysqlPoolTransport;

#[async_trait]
impl MysqlPoolTransport for SqlxMysqlPoolTransport {
    async fn connect(
        &self,
        options: &MySqlConnectOptions,
    ) -> Result<MySqlPool, MysqlTransportError> {
        MySqlPoolOptions::new()
            .max_connections(20)
            .min_connections(5)
            .acquire_timeout(Duration::from_secs(30))
            .idle_timeout(Duration::from_secs(600))
            .max_lifetime(Duration::from_secs(1800))
            .connect_with(options.clone())
            .await
            .map_err(|error| {
                MysqlTransportError::new(MysqlConnectFailureReason::TlsHandshakeFailed, error)
            })
    }
}

/// Creates the production pool with exactly one strict TLS transport attempt.
pub async fn create_pool(
    options: StrictMysqlConnectOptions,
) -> Result<MySqlPool, MysqlTransportError> {
    create_pool_with_transport(options, &SqlxMysqlPoolTransport).await
}

/// Creates a pool through an injected transport without retry or downgrade.
pub async fn create_pool_with_transport(
    strict: StrictMysqlConnectOptions,
    transport: &impl MysqlPoolTransport,
) -> Result<MySqlPool, MysqlTransportError> {
    transport.connect(strict.as_ref()).await
}
