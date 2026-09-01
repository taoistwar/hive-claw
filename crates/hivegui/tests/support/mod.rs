//! Shared integration-test infrastructure for HiveGUI's local runtime.
//!
//! The helpers deliberately avoid mutating process-wide environment variables.
//! Tests receive isolated paths and can apply [`TestWorkspace::environment`] to
//! the child process or adapter they own, which keeps parallel test execution
//! deterministic.

pub mod performance;
pub mod scroll_inventory;
pub mod sensitive_canary;

use std::{
    collections::{HashMap, VecDeque},
    ffi::OsString,
    fmt,
    fs::{self, OpenOptions},
    io::{self, ErrorKind, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde_json::Value;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
};

/// Deterministic non-secret key material for fixtures that are not testing
/// randomness. Device-key lifecycle tests must generate their own random key.
pub const FIXTURE_DEVICE_KEY: [u8; 32] = [0x5a; 32];

/// Private paths owned by one test case.
#[derive(Debug)]
pub struct TestWorkspace {
    root: TempDir,
    data_home: PathBuf,
    state_home: PathBuf,
    config_home: PathBuf,
    cache_home: PathBuf,
    database_path: PathBuf,
    device_key_path: PathBuf,
    plugin_root: PathBuf,
    log_root: PathBuf,
}

impl TestWorkspace {
    /// Creates an isolated XDG-shaped directory tree without changing the
    /// environment of the current test process.
    pub fn new() -> io::Result<Self> {
        let root = tempfile::tempdir()?;
        let data_home = root.path().join("data");
        let state_home = root.path().join("state");
        let config_home = root.path().join("config");
        let cache_home = root.path().join("cache");
        let database_path = data_home.join("hivegui").join("hivegui.db");
        let device_key_path = config_home.join("hivegui").join("device.key");
        let plugin_root = data_home.join("hivegui").join("plugins");
        let log_root = state_home.join("hivegui").join("logs");

        for directory in [
            &data_home,
            &state_home,
            &config_home,
            &cache_home,
            database_path.parent().expect("database has parent"),
            &plugin_root,
            &log_root,
        ] {
            fs::create_dir_all(directory)?;
        }

        Ok(Self {
            root,
            data_home,
            state_home,
            config_home,
            cache_home,
            database_path,
            device_key_path,
            plugin_root,
            log_root,
        })
    }

    pub fn root(&self) -> &Path {
        self.root.path()
    }

    pub fn data_home(&self) -> &Path {
        &self.data_home
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub fn device_key_path(&self) -> &Path {
        &self.device_key_path
    }

    pub fn plugin_root(&self) -> &Path {
        &self.plugin_root
    }

    pub fn log_root(&self) -> &Path {
        &self.log_root
    }

    /// Environment values for a child process or injected configuration.
    pub fn environment(&self) -> Vec<(OsString, OsString)> {
        vec![
            ("XDG_DATA_HOME".into(), self.data_home.clone().into()),
            ("XDG_STATE_HOME".into(), self.state_home.clone().into()),
            ("XDG_CONFIG_HOME".into(), self.config_home.clone().into()),
            ("XDG_CACHE_HOME".into(), self.cache_home.clone().into()),
            ("HIVEGUI_LOG_DIR".into(), self.log_root.clone().into()),
        ]
    }

    /// Writes deterministic fixture key material with owner-only permissions.
    /// Existing content must match exactly; this helper never replaces a key.
    pub fn ensure_fixture_device_key(&self) -> io::Result<()> {
        let parent = self
            .device_key_path
            .parent()
            .expect("device key has parent");
        fs::create_dir_all(parent)?;

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }

        match options.open(&self.device_key_path) {
            Ok(mut file) => {
                file.write_all(&FIXTURE_DEVICE_KEY)?;
                file.sync_all()?;
                sync_parent(parent)?;
                Ok(())
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                let existing = fs::read(&self.device_key_path)?;
                if existing == FIXTURE_DEVICE_KEY {
                    Ok(())
                } else {
                    Err(io::Error::new(
                        ErrorKind::InvalidData,
                        "fixture device key already exists with different content",
                    ))
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Opens a one-connection SQLite pool suitable for deterministic tests.
    pub async fn sqlite_pool(&self) -> Result<SqlitePool, sqlx::Error> {
        let options = SqliteConnectOptions::new()
            .filename(&self.database_path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);

        SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .connect_with(options)
            .await
    }
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> io::Result<()> {
    Ok(())
}

/// Stable fault-injection error used by Store and filesystem adapter tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InjectedFault {
    pub point: String,
}

impl fmt::Display for InjectedFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "injected fault at {}", self.point)
    }
}

impl std::error::Error for InjectedFault {}

/// Cloneable countdown fault injector. A point fails exactly the configured
/// number of subsequent checks and then becomes healthy.
#[derive(Debug, Clone, Default)]
pub struct FaultInjector {
    remaining: Arc<Mutex<HashMap<String, usize>>>,
}

impl FaultInjector {
    pub fn fail_next(&self, point: impl Into<String>, count: usize) {
        let mut remaining = self.remaining.lock().expect("fault mutex poisoned");
        remaining.insert(point.into(), count);
    }

    pub fn check(&self, point: &str) -> Result<(), InjectedFault> {
        let mut remaining = self.remaining.lock().expect("fault mutex poisoned");
        let Some(count) = remaining.get_mut(point) else {
            return Ok(());
        };
        if *count == 0 {
            remaining.remove(point);
            return Ok(());
        }
        *count -= 1;
        Err(InjectedFault {
            point: point.to_owned(),
        })
    }
}

/// One deterministic response emitted by [`CapturedHttpServer`].
#[derive(Debug, Clone)]
pub struct MockHttpResponse {
    pub status: u16,
    pub body: Value,
}

impl MockHttpResponse {
    pub fn json(status: u16, body: Value) -> Self {
        Self { status, body }
    }

    fn encode(&self) -> Vec<u8> {
        let body = serde_json::to_vec(&self.body).expect("mock JSON is serializable");
        let reason = match self.status {
            200 => "OK",
            400 => "Bad Request",
            401 => "Unauthorized",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            503 => "Service Unavailable",
            _ => "Mock Response",
        };
        let headers = format!(
            "HTTP/1.1 {} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            self.status,
            reason,
            body.len()
        );
        [headers.as_bytes(), body.as_slice()].concat()
    }
}

/// Loopback HTTP server that captures complete requests and returns queued
/// JSON responses. It can act as both the mock LLM endpoint and the network
/// observation boundary used to prove that no HiveWeb request was attempted.
#[derive(Debug)]
pub struct CapturedHttpServer {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<Vec<u8>>>>,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl CapturedHttpServer {
    pub async fn spawn(responses: Vec<MockHttpResponse>) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let queued = Arc::new(Mutex::new(VecDeque::from(responses)));
        let response_queue = Arc::clone(&queued);
        let (shutdown, mut shutdown_receiver) = oneshot::channel();

        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_receiver => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _peer)) = accepted else { break };
                        let captured = Arc::clone(&captured);
                        let response_queue = Arc::clone(&response_queue);
                        tokio::spawn(async move {
                            let _ = serve_connection(stream, captured, response_queue).await;
                        });
                    }
                }
            }
        });

        Ok(Self {
            address,
            requests,
            shutdown: Some(shutdown),
            task,
        })
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn requests(&self) -> Vec<Vec<u8>> {
        self.requests
            .lock()
            .expect("request mutex poisoned")
            .clone()
    }
}

impl Drop for CapturedHttpServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.task.abort();
    }
}

async fn serve_connection(
    mut stream: TcpStream,
    captured: Arc<Mutex<Vec<Vec<u8>>>>,
    responses: Arc<Mutex<VecDeque<MockHttpResponse>>>,
) -> io::Result<()> {
    let request = read_http_request(&mut stream, 1024 * 1024).await?;
    captured
        .lock()
        .expect("request mutex poisoned")
        .push(request);

    let response = responses
        .lock()
        .expect("response mutex poisoned")
        .pop_front()
        .unwrap_or_else(|| {
            MockHttpResponse::json(
                500,
                serde_json::json!({"error": "mock response queue exhausted"}),
            )
        });
    stream.write_all(&response.encode()).await?;
    stream.shutdown().await
}

async fn read_http_request(stream: &mut TcpStream, limit: usize) -> io::Result<Vec<u8>> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut expected_length = None;

    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        if request.len() > limit {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "captured HTTP request exceeded test limit",
            ));
        }

        if expected_length.is_none() {
            expected_length = total_http_request_length(&request);
        }
        if expected_length.is_some_and(|expected| request.len() >= expected) {
            break;
        }
    }

    Ok(request)
}

fn total_http_request_length(request: &[u8]) -> Option<usize> {
    let header_end = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")?
        + 4;
    let headers = std::str::from_utf8(&request[..header_end]).ok()?;
    let content_length = headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())
            .flatten()
    });
    Some(header_end + content_length.unwrap_or(0))
}
