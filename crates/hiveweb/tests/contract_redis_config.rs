use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use hiveweb::cache::redis::{RedisConfig, RedisMode, create_client};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream, tcp::OwnedReadHalf},
    task::JoinHandle,
    time::Instant,
};

struct FakeRedisServer {
    address: std::net::SocketAddr,
    task: JoinHandle<()>,
}

impl Drop for FakeRedisServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_resp_command(
    reader: &mut BufReader<OwnedReadHalf>,
) -> io::Result<Option<Vec<Vec<u8>>>> {
    let mut line = Vec::new();
    if reader.read_until(b'\n', &mut line).await? == 0 {
        return Ok(None);
    }
    let argument_count = parse_resp_length(&line, b'*')?;
    let mut arguments = Vec::with_capacity(argument_count);
    for _ in 0..argument_count {
        line.clear();
        reader.read_until(b'\n', &mut line).await?;
        let length = parse_resp_length(&line, b'$')?;
        let mut argument = vec![0; length];
        reader.read_exact(&mut argument).await?;
        let mut terminator = [0; 2];
        reader.read_exact(&mut terminator).await?;
        if terminator != *b"\r\n" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "RESP bulk string is missing its CRLF terminator",
            ));
        }
        arguments.push(argument);
    }
    Ok(Some(arguments))
}

fn parse_resp_length(line: &[u8], marker: u8) -> io::Result<usize> {
    let number = line
        .strip_prefix(&[marker])
        .and_then(|line| line.strip_suffix(b"\r\n"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid RESP length"))?;
    std::str::from_utf8(number)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
        .parse()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn command_name(command: &[Vec<u8>]) -> String {
    String::from_utf8_lossy(command.first().map(Vec::as_slice).unwrap_or_default())
        .to_ascii_uppercase()
}

async fn serve_data_node(socket: TcpStream) -> io::Result<()> {
    let (read_half, mut write_half) = socket.into_split();
    let mut reader = BufReader::new(read_half);
    while let Some(command) = read_resp_command(&mut reader).await? {
        let response = match command_name(&command).as_str() {
            "CLIENT" | "SELECT" => b"+OK\r\n".as_slice(),
            "ROLE" => b"*3\r\n$6\r\nmaster\r\n:0\r\n*0\r\n".as_slice(),
            "PING" => b"+PONG\r\n".as_slice(),
            _ => b"-ERR unsupported fake Redis command\r\n".as_slice(),
        };
        write_half.write_all(response).await?;
    }
    Ok(())
}

async fn spawn_data_node() -> FakeRedisServer {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("fake Redis data node should bind");
    let address = listener
        .local_addr()
        .expect("fake Redis data node should have an address");
    let task = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                serve_data_node(socket)
                    .await
                    .expect("fake Redis data node should serve RESP commands");
            });
        }
    });
    FakeRedisServer { address, task }
}

fn append_bulk_string(response: &mut Vec<u8>, value: &str) {
    response.extend_from_slice(format!("${}\r\n", value.len()).as_bytes());
    response.extend_from_slice(value.as_bytes());
    response.extend_from_slice(b"\r\n");
}

fn sentinel_masters_response(data_address: std::net::SocketAddr) -> Vec<u8> {
    let fields = [
        ("name", "mymaster".to_owned()),
        ("ip", data_address.ip().to_string()),
        ("port", data_address.port().to_string()),
        ("flags", "master".to_owned()),
    ];
    let mut response = format!("*1\r\n*{}\r\n", fields.len() * 2).into_bytes();
    for (key, value) in fields {
        append_bulk_string(&mut response, key);
        append_bulk_string(&mut response, &value);
    }
    response
}

async fn serve_sentinel(
    socket: TcpStream,
    masters_response: Arc<Vec<u8>>,
    masters_requests: Arc<AtomicUsize>,
) -> io::Result<()> {
    let (read_half, mut write_half) = socket.into_split();
    let mut reader = BufReader::new(read_half);
    while let Some(command) = read_resp_command(&mut reader).await? {
        match command_name(&command).as_str() {
            "CLIENT" => write_half.write_all(b"+OK\r\n").await?,
            "SENTINEL" if command.get(1).map(Vec::as_slice) == Some(b"MASTERS") => {
                let request_number = masters_requests.fetch_add(1, Ordering::SeqCst);
                if request_number == 0 {
                    write_half.write_all(&masters_response).await?;
                } else {
                    std::future::pending::<()>().await;
                }
            }
            _ => {
                write_half
                    .write_all(b"-ERR unsupported fake Sentinel command\r\n")
                    .await?
            }
        }
    }
    Ok(())
}

async fn spawn_sentinel(data_address: std::net::SocketAddr) -> (FakeRedisServer, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("fake Sentinel should bind");
    let address = listener
        .local_addr()
        .expect("fake Sentinel should have an address");
    let masters_response = Arc::new(sentinel_masters_response(data_address));
    let masters_requests = Arc::new(AtomicUsize::new(0));
    let requests = Arc::clone(&masters_requests);
    let task = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let masters_response = Arc::clone(&masters_response);
            let masters_requests = Arc::clone(&requests);
            tokio::spawn(async move {
                serve_sentinel(socket, masters_response, masters_requests)
                    .await
                    .expect("fake Sentinel should serve RESP commands");
            });
        }
    });
    (FakeRedisServer { address, task }, masters_requests)
}

#[test]
fn direct_mode_keeps_redis_url_backward_compatible() {
    let config = RedisConfig::from_vars([
        ("REDIS_URL", "redis://:secret@127.0.0.1:6379/2"),
        ("REDIS_CONNECT_TIMEOUT_MS", "2500"),
        ("REDIS_SENTINEL_REFRESH_MS", "ignored-in-direct-mode"),
    ])
    .expect("direct Redis configuration should parse");

    assert_eq!(config.mode(), RedisMode::Direct);
    assert_eq!(
        config.direct_url(),
        Some("redis://:secret@127.0.0.1:6379/2")
    );
    assert_eq!(config.connect_timeout(), Duration::from_millis(2500));
    assert!(!format!("{config:?}").contains("secret"));
}

#[test]
fn unified_client_is_clone_send_and_sync() {
    fn assert_traits<T: Clone + Send + Sync>() {}
    assert_traits::<hiveweb::cache::redis::RedisClient>();
}

#[test]
fn sentinel_mode_parses_master_nodes_and_data_database() {
    let config = RedisConfig::from_vars([
        ("REDIS_MODE", "sentinel"),
        ("REDIS_URL", "ignored-in-sentinel-mode"),
        ("REDIS_SENTINEL_MASTER", "mymaster"),
        (
            "REDIS_SENTINEL_NODES",
            "10.0.0.1:26379, 10.0.0.2:26379,10.0.0.3:26379",
        ),
        ("REDIS_DATABASE", "15"),
        ("REDIS_USERNAME", "data-user"),
        ("REDIS_PASSWORD", "data-password"),
        ("REDIS_SENTINEL_USERNAME", "sentinel-user"),
        ("REDIS_SENTINEL_PASSWORD", "sentinel-password"),
        ("REDIS_SENTINEL_REFRESH_MS", "1500"),
    ])
    .expect("Sentinel Redis configuration should parse");

    assert_eq!(config.mode(), RedisMode::Sentinel);
    assert_eq!(config.sentinel_master(), Some("mymaster"));
    assert_eq!(config.sentinel_node_count(), 3);
    assert_eq!(config.database(), 15);
    assert_eq!(config.connect_timeout(), Duration::from_secs(5));
    assert_eq!(
        config.sentinel_refresh_interval(),
        Duration::from_millis(1500)
    );

    let debug = format!("{config:?}");
    assert!(!debug.contains("data-password"));
    assert!(!debug.contains("sentinel-password"));
}

#[test]
fn sentinel_mode_requires_master_and_nodes() {
    let missing_master = RedisConfig::from_vars([
        ("REDIS_MODE", "sentinel"),
        ("REDIS_SENTINEL_NODES", "127.0.0.1:26379"),
    ])
    .expect_err("master name is required");
    assert!(missing_master.to_string().contains("REDIS_SENTINEL_MASTER"));

    let missing_nodes = RedisConfig::from_vars([
        ("REDIS_MODE", "sentinel"),
        ("REDIS_SENTINEL_MASTER", "mymaster"),
    ])
    .expect_err("Sentinel node list is required");
    assert!(missing_nodes.to_string().contains("REDIS_SENTINEL_NODES"));

    let minimal = RedisConfig::from_vars([
        ("REDIS_MODE", "sentinel"),
        ("REDIS_SENTINEL_MASTER", "mymaster"),
        ("REDIS_SENTINEL_NODES", "127.0.0.1:26379"),
    ])
    .expect("minimal Sentinel configuration should use defaults");
    assert_eq!(minimal.sentinel_refresh_interval(), Duration::from_secs(1));
}

#[test]
fn invalid_mode_database_and_timeout_are_rejected() {
    let invalid_mode = RedisConfig::from_vars([("REDIS_MODE", "cluster")])
        .expect_err("unsupported Redis mode must fail");
    assert!(invalid_mode.to_string().contains("REDIS_MODE"));

    let invalid_database = RedisConfig::from_vars([
        ("REDIS_MODE", "sentinel"),
        ("REDIS_SENTINEL_MASTER", "mymaster"),
        ("REDIS_SENTINEL_NODES", "127.0.0.1:26379"),
        ("REDIS_DATABASE", "not-a-number"),
    ])
    .expect_err("invalid database must fail");
    assert!(invalid_database.to_string().contains("REDIS_DATABASE"));

    let zero_timeout = RedisConfig::from_vars([("REDIS_CONNECT_TIMEOUT_MS", "0")])
        .expect_err("zero timeout must fail");
    assert!(
        zero_timeout
            .to_string()
            .contains("REDIS_CONNECT_TIMEOUT_MS")
    );

    let zero_refresh = RedisConfig::from_vars([
        ("REDIS_MODE", "sentinel"),
        ("REDIS_SENTINEL_MASTER", "mymaster"),
        ("REDIS_SENTINEL_NODES", "127.0.0.1:26379"),
        ("REDIS_SENTINEL_REFRESH_MS", "0"),
    ])
    .expect_err("zero Sentinel refresh interval must fail");
    assert!(
        zero_refresh
            .to_string()
            .contains("REDIS_SENTINEL_REFRESH_MS")
    );

    let unsupported_tls = RedisConfig::from_vars([
        ("REDIS_MODE", "sentinel"),
        ("REDIS_SENTINEL_MASTER", "mymaster"),
        ("REDIS_SENTINEL_NODES", "rediss://127.0.0.1:26379"),
    ])
    .expect_err("Sentinel TLS must not silently downgrade data-node connections");
    assert!(unsupported_tls.to_string().contains("TLS"));
}

#[tokio::test]
async fn sentinel_refresh_timeout_uses_cached_master_before_connect_timeout() {
    let data_node = spawn_data_node().await;
    let (sentinel, masters_requests) = spawn_sentinel(data_node.address).await;
    let connect_timeout = Duration::from_millis(2_400);
    let config = RedisConfig::from_vars([
        ("REDIS_MODE", "sentinel".to_owned()),
        ("REDIS_SENTINEL_MASTER", "mymaster".to_owned()),
        ("REDIS_SENTINEL_NODES", sentinel.address.to_string()),
        ("REDIS_DATABASE", "0".to_owned()),
        (
            "REDIS_CONNECT_TIMEOUT_MS",
            connect_timeout.as_millis().to_string(),
        ),
        ("REDIS_SENTINEL_REFRESH_MS", "200".to_owned()),
    ])
    .expect("fake Sentinel configuration should parse");

    let client = create_client(config)
        .await
        .expect("initial Sentinel discovery and data-node PING should succeed");
    assert_eq!(masters_requests.load(Ordering::SeqCst), 1);

    tokio::time::sleep(Duration::from_millis(250)).await;
    let started = Instant::now();
    let pong = tokio::time::timeout(Duration::from_millis(1_700), async {
        let mut connection = client.get_multiplexed_async_connection().await?;
        redis::cmd("PING")
            .query_async::<_, String>(&mut connection)
            .await
    })
    .await
    .expect("cached-master fallback should finish well before the connection timeout")
    .expect("cached master should remain usable when Sentinel does not respond");
    let elapsed = started.elapsed();

    assert_eq!(pong, "PONG");
    assert!(
        elapsed >= Duration::from_millis(900),
        "the fake Sentinel did not exercise the internal refresh timeout: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(1_700),
        "refresh consumed too much of the {connect_timeout:?} connection budget: {elapsed:?}"
    );
    assert_eq!(masters_requests.load(Ordering::SeqCst), 2);
}
