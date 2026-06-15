//! External database queries for VIP membership and cloud user validation.
//!
//! These functions query the external (non-hive) database for user membership
//! status, balance/coins, and subscription information.

use rust_decimal::Decimal;
use sqlx::MySqlPool;

#[derive(Debug, Clone, sqlx::FromRow)]
struct CcUserMembership {
    id: i64,
    membership_level: Option<String>,
    effective_end_time: Option<chrono::NaiveDateTime>,
}

/// Check if a user has an active VIP membership in the external database.
pub async fn check_vip_membership(pool: &MySqlPool, user_id: i64) -> Result<bool, sqlx::Error> {
    sqlx::query_as::<_, CcUserMembership>(
        "SELECT id, membership_level, effective_end_time FROM cc_user_membership WHERE id = ? LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map(|row| {
        row.map_or(false, |m| {
            m.effective_end_time
                .map_or(true, |end| end >= chrono::Utc::now().naive_utc())
        })
    })
}

/// Check if a user exists in the external cloud_user table.
pub async fn user_exists_in_cloud(pool: &MySqlPool, user_id: i64) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(1) FROM cloud_user WHERE ID = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .map(|count| count > 0)
}

/// Get cloud_user uid and nickname for a given user ID.
pub async fn get_cloud_user_info(
    pool: &MySqlPool,
    user_id: i64,
) -> Result<Option<(String, String)>, sqlx::Error> {
    sqlx::query_as::<_, (String, String)>(
        "SELECT uid, nickname FROM cloud_user WHERE ID = ?",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
}

// ---------- query_balance support ----------

/// Row returned by the balance/coins query.
#[derive(Debug, sqlx::FromRow)]
#[allow(dead_code)]
pub struct MembershipBalanceRow {
    pub total_coins: Option<Decimal>,
    pub expire_coins_7d: Option<Decimal>,
    pub disk_total_size: Option<Decimal>,
    pub disk_end_time: Option<i64>,
}

/// Query user balance: total coins and coins expiring within 7 days.
pub async fn query_membership_balance(
    ext_pool: &MySqlPool,
    user_id: i64,
) -> Result<Option<MembershipBalanceRow>, sqlx::Error> {
    sqlx::query_as(
        r#"select
	m7.total_coins, m2.expire_coins_7d, m4.disk_total_size, m4.disk_end_time
from
(
	select ? as user_id
) m1
left join
(
	select user_id, IFNULL(sum(value), 0) as expire_coins_7d from cc_user_asset_coin
	where user_id = ?
	    and expire_time > UNIX_TIMESTAMP() *1000
	    and expire_time < (7*24*60*60*1000+UNIX_TIMESTAMP()*1000)
	    and value>0
	group by user_id
) m2 on m1.user_id = m2.user_id
LEFT JOIN (
	SELECT user_id, sum(size/1024/1024/1024) as disk_total_size, MAX(end_time) as disk_end_time
	from cc_user_disk
	where user_id=? and end_time > UNIX_TIMESTAMP()*1000 and start_time < UNIX_TIMESTAMP()*1000
	group by user_id
) m4 on m1.user_id = m4.user_id
LEFT JOIN (
	select user_id, IFNULL(sum(value), 0) as total_coins from cc_user_asset_coin
  where user_id = ? and expire_time > UNIX_TIMESTAMP() and value>0
	group by user_id
) m7 on m1.user_id = m7.user_id
"#,
    )
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .fetch_optional(ext_pool)
    .await
}

/// Row returned by the membership + subscription status query.
#[derive(Debug, sqlx::FromRow)]
#[allow(dead_code)]
pub struct MembershipSubscriptionRow {
    pub membership_level: Option<String>,
    pub level_name: Option<String>,
    pub membership_category: Option<String>,
    pub membership_category_name: Option<String>,
    pub effective_start_time: Option<chrono::NaiveDateTime>,
    pub effective_end_time: Option<chrono::NaiveDateTime>,
    pub product_title: Option<String>,
    pub subscription_id: Option<i64>,
    pub subscription_status: Option<String>,
    pub subscription_status_name: Option<String>,
    pub next_billing_time: Option<chrono::NaiveDateTime>,
    pub auto_renew: Option<i8>,
    pub payment_method: Option<String>,
    pub subscription_start_time: Option<chrono::NaiveDateTime>,
    pub subscription_end_time: Option<chrono::NaiveDateTime>,
}

/// Query all membership records and associated subscription status for a user.
pub async fn query_membership_subscriptions(
    ext_pool: &MySqlPool,
    user_id: i64,
) -> Result<Vec<MembershipSubscriptionRow>, sqlx::Error> {
    sqlx::query_as(
        r#"SELECT
    um.membership_level         AS membership_level,
    ml.level_name               AS level_name,
    um.membership_category      AS membership_category,
    CASE um.membership_category
        WHEN 'SUBSCRIPTION' THEN '订阅型'
        WHEN 'ONE_TIME' THEN '一次性'
        ELSE um.membership_category
    END                         AS membership_category_name,
    um.effective_start_time     AS effective_start_time,
    um.effective_end_time       AS effective_end_time,
    um.product_title            AS product_title,
    us.id                       AS subscription_id,
    us.status                   AS subscription_status,
    CASE us.status
        WHEN 'ACTIVE'  THEN '生效'
        WHEN 'REVOKE'  THEN '已解约'
        WHEN 'EXPIRED' THEN '已过期'
        WHEN 'PENDING' THEN '待签约'
        ELSE us.status
    END                         AS subscription_status_name,
    us.next_billing_time        AS next_billing_time,
    us.auto_renew               AS auto_renew,
    us.payment_method           AS payment_method,
    us.start_time               AS subscription_start_time,
    us.end_time                 AS subscription_end_time
FROM cc_user_membership um
LEFT JOIN cc_membership_level ml ON um.membership_level = ml.level_code
LEFT JOIN cc_user_subscription us ON um.user_subscription_id = us.id
WHERE um.user_id = ?
ORDER BY ml.level_order DESC, um.effective_end_time DESC"#,
    )
    .bind(user_id)
    .fetch_all(ext_pool)
    .await
}

/// Row returned by the duration card (时长卡) query.
#[derive(Debug, sqlx::FromRow)]
#[allow(dead_code)]
pub struct DurationCardRow {
    pub card_asset_id: Option<i64>,
    pub remain_duration: Option<i64>,
    pub computer_biz_type: Option<String>,
    pub expire_time: Option<i64>,
    pub card_type: Option<i8>,
    pub card_type_name: Option<String>,
    pub order_id: Option<i64>,
    pub consume_label: Option<serde_json::Value>,
    pub extra: Option<serde_json::Value>,
    pub create_time: Option<chrono::DateTime<chrono::Utc>>,
}

/// Query duration cards (时长卡) for a user — gold card (type=8) and black gold card (type=9).
pub async fn query_duration_cards(
    ext_pool: &MySqlPool,
    user_id: i64,
) -> Result<Vec<DurationCardRow>, sqlx::Error> {
    sqlx::query_as(
        r#"SELECT
    uac.id                      AS card_asset_id,
    uac.value                   AS remain_duration,
    uac.computer_biz_type       AS computer_biz_type,
    uac.expire_time             AS expire_time,
    uac.type                    AS card_type,
    CASE uac.type
        WHEN 8 THEN '金卡'
        WHEN 9 THEN '黑金卡'
        ELSE '其他'
    END                         AS card_type_name,
    uac.order_id                AS order_id,
    uac.consume_label           AS consume_label,
    uac.extra                   AS extra,
    uac.create_time             AS create_time
FROM cc_user_asset_coin uac
WHERE uac.user_id = ?
  AND uac.type IN (8, 9)
  AND uac.value > 0
  AND (uac.type = 9 OR uac.expire_time > UNIX_TIMESTAMP(NOW()) * 1000)
ORDER BY uac.value ASC"#,
    )
    .bind(user_id)
    .fetch_all(ext_pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires external DB"]
    async fn check_vip_membership_active() {
        // Requires seeded cc_user_membership with future effective_end_time
    }

    #[tokio::test]
    #[ignore = "requires external DB"]
    async fn user_exists_in_cloud_returns_true() {
        // Requires seeded cloud_user
    }
}
