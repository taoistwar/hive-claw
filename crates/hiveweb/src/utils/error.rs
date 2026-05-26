use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

/// Business error codes — must match `specs/003-admin-center/spec.md §Error Codes`
/// and `quickstart.md §Error Codes`. New codes belong in that table first,
/// here second.
pub mod codes {
    // Auth (1001..1004)
    pub const WRONG_PASSWORD: u16 = 1001;
    pub const ACCOUNT_DISABLED: u16 = 1002;
    pub const ACCOUNT_LOCKED: u16 = 1003;
    pub const TOKEN_INVALID: u16 = 1004;

    // Permission
    pub const INSUFFICIENT_PERMISSION: u16 = 2001;

    // Admin domain (3001..3004)
    pub const ADMIN_NOT_FOUND: u16 = 3001;
    pub const PHONE_ALREADY_EXISTS: u16 = 3002;
    pub const CANNOT_DELETE_SUPER_ADMIN: u16 = 3003;
    pub const CANNOT_DISABLE_LAST_SUPER_ADMIN: u16 = 3004;

    // Generic envelopes (used when no spec-level code applies)
    pub const BAD_REQUEST: u16 = 4000;
    pub const NOT_FOUND: u16 = 4040;
    pub const CONFLICT: u16 = 4090;
    pub const INTERNAL: u16 = 5000;
}

#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub code: u16,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            code: 0,
            message: "success".to_string(),
            data: Some(data),
        }
    }

    pub fn err(code: u16, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

impl<T: Serialize> IntoResponse for ApiResponse<T> {
    fn into_response(self) -> Response {
        let status = http_status_for_code(self.code);
        (status, Json(self)).into_response()
    }
}

/// Map every business code to an HTTP status. Keep aligned with spec.md when
/// new codes are added.
pub fn http_status_for_code(code: u16) -> StatusCode {
    match code {
        0 => StatusCode::OK,
        // Auth
        codes::WRONG_PASSWORD | codes::TOKEN_INVALID => StatusCode::UNAUTHORIZED,
        codes::ACCOUNT_DISABLED | codes::ACCOUNT_LOCKED => StatusCode::FORBIDDEN,
        // Permission
        codes::INSUFFICIENT_PERMISSION => StatusCode::FORBIDDEN,
        // Admin domain
        codes::ADMIN_NOT_FOUND => StatusCode::NOT_FOUND,
        codes::PHONE_ALREADY_EXISTS => StatusCode::CONFLICT,
        codes::CANNOT_DELETE_SUPER_ADMIN | codes::CANNOT_DISABLE_LAST_SUPER_ADMIN => {
            StatusCode::FORBIDDEN
        }
        // Generic
        codes::BAD_REQUEST => StatusCode::BAD_REQUEST,
        codes::NOT_FOUND => StatusCode::NOT_FOUND,
        codes::CONFLICT => StatusCode::CONFLICT,
        codes::INTERNAL => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[derive(Debug)]
pub enum AppError {
    // Auth — spec.md §Error Codes 1001..1004
    WrongPassword(String),
    AccountDisabled(String),
    AccountLocked(String),
    TokenInvalid(String),

    // Permission — 2001
    InsufficientPermission(String),

    // Admin — 3001..3004
    AdminNotFound(String),
    PhoneAlreadyExists(String),
    CannotDeleteSuperAdmin(String),
    CannotDisableLastSuperAdmin(String),

    // Generic fallbacks
    BadRequest(String),
    NotFound(String),
    Conflict(String),
    Internal(String),
}

impl AppError {
    pub fn code(&self) -> u16 {
        match self {
            AppError::WrongPassword(_) => codes::WRONG_PASSWORD,
            AppError::AccountDisabled(_) => codes::ACCOUNT_DISABLED,
            AppError::AccountLocked(_) => codes::ACCOUNT_LOCKED,
            AppError::TokenInvalid(_) => codes::TOKEN_INVALID,
            AppError::InsufficientPermission(_) => codes::INSUFFICIENT_PERMISSION,
            AppError::AdminNotFound(_) => codes::ADMIN_NOT_FOUND,
            AppError::PhoneAlreadyExists(_) => codes::PHONE_ALREADY_EXISTS,
            AppError::CannotDeleteSuperAdmin(_) => codes::CANNOT_DELETE_SUPER_ADMIN,
            AppError::CannotDisableLastSuperAdmin(_) => codes::CANNOT_DISABLE_LAST_SUPER_ADMIN,
            AppError::BadRequest(_) => codes::BAD_REQUEST,
            AppError::NotFound(_) => codes::NOT_FOUND,
            AppError::Conflict(_) => codes::CONFLICT,
            AppError::Internal(_) => codes::INTERNAL,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            AppError::WrongPassword(m)
            | AppError::AccountDisabled(m)
            | AppError::AccountLocked(m)
            | AppError::TokenInvalid(m)
            | AppError::InsufficientPermission(m)
            | AppError::AdminNotFound(m)
            | AppError::PhoneAlreadyExists(m)
            | AppError::CannotDeleteSuperAdmin(m)
            | AppError::CannotDisableLastSuperAdmin(m)
            | AppError::BadRequest(m)
            | AppError::NotFound(m)
            | AppError::Conflict(m)
            | AppError::Internal(m) => m,
        }
    }

    pub fn into_response<T: Serialize>(self) -> ApiResponse<T> {
        let code = self.code();
        let msg = self.message().to_string();
        ApiResponse::err(code, msg)
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl From<AppError> for StatusCode {
    fn from(err: AppError) -> Self {
        http_status_for_code(err.code())
    }
}
