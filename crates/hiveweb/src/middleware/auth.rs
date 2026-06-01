use axum::{
    body::Body,
    extract::Request,
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::utils::error::AppError;
use crate::utils::jwt::{verify_admin_token, verify_user_token};

pub async fn admin_auth_middleware(mut request: Request<Body>, next: Next) -> Response {
    let auth_header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok());

    let token = match auth_header.and_then(|h| h.strip_prefix("Bearer ")) {
        Some(t) => t,
        None => {
            return AppError::TokenInvalid("Missing or malformed Authorization header".to_string())
                .into_response::<()>()
                .into_response();
        }
    };

    let claims = match verify_admin_token(token) {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!("Admin auth failed: {}", e);
            return AppError::TokenInvalid("Admin token invalid or expired".to_string())
                .into_response::<()>()
                .into_response();
        }
    };

    request.extensions_mut().insert(claims);
    next.run(request).await
}

pub async fn user_auth_middleware(mut request: Request<Body>, next: Next) -> Response {
    let auth_header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok());

    let token = match auth_header.and_then(|h| h.strip_prefix("Bearer ")) {
        Some(t) => t,
        None => {
            return AppError::TokenInvalid("Missing or malformed Authorization header".to_string())
                .into_response::<()>()
                .into_response();
        }
    };

    let claims = match verify_user_token(token) {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!("User auth failed: {}", e);
            return AppError::TokenInvalid("User token invalid or expired".to_string())
                .into_response::<()>()
                .into_response();
        }
    };

    request.extensions_mut().insert(claims);
    next.run(request).await
}
