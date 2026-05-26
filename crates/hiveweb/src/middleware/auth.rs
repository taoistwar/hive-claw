use axum::{
    body::Body,
    extract::Request,
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::utils::error::AppError;
use crate::utils::jwt::verify_token;
use crate::utils::jwt::Claims;

pub async fn auth_middleware(mut request: Request<Body>, next: Next) -> Response {
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

    let claims = match verify_token(token) {
        Ok(c) => c,
        Err(_) => {
            return AppError::TokenInvalid("Token invalid or expired".to_string())
                .into_response::<()>()
                .into_response();
        }
    };

    request.extensions_mut().insert(claims);
    next.run(request).await
}

pub fn extract_claims(request: &Request<Body>) -> Option<&Claims> {
    request.extensions().get::<Claims>()
}
