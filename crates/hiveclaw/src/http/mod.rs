pub mod responses;

use axum::{Router, routing::get, routing::post};
use std::sync::Arc;
use tower_http::trace::TraceLayer;

use crate::agent_backend::AgentBackend;

/// Build the production axum router. `POST /v1/responses` is the
/// OpenResponses contract endpoint (US2); `GET /healthz` returns 200 for
/// liveness checks (US1).
pub fn router(agent: Arc<AgentBackend>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/responses", post(responses::make_handler(agent)))
        .layer(TraceLayer::new_for_http())
}

/// Build a test router using the stub backend (placeholder responses, no LLM).
pub fn router_stub() -> Router {
    router(Arc::new(AgentBackend::stub()))
}

async fn healthz() -> &'static str {
    "ok"
}
