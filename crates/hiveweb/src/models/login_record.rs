use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Login record row — post-V004 schema. `admin_id` may be NULL when the
/// referenced admin has been deleted (ON DELETE SET NULL). The snapshot
/// columns retain the phone/nickname at the time of the login event so the
/// audit trail outlives the admin record itself (spec FR-022).
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct LoginRecord {
    pub id: i64,
    pub admin_id: Option<i64>,
    pub admin_phone_snapshot: String,
    pub admin_nickname_snapshot: String,
    pub login_at: DateTime<Utc>,
    pub ip_address: String,
    pub success: bool,
    pub failure_reason: Option<String>,
}
