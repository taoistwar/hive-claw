//! New Session API — 创建新会话接口（MD5 签名鉴权）
//!
//! POST /api/newsession?sign={md5}
//!
//! 请求头：
//!   Content-Type: application/json; charset=UTF-8
//!
//! 请求体：
//!   { "user_id": 123 }
//!
//! 响应：
//!   { "success": true }

use axum::{
    Router,
    extract::{Query, State},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Deserialize;
use std::collections::HashMap;

use crate::api::AppState;
use crate::api::chat_common;
use crate::services::membership;
use crate::services::user_auth;
use crate::services::chat_user as svc;
use crate::utils::error::AppError;

pub fn router() -> Router<AppState> {
    Router::new().route("/newsession", post(create_new_session))
}

#[derive(Debug, Deserialize)]
pub struct NewSessionRequest {
    pub user_id: i64,
}

async fn create_new_session(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    body: String,
) -> Response {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if content_type != "application/json; charset=UTF-8" {
        return AppError::BadRequest(
            "Invalid Content-Type, must be: application/json; charset=UTF-8".into(),
        )
        .into_response::<()>()
        .into_response();
    }

    let secret = chat_common::get_assistant_secret();
    if !secret.is_empty() {
        let sign = params.get("sign").map(|s| s.as_str()).unwrap_or("");
        if !chat_common::verify_sign(secret, "/api/newsession", &body, sign) {
            return AppError::BadRequest("Invalid signature".into())
                .into_response::<()>()
                .into_response();
        }
    }

    let req: NewSessionRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(_) => {
            return AppError::BadRequest("Invalid request body".into())
                .into_response::<()>()
                .into_response();
        }
    };

    if req.user_id <= 0 {
        return AppError::BadRequest("user_id must be positive".into())
            .into_response::<()>()
            .into_response();
    }

    let ext_pool = match &state.ext_pool {
        Some(p) => p,
        None => {
            return AppError::Internal("New session service unavailable".into())
                .into_response::<()>()
                .into_response();
        }
    };

    match membership::user_exists_in_cloud_cached(&state.redis, ext_pool, req.user_id).await {
        Ok(true) => {}
        Ok(false) => {
            return AppError::BadRequest("User not found".into())
                .into_response::<()>()
                .into_response();
        }
        Err(e) => {
            tracing::error!(user_id = req.user_id, error = %e, "membership::user_exists_in_cloud_cached 查询失败");
            return AppError::Internal("用户数据查询失败，请稍后重试".into())
                .into_response::<()>()
                .into_response();
        }
    }

    let cloud_info = membership::get_cloud_user_info_cached(&state.redis, ext_pool, req.user_id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(user_id = req.user_id, error = %e, "get_cloud_user_info_cached failed");
            None
        });
    let (uid, nickname) = cloud_info
        .as_ref()
        .map(|(u, n)| (Some(u.as_str()), Some(n.as_str())))
        .unwrap_or((None, None));

    if let Err(e) = user_auth::ensure_user_exists(&state.pool, req.user_id, uid, nickname).await {
        return AppError::Internal(format!("user sync: {e}"))
            .into_response::<()>()
            .into_response();
    }

    let session = match svc::create_user_session(&state.pool, req.user_id, None).await {
        Ok(s) => s,
        Err(e) => {
            return e.into_response::<()>().into_response();
        }
    };

    tracing::info!(user_id = req.user_id, session_id = session.id, "New session created");

    axum::Json(serde_json::json!({ "success": true })).into_response()
}