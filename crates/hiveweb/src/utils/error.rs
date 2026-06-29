use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

/// Business error codes — must match `specs/003-admin-center/spec.md §Error Codes`
/// and `quickstart.md §Error Codes`. New codes belong in that table first,
/// here second.
pub mod codes {
    // Auth (1001..1005)
    pub const WRONG_PASSWORD: u16 = 1001;
    pub const ACCOUNT_DISABLED: u16 = 1002;
    pub const ACCOUNT_LOCKED: u16 = 1003;
    pub const TOKEN_INVALID: u16 = 1004;
    pub const NOT_ADMINISTRATOR: u16 = 1005;

    // Permission
    pub const INSUFFICIENT_PERMISSION: u16 = 2001;

    // Admin domain (3001..3004)
    pub const ADMIN_NOT_FOUND: u16 = 3001;
    pub const PHONE_ALREADY_EXISTS: u16 = 3002;
    pub const CANNOT_DELETE_SUPER_ADMIN: u16 = 3003;
    pub const CANNOT_DISABLE_LAST_SUPER_ADMIN: u16 = 3004;
    pub const NEW_PASSWORD_SAME_AS_OLD: u16 = 3008;

    // Generic envelopes (used when no spec-level code applies)
    pub const BAD_REQUEST: u16 = 4000;
    pub const NOT_FOUND: u16 = 4040;
    pub const CONFLICT: u16 = 4090;
    pub const INTERNAL: u16 = 5000;

    // ----- 006 Game Alias Management -----
    pub const GAME_ALIAS_NOT_FOUND: u16 = 4001;
    pub const GAME_NAME_ALREADY_EXISTS: u16 = 4002;
    pub const GAME_NAME_EMPTY: u16 = 4003;
    pub const GAME_NAME_TOO_LONG: u16 = 4004;
    pub const ALIASES_EMPTY: u16 = 4005;
    pub const ALIAS_TOO_LONG: u16 = 4006;
    pub const ALIASES_TOO_MANY: u16 = 4007;
    pub const ALIAS_ALREADY_IN_USE: u16 = 4008;

    // ----- 004 Agent Runtime（contracts/api.md §Errors） -----
    pub const CAPABILITY_DENIED_RUNTIME: u16 = 4030;
    /// 实现期发现 003 NOT_FOUND=4040 已占用，004 contracts/api.md 原写 4040；
    /// 改用 4045 以避免 u16 业务码冲突。contracts/api.md 同步更新。
    pub const CAPABILITY_UNKNOWN: u16 = 4045;
    pub const TAG_IN_USE: u16 = 4091;
    pub const DAG_CYCLE: u16 = 4092;
    pub const RESOURCE_IN_USE: u16 = 4093;
    pub const OPTIMISTIC_LOCK_CONFLICT: u16 = 4094;
    pub const SSE_CONCURRENCY_EXCEEDED: u16 = 4291;

    /// AI 助手日访问次数超限
    pub const DAILY_LIMIT_REACHED: u16 = 4290;

    /// 敏感词过滤拦截
    pub const SENSITIVE_WORD_BLOCKED: u16 = 4009;
    pub const CANNOT_DELETE_MAIN_AGENT: u16 = 5001;
    pub const SCHEMA_MISMATCH: u16 = 5002;
    pub const CAPABILITY_DENIED_CHAT: u16 = 5003;
    pub const PLUGIN_INVOCATION_TIMEOUT: u16 = 5004;
    pub const WORKFLOW_MAPPING_INVALID: u16 = 5005;
    pub const AGENT_DEPTH_EXCEEDED: u16 = 5006;
    pub const MODEL_PRESET_UNKNOWN: u16 = 5007;
    pub const BUILTIN_SKILL_PROTECTED: u16 = 5008;
    pub const POOL_BUSY: u16 = 5009;
    pub const BUILTIN_TOOL_PROTECTED: u16 = 5010;

    /// 011 Plugin System Toggle — 插件系统已被环境变量 `PLUGIN_SYSTEM_ENABLED=false` 关闭
    pub const PLUGIN_SYSTEM_DISABLED: u16 = 5031;

    // ----- 008 Agent Hook 配置管理（6001-6010） -----
    pub const HOOK_TRIGGER_LIMIT_EXCEEDED: u16 = 6001;
    pub const HOOK_REFERENCE_INVALID: u16 = 6002;
    pub const HOOK_WEBHOOK_URL_INVALID: u16 = 6003;
    pub const HOOK_EXECUTION_TIMEOUT: u16 = 6004;
    pub const HOOK_BLOCKING_FAILED: u16 = 6005;
    pub const HOOK_NOT_FOUND: u16 = 6006;
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
        codes::ACCOUNT_DISABLED | codes::ACCOUNT_LOCKED | codes::NOT_ADMINISTRATOR => {
            StatusCode::FORBIDDEN
        }
        // Permission
        codes::INSUFFICIENT_PERMISSION => StatusCode::FORBIDDEN,
        // Admin domain
        codes::ADMIN_NOT_FOUND => StatusCode::NOT_FOUND,
        codes::PHONE_ALREADY_EXISTS => StatusCode::CONFLICT,
        codes::CANNOT_DELETE_SUPER_ADMIN | codes::CANNOT_DISABLE_LAST_SUPER_ADMIN => {
            StatusCode::FORBIDDEN
        }
        codes::NEW_PASSWORD_SAME_AS_OLD => StatusCode::BAD_REQUEST,
        // Generic
        codes::BAD_REQUEST | codes::SENSITIVE_WORD_BLOCKED => StatusCode::BAD_REQUEST,
        codes::NOT_FOUND => StatusCode::NOT_FOUND,
        codes::CONFLICT => StatusCode::CONFLICT,
        codes::INTERNAL => StatusCode::INTERNAL_SERVER_ERROR,
        // 006 — Game Alias Management HTTP 映射
        codes::GAME_ALIAS_NOT_FOUND => StatusCode::NOT_FOUND,
        codes::GAME_NAME_EMPTY
        | codes::GAME_NAME_TOO_LONG
        | codes::ALIASES_EMPTY
        | codes::ALIAS_TOO_LONG
        | codes::ALIASES_TOO_MANY => StatusCode::BAD_REQUEST,
        codes::GAME_NAME_ALREADY_EXISTS | codes::ALIAS_ALREADY_IN_USE => StatusCode::CONFLICT,
        // 004 — contracts/api.md §Errors HTTP 映射
        codes::CAPABILITY_DENIED_RUNTIME => StatusCode::FORBIDDEN,
        codes::TAG_IN_USE
        | codes::DAG_CYCLE
        | codes::RESOURCE_IN_USE
        | codes::OPTIMISTIC_LOCK_CONFLICT => StatusCode::CONFLICT,
        codes::SSE_CONCURRENCY_EXCEEDED | codes::DAILY_LIMIT_REACHED => StatusCode::TOO_MANY_REQUESTS,
        codes::CANNOT_DELETE_MAIN_AGENT
        | codes::CAPABILITY_DENIED_CHAT
        | codes::BUILTIN_SKILL_PROTECTED
        | codes::BUILTIN_TOOL_PROTECTED => StatusCode::FORBIDDEN,
        codes::SCHEMA_MISMATCH
        | codes::WORKFLOW_MAPPING_INVALID
        | codes::AGENT_DEPTH_EXCEEDED
        | codes::MODEL_PRESET_UNKNOWN => StatusCode::UNPROCESSABLE_ENTITY,
        codes::PLUGIN_INVOCATION_TIMEOUT => StatusCode::REQUEST_TIMEOUT,
        codes::POOL_BUSY | codes::PLUGIN_SYSTEM_DISABLED => StatusCode::SERVICE_UNAVAILABLE,
        // 008 — Agent Hook HTTP 映射
        codes::HOOK_TRIGGER_LIMIT_EXCEEDED | codes::HOOK_REFERENCE_INVALID => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        codes::HOOK_WEBHOOK_URL_INVALID => StatusCode::BAD_REQUEST,
        codes::HOOK_EXECUTION_TIMEOUT => StatusCode::REQUEST_TIMEOUT,
        codes::HOOK_BLOCKING_FAILED => StatusCode::INTERNAL_SERVER_ERROR,
        codes::HOOK_NOT_FOUND => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[derive(Debug)]
pub enum AppError {
    // Auth — spec.md §Error Codes 1001..1005
    WrongPassword(String),
    AccountDisabled(String),
    AccountLocked(String),
    TokenInvalid(String),
    NotAdministrator(String),

    // Permission — 2001
    InsufficientPermission(String),

    // Admin — 3001..3004
    AdminNotFound(String),
    PhoneAlreadyExists(String),
    CannotDeleteSuperAdmin(String),
    CannotDisableLastSuperAdmin(String),
    NewPasswordSameAsOld(String),

    // Generic fallbacks
    BadRequest(String),
    NotFound(String),
    Conflict(String),
    Internal(String),

    // ----- 006 Game Alias Management -----
    GameAliasNotFound(String),
    GameNameAlreadyExists(String),
    GameNameEmpty(String),
    GameNameTooLong(String),
    AliasesEmpty(String),
    AliasTooLong(String),
    AliasesTooMany(String),
    AliasAlreadyInUse(String),

    // ----- 004 Agent Runtime（contracts/api.md §Errors） -----
    CapabilityDeniedRuntime(String),
    CapabilityUnknown(String),
    TagInUse(String),
    DagCycle(String),
    ResourceInUse(String),
    OptimisticLockConflict(String),
    SseConcurrencyExceeded(String),
    DailyLimitReached(String),
    SensitiveWordBlocked(String),
    CannotDeleteMainAgent(String),
    SchemaMismatch(String),
    CapabilityDeniedChat(String),
    PluginInvocationTimeout(String),
    WorkflowMappingInvalid(String),
    AgentDepthExceeded(String),
    ModelPresetUnknown(String),
    BuiltinSkillProtected(String),
    PoolBusy(String),
    BuiltinToolProtected(String),
    PluginSystemDisabled(String),

    // ----- 008 Agent Hook（6001-6006） -----
    HookTriggerLimitExceeded(String),
    HookReferenceInvalid(String),
    HookWebhookUrlInvalid(String),
    HookExecutionTimeout(String),
    HookBlockingFailed(String),
    HookNotFound(String),
}

impl AppError {
    pub fn code(&self) -> u16 {
        match self {
            AppError::WrongPassword(_) => codes::WRONG_PASSWORD,
            AppError::AccountDisabled(_) => codes::ACCOUNT_DISABLED,
            AppError::AccountLocked(_) => codes::ACCOUNT_LOCKED,
            AppError::TokenInvalid(_) => codes::TOKEN_INVALID,
            AppError::NotAdministrator(_) => codes::NOT_ADMINISTRATOR,
            AppError::InsufficientPermission(_) => codes::INSUFFICIENT_PERMISSION,
            AppError::AdminNotFound(_) => codes::ADMIN_NOT_FOUND,
            AppError::PhoneAlreadyExists(_) => codes::PHONE_ALREADY_EXISTS,
            AppError::CannotDeleteSuperAdmin(_) => codes::CANNOT_DELETE_SUPER_ADMIN,
            AppError::CannotDisableLastSuperAdmin(_) => codes::CANNOT_DISABLE_LAST_SUPER_ADMIN,
            AppError::NewPasswordSameAsOld(_) => codes::NEW_PASSWORD_SAME_AS_OLD,
            AppError::BadRequest(_) => codes::BAD_REQUEST,
            AppError::NotFound(_) => codes::NOT_FOUND,
            AppError::Conflict(_) => codes::CONFLICT,
            AppError::Internal(_) => codes::INTERNAL,
            // 006 Game Alias Management
            AppError::GameAliasNotFound(_) => codes::GAME_ALIAS_NOT_FOUND,
            AppError::GameNameAlreadyExists(_) => codes::GAME_NAME_ALREADY_EXISTS,
            AppError::GameNameEmpty(_) => codes::GAME_NAME_EMPTY,
            AppError::GameNameTooLong(_) => codes::GAME_NAME_TOO_LONG,
            AppError::AliasesEmpty(_) => codes::ALIASES_EMPTY,
            AppError::AliasTooLong(_) => codes::ALIAS_TOO_LONG,
            AppError::AliasesTooMany(_) => codes::ALIASES_TOO_MANY,
            AppError::AliasAlreadyInUse(_) => codes::ALIAS_ALREADY_IN_USE,
            // 004 Agent Runtime
            AppError::CapabilityDeniedRuntime(_) => codes::CAPABILITY_DENIED_RUNTIME,
            AppError::CapabilityUnknown(_) => codes::CAPABILITY_UNKNOWN,
            AppError::TagInUse(_) => codes::TAG_IN_USE,
            AppError::DagCycle(_) => codes::DAG_CYCLE,
            AppError::ResourceInUse(_) => codes::RESOURCE_IN_USE,
            AppError::OptimisticLockConflict(_) => codes::OPTIMISTIC_LOCK_CONFLICT,
            AppError::SseConcurrencyExceeded(_) => codes::SSE_CONCURRENCY_EXCEEDED,
            AppError::DailyLimitReached(_) => codes::DAILY_LIMIT_REACHED,
            AppError::SensitiveWordBlocked(_) => codes::SENSITIVE_WORD_BLOCKED,
            AppError::CannotDeleteMainAgent(_) => codes::CANNOT_DELETE_MAIN_AGENT,
            AppError::SchemaMismatch(_) => codes::SCHEMA_MISMATCH,
            AppError::CapabilityDeniedChat(_) => codes::CAPABILITY_DENIED_CHAT,
            AppError::PluginInvocationTimeout(_) => codes::PLUGIN_INVOCATION_TIMEOUT,
            AppError::WorkflowMappingInvalid(_) => codes::WORKFLOW_MAPPING_INVALID,
            AppError::AgentDepthExceeded(_) => codes::AGENT_DEPTH_EXCEEDED,
            AppError::ModelPresetUnknown(_) => codes::MODEL_PRESET_UNKNOWN,
            AppError::BuiltinSkillProtected(_) => codes::BUILTIN_SKILL_PROTECTED,
            AppError::PoolBusy(_) => codes::POOL_BUSY,
            AppError::BuiltinToolProtected(_) => codes::BUILTIN_TOOL_PROTECTED,
            AppError::PluginSystemDisabled(_) => codes::PLUGIN_SYSTEM_DISABLED,
            // 008 Agent Hook
            AppError::HookTriggerLimitExceeded(_) => codes::HOOK_TRIGGER_LIMIT_EXCEEDED,
            AppError::HookReferenceInvalid(_) => codes::HOOK_REFERENCE_INVALID,
            AppError::HookWebhookUrlInvalid(_) => codes::HOOK_WEBHOOK_URL_INVALID,
            AppError::HookExecutionTimeout(_) => codes::HOOK_EXECUTION_TIMEOUT,
            AppError::HookBlockingFailed(_) => codes::HOOK_BLOCKING_FAILED,
            AppError::HookNotFound(_) => codes::HOOK_NOT_FOUND,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            AppError::WrongPassword(m)
            | AppError::AccountDisabled(m)
            | AppError::AccountLocked(m)
            | AppError::TokenInvalid(m)
            | AppError::NotAdministrator(m)
            | AppError::InsufficientPermission(m)
            | AppError::AdminNotFound(m)
            | AppError::PhoneAlreadyExists(m)
            | AppError::CannotDeleteSuperAdmin(m)
            | AppError::CannotDisableLastSuperAdmin(m)
            | AppError::BadRequest(m)
            | AppError::NotFound(m)
            | AppError::Conflict(m)
            | AppError::Internal(m)
            | AppError::CapabilityDeniedRuntime(m)
            | AppError::CapabilityUnknown(m)
            | AppError::TagInUse(m)
            | AppError::DagCycle(m)
            | AppError::ResourceInUse(m)
            | AppError::OptimisticLockConflict(m)
            | AppError::SseConcurrencyExceeded(m)
            | AppError::DailyLimitReached(m)
            | AppError::SensitiveWordBlocked(m)
            | AppError::CannotDeleteMainAgent(m)
            | AppError::SchemaMismatch(m)
            | AppError::CapabilityDeniedChat(m)
            | AppError::PluginInvocationTimeout(m)
            | AppError::WorkflowMappingInvalid(m)
            | AppError::AgentDepthExceeded(m)
            | AppError::ModelPresetUnknown(m)
            | AppError::BuiltinSkillProtected(m)
            | AppError::PoolBusy(m)
            | AppError::BuiltinToolProtected(m)
            | AppError::PluginSystemDisabled(m)
            | AppError::HookTriggerLimitExceeded(m)
            | AppError::HookReferenceInvalid(m)
            | AppError::HookWebhookUrlInvalid(m)
            | AppError::HookExecutionTimeout(m)
            | AppError::HookBlockingFailed(m)
            | AppError::HookNotFound(m)
            | AppError::GameAliasNotFound(m)
            | AppError::GameNameAlreadyExists(m)
            | AppError::GameNameEmpty(m)
            | AppError::GameNameTooLong(m)
            | AppError::AliasesEmpty(m)
            | AppError::AliasTooLong(m)
            | AppError::AliasesTooMany(m)
            | AppError::AliasAlreadyInUse(m)
            | AppError::NewPasswordSameAsOld(m) => m,
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
