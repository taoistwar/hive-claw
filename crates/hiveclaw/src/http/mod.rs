pub mod responses;

use axum::{routing::get, routing::post, Router};
use tower_http::trace::TraceLayer;

/// Build the production axum router. `POST /v1/responses` is the
/// OpenResponses contract endpoint (US2); `GET /healthz` returns 200 for
/// liveness checks (US1).
pub fn router() -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/responses", post(responses::handle))
        .layer(TraceLayer::new_for_http())
}

async fn healthz() -> &'static str {
    "ok"
}
