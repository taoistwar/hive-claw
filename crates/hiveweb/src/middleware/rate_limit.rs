use axum::{body::Body, extract::Request, http::StatusCode, middleware::Next, response::Response};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct RateLimitState {
    records: Arc<Mutex<HashMap<String, RequestRecord>>>,
    limit: u64,
    window: Duration,
}

#[derive(Clone)]
struct RequestRecord {
    count: u64,
    window_start: Instant,
}

impl RateLimitState {
    pub fn new(limit: u64, window: Duration) -> Self {
        Self {
            records: Arc::new(Mutex::new(HashMap::new())),
            limit,
            window,
        }
    }
}

pub async fn rate_limit_middleware(
    state: axum::extract::State<RateLimitState>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let ip = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|connect_info| connect_info.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let now = Instant::now();
    let mut records = state.records.lock().await;

    let record = records.entry(ip.clone()).or_insert(RequestRecord {
        count: 0,
        window_start: now,
    });

    if now.duration_since(record.window_start) > state.window {
        record.count = 0;
        record.window_start = now;
    }

    record.count += 1;

    if record.count > state.limit {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }

    drop(records);

    Ok(next.run(request).await)
}
