//! Timezone contract for `/api/dashboard/recent-logins`.

use chrono::TimeZone;
use hiveweb::services::dashboard::RecentLoginRow;

#[test]
fn recent_login_timestamp_includes_utc_offset() {
    let row = RecentLoginRow {
        id: 1,
        admin_id: Some(2),
        admin_nickname: "admin".to_string(),
        login_at: chrono::Utc
            .with_ymd_and_hms(2026, 7, 15, 2, 15, 1)
            .single()
            .expect("valid UTC timestamp"),
        ip_address: "127.0.0.1".to_string(),
        success: true,
    };

    let value = serde_json::to_value(row).expect("recent login row should serialize");

    assert_eq!(value["login_at"], "2026-07-15T02:15:01Z");
}
