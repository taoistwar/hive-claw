use std::{collections::HashMap, fmt, io, sync::Arc, time::Duration};

use anyhow::{Context, bail};
use futures::stream::{FuturesUnordered, StreamExt};
use redis::{
    Client, ConnectionInfo, ErrorKind, IntoConnectionInfo, RedisConnectionInfo, RedisError,
    RedisResult,
    aio::MultiplexedConnection,
    sentinel::{Sentinel, SentinelNodeConnectionInfo},
};
use tokio::{
    sync::{Mutex, RwLock},
    time::{Instant, sleep, timeout},
};

const DEFAULT_REDIS_URL: &str = "redis://127.0.0.1:6379";
const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_SENTINEL_REFRESH_MS: u64 = 1_000;
const SENTINEL_HEDGE_DELAY: Duration = Duration::from_millis(100);
const SENTINEL_REFRESH_TIMEOUT_MAX: Duration = Duration::from_secs(1);

/// Redis discovery mode used by hiveweb.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedisMode {
    Direct,
    Sentinel,
}

impl RedisMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Sentinel => "sentinel",
        }
    }
}

enum RedisConfigKind {
    Direct {
        url: String,
    },
    Sentinel {
        master_name: String,
        sentinel_nodes: Vec<ConnectionInfo>,
        data_node: SentinelNodeConnectionInfo,
    },
}

/// Redis startup configuration.
///
/// This type deliberately uses a redacted `Debug` implementation because both
/// direct URLs and Sentinel connection information may contain credentials.
pub struct RedisConfig {
    kind: RedisConfigKind,
    connect_timeout: Duration,
    sentinel_refresh_interval: Duration,
    database: i64,
}

impl fmt::Debug for RedisConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisConfig")
            .field("mode", &self.mode())
            .field("database", &self.database)
            .field("sentinel_master", &self.sentinel_master())
            .field("sentinel_node_count", &self.sentinel_node_count())
            .field("connect_timeout", &self.connect_timeout)
            .field("sentinel_refresh_interval", &self.sentinel_refresh_interval)
            .finish_non_exhaustive()
    }
}

impl RedisConfig {
    /// Read Redis configuration from the process environment.
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Parse Redis configuration from a key/value source.
    ///
    /// This is also useful for configuration validation in tests and deployment
    /// tooling without mutating the process environment.
    pub fn from_vars<I, K, V>(vars: I) -> anyhow::Result<Self>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let vars = vars
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect::<HashMap<_, _>>();
        Self::from_lookup(|name| vars.get(name).cloned())
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let value = |name| {
            get(name)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };

        let connect_timeout_ms = value("REDIS_CONNECT_TIMEOUT_MS")
            .unwrap_or_else(|| DEFAULT_CONNECT_TIMEOUT_MS.to_string())
            .parse::<u64>()
            .context("REDIS_CONNECT_TIMEOUT_MS must be a positive integer")?;
        if connect_timeout_ms == 0 {
            bail!("REDIS_CONNECT_TIMEOUT_MS must be greater than zero");
        }
        let mode = match value("REDIS_MODE")
            .unwrap_or_else(|| RedisMode::Direct.as_str().to_owned())
            .to_ascii_lowercase()
            .as_str()
        {
            "direct" => RedisMode::Direct,
            "sentinel" => RedisMode::Sentinel,
            other => bail!("REDIS_MODE must be 'direct' or 'sentinel', got '{other}'"),
        };

        let connect_timeout = Duration::from_millis(connect_timeout_ms);
        match mode {
            RedisMode::Direct => {
                let url = value("REDIS_URL").unwrap_or_else(|| DEFAULT_REDIS_URL.to_owned());
                let connection_info = url
                    .as_str()
                    .into_connection_info()
                    .context("REDIS_URL is not a valid Redis connection URL")?;

                Ok(Self {
                    kind: RedisConfigKind::Direct { url },
                    connect_timeout,
                    sentinel_refresh_interval: Duration::from_millis(DEFAULT_SENTINEL_REFRESH_MS),
                    database: connection_info.redis.db,
                })
            }
            RedisMode::Sentinel => {
                let master_name = value("REDIS_SENTINEL_MASTER")
                    .context("REDIS_SENTINEL_MASTER must be set in Sentinel mode")?;
                let node_list = value("REDIS_SENTINEL_NODES")
                    .context("REDIS_SENTINEL_NODES must be set in Sentinel mode")?;

                let database = value("REDIS_DATABASE")
                    .unwrap_or_else(|| "0".to_owned())
                    .parse::<i64>()
                    .context("REDIS_DATABASE must be a non-negative integer")?;
                if database < 0 {
                    bail!("REDIS_DATABASE must be a non-negative integer");
                }
                let sentinel_refresh_ms = value("REDIS_SENTINEL_REFRESH_MS")
                    .unwrap_or_else(|| DEFAULT_SENTINEL_REFRESH_MS.to_string())
                    .parse::<u64>()
                    .context("REDIS_SENTINEL_REFRESH_MS must be a positive integer")?;
                if sentinel_refresh_ms == 0 {
                    bail!("REDIS_SENTINEL_REFRESH_MS must be greater than zero");
                }

                let data_username = value("REDIS_USERNAME");
                let data_password = value("REDIS_PASSWORD");
                let sentinel_username = value("REDIS_SENTINEL_USERNAME");
                let sentinel_password = value("REDIS_SENTINEL_PASSWORD");

                let sentinel_nodes = node_list
                    .split(',')
                    .map(str::trim)
                    .filter(|node| !node.is_empty())
                    .enumerate()
                    .map(|(index, node)| {
                        if node.starts_with("rediss://") {
                            bail!(
                                "REDIS_SENTINEL_NODES entry {} uses TLS, which is not supported",
                                index + 1
                            );
                        }
                        if node.contains("://") && !node.starts_with("redis://") {
                            bail!(
                                "REDIS_SENTINEL_NODES entry {} must use host:port or redis://",
                                index + 1
                            );
                        }
                        let url = if node.contains("://") {
                            node.to_owned()
                        } else {
                            format!("redis://{node}/")
                        };
                        let mut connection_info =
                            url.as_str().into_connection_info().with_context(|| {
                                format!(
                                    "REDIS_SENTINEL_NODES entry {} is not a valid Redis address",
                                    index + 1
                                )
                            })?;
                        connection_info.redis.db = 0;
                        if sentinel_username.is_some() {
                            connection_info.redis.username = sentinel_username.clone();
                        }
                        if sentinel_password.is_some() {
                            connection_info.redis.password = sentinel_password.clone();
                        }
                        Ok(connection_info)
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                if sentinel_nodes.is_empty() {
                    bail!("REDIS_SENTINEL_NODES must contain at least one Sentinel address");
                }

                Ok(Self {
                    kind: RedisConfigKind::Sentinel {
                        master_name,
                        sentinel_nodes,
                        data_node: SentinelNodeConnectionInfo {
                            tls_mode: None,
                            redis_connection_info: Some(RedisConnectionInfo {
                                db: database,
                                username: data_username,
                                password: data_password,
                            }),
                        },
                    },
                    connect_timeout,
                    sentinel_refresh_interval: Duration::from_millis(sentinel_refresh_ms),
                    database,
                })
            }
        }
    }

    pub fn mode(&self) -> RedisMode {
        match self.kind {
            RedisConfigKind::Direct { .. } => RedisMode::Direct,
            RedisConfigKind::Sentinel { .. } => RedisMode::Sentinel,
        }
    }

    pub fn direct_url(&self) -> Option<&str> {
        match &self.kind {
            RedisConfigKind::Direct { url } => Some(url),
            RedisConfigKind::Sentinel { .. } => None,
        }
    }

    pub fn sentinel_master(&self) -> Option<&str> {
        match &self.kind {
            RedisConfigKind::Direct { .. } => None,
            RedisConfigKind::Sentinel { master_name, .. } => Some(master_name),
        }
    }

    pub fn sentinel_node_count(&self) -> usize {
        match &self.kind {
            RedisConfigKind::Direct { .. } => 0,
            RedisConfigKind::Sentinel { sentinel_nodes, .. } => sentinel_nodes.len(),
        }
    }

    pub fn database(&self) -> i64 {
        self.database
    }

    pub fn connect_timeout(&self) -> Duration {
        self.connect_timeout
    }

    pub fn sentinel_refresh_interval(&self) -> Duration {
        self.sentinel_refresh_interval
    }
}

enum RedisClientInner {
    Direct(Client),
    Sentinel(Box<SentinelRedisClient>),
}

struct SentinelResolverConfig {
    master_name: String,
    sentinel_nodes: Vec<ConnectionInfo>,
    data_node: SentinelNodeConnectionInfo,
    hedge_delay: Duration,
}

struct SentinelRedisClient {
    resolver: SentinelResolverConfig,
    current_master: RwLock<Client>,
    refresh: Mutex<SentinelRefreshState>,
    refresh_interval: Duration,
    refresh_timeout: Duration,
}

struct SentinelRefreshState {
    next_refresh: Instant,
    preferred_node: usize,
}

/// Cloneable Redis client that supports both a direct server and Sentinel discovery.
#[derive(Clone)]
pub struct RedisClient {
    inner: Arc<RedisClientInner>,
    connect_timeout: Duration,
}

impl RedisClient {
    fn direct(client: Client, connect_timeout: Duration) -> Self {
        Self {
            inner: Arc::new(RedisClientInner::Direct(client)),
            connect_timeout,
        }
    }

    /// Acquire a connection to the configured data node.
    ///
    /// Sentinel mode uses the last discovered master and refreshes it at the
    /// configured interval. Discovery never serialises other connection callers.
    pub async fn get_multiplexed_async_connection(&self) -> RedisResult<MultiplexedConnection> {
        let acquire = async {
            match self.inner.as_ref() {
                RedisClientInner::Direct(client) => client.get_multiplexed_async_connection().await,
                RedisClientInner::Sentinel(client) => client.get_connection().await,
            }
        };

        timeout(self.connect_timeout, acquire)
            .await
            .map_err(|_| redis_timeout_error("Redis connection timed out"))?
    }
}

impl SentinelRedisClient {
    fn new(
        resolver: SentinelResolverConfig,
        current_master: Client,
        preferred_node: usize,
        refresh_interval: Duration,
        refresh_timeout: Duration,
    ) -> Self {
        Self {
            resolver,
            current_master: RwLock::new(current_master),
            refresh: Mutex::new(SentinelRefreshState {
                next_refresh: Instant::now() + refresh_interval,
                preferred_node,
            }),
            refresh_interval,
            refresh_timeout,
        }
    }

    async fn get_connection(&self) -> RedisResult<MultiplexedConnection> {
        self.refresh_master_if_due().await;
        let master = self.current_master.read().await.clone();
        master.get_multiplexed_async_connection().await
    }

    async fn refresh_master_if_due(&self) {
        let Ok(mut refresh) = self.refresh.try_lock() else {
            // Another request is refreshing. Continue with the last known master
            // instead of serialising normal Redis traffic behind discovery.
            return;
        };
        if Instant::now() < refresh.next_refresh {
            return;
        }

        let resolution = timeout(
            self.refresh_timeout,
            resolve_sentinel_master(&self.resolver, refresh.preferred_node),
        )
        .await
        .map_err(|_| redis_timeout_error("Redis Sentinel master refresh timed out"))
        .and_then(|result| result);

        match resolution {
            Ok((master, preferred_node)) => {
                let mut current_master = self.current_master.write().await;
                let old_address = current_master.get_connection_info().addr.clone();
                let new_address = master.get_connection_info().addr.clone();
                *current_master = master;
                if old_address != new_address {
                    tracing::info!(
                        old_master = %old_address,
                        new_master = %new_address,
                        "Redis Sentinel master changed"
                    );
                }
                refresh.preferred_node = preferred_node;
                refresh.next_refresh = Instant::now() + self.refresh_interval;
            }
            Err(error) => {
                refresh.next_refresh = Instant::now() + self.refresh_interval;
                tracing::warn!(
                    error = %error,
                    retry_after_ms = self.refresh_interval.as_millis(),
                    "Redis Sentinel master refresh failed; using last known master"
                );
            }
        }
    }
}

fn redis_timeout_error(message: &'static str) -> RedisError {
    RedisError::from(io::Error::new(io::ErrorKind::TimedOut, message))
}

async fn resolve_sentinel_master(
    config: &SentinelResolverConfig,
    preferred_node: usize,
) -> RedisResult<(Client, usize)> {
    let node_count = config.sentinel_nodes.len();
    if node_count == 0 {
        return Err(RedisError::from((
            ErrorKind::EmptySentinelList,
            "at least one Redis Sentinel node is required",
        )));
    }

    // Prefer the last Sentinel that answered successfully. Later nodes start
    // after a short hedge delay, so a black-holed preferred node cannot consume
    // the whole discovery budget before another node is tried.
    let start = preferred_node % node_count;
    let mut attempts = FuturesUnordered::new();
    for offset in 0..node_count {
        let node_index = (start + offset) % node_count;
        let node = config.sentinel_nodes[node_index].clone();
        let master_name = config.master_name.clone();
        let data_node = config.data_node.clone();
        let hedge_delay = config.hedge_delay.saturating_mul(offset as u32);
        attempts.push(async move {
            if !hedge_delay.is_zero() {
                sleep(hedge_delay).await;
            }
            let mut sentinel = Sentinel::build(vec![node])?;
            let master = sentinel
                .async_master_for(&master_name, Some(&data_node))
                .await?;
            Ok((master, node_index))
        });
    }

    let mut last_error = None;
    while let Some(result) = attempts.next().await {
        match result {
            Ok(master) => return Ok(master),
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        RedisError::from((
            ErrorKind::EmptySentinelList,
            "at least one Redis Sentinel node is required",
        ))
    }))
}

impl From<Client> for RedisClient {
    fn from(client: Client) -> Self {
        Self::direct(client, Duration::from_millis(DEFAULT_CONNECT_TIMEOUT_MS))
    }
}

/// Create and verify a Redis client from an explicit configuration.
pub async fn create_client(config: RedisConfig) -> anyhow::Result<RedisClient> {
    let mode = config.mode();
    let database = config.database();
    let sentinel_master = config.sentinel_master().map(str::to_owned);
    let sentinel_node_count = config.sentinel_node_count();
    let connect_timeout = config.connect_timeout;
    let sentinel_refresh_interval = config.sentinel_refresh_interval;
    let sentinel_refresh_timeout = connect_timeout
        .checked_div(2)
        .unwrap_or(Duration::ZERO)
        .min(SENTINEL_REFRESH_TIMEOUT_MAX);

    let inner = match config.kind {
        RedisConfigKind::Direct { url } => {
            RedisClientInner::Direct(Client::open(url).context("failed to create Redis client")?)
        }
        RedisConfigKind::Sentinel {
            master_name,
            sentinel_nodes,
            data_node,
        } => {
            let divisor = u32::try_from(sentinel_nodes.len().saturating_add(1)).unwrap_or(u32::MAX);
            let hedge_delay = SENTINEL_HEDGE_DELAY.min(
                sentinel_refresh_timeout
                    .checked_div(divisor)
                    .unwrap_or(Duration::ZERO),
            );
            let resolver = SentinelResolverConfig {
                master_name,
                sentinel_nodes,
                data_node,
                hedge_delay,
            };
            let (current_master, preferred_node) =
                timeout(connect_timeout, resolve_sentinel_master(&resolver, 0))
                    .await
                    .context("Redis Sentinel master discovery timed out")?
                    .context("Redis Sentinel master discovery failed")?;

            RedisClientInner::Sentinel(Box::new(SentinelRedisClient::new(
                resolver,
                current_master,
                preferred_node,
                sentinel_refresh_interval,
                sentinel_refresh_timeout,
            )))
        }
    };
    let client = RedisClient {
        inner: Arc::new(inner),
        connect_timeout,
    };

    timeout(connect_timeout, async {
        let mut connection = client.get_multiplexed_async_connection().await?;
        redis::cmd("PING")
            .query_async::<_, String>(&mut connection)
            .await
            .map(|_| ())
    })
    .await
    .context("Redis startup PING timed out")?
    .context("Redis startup PING failed")?;

    match mode {
        RedisMode::Direct => {
            tracing::info!(
                redis_mode = mode.as_str(),
                database,
                "Redis connection established"
            );
        }
        RedisMode::Sentinel => {
            tracing::info!(
                redis_mode = mode.as_str(),
                database,
                sentinel_master = sentinel_master.as_deref().unwrap_or_default(),
                sentinel_node_count,
                "Redis Sentinel connection established"
            );
        }
    }

    Ok(client)
}

/// Load Redis configuration from the environment and verify the connection.
pub async fn create_from_env() -> anyhow::Result<RedisClient> {
    create_client(RedisConfig::from_env()?).await
}

/// Backward-compatible direct-mode constructor used by existing tests and tools.
pub async fn create_pool(redis_url: &str) -> anyhow::Result<RedisClient> {
    let config = RedisConfig::from_vars([("REDIS_URL", redis_url)])?;
    create_client(config).await
}
