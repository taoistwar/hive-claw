//! External database queries for VIP membership and cloud user validation.
//!
//! These functions query the external (non-hive) database for user membership
//! status, balance/coins, and subscription information.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;

use super::cache_helper;
use super::cache_helper::{cached_or_fetch};

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
struct CcUserMembership {
    id: i64,
    membership_level: Option<String>,
    effective_end_time: Option<chrono::NaiveDateTime>,
}

/// Check if a user has an active VIP membership in the external database.
pub async fn check_vip_membership(pool: &MySqlPool, user_id: i64) -> Result<bool, sqlx::Error> {
    sqlx::query_as::<_, CcUserMembership>(
        r#"SELECT id, membership_level, effective_end_time FROM cc_user_membership
        WHERE user_id = ? and effective_end_time > now() AND effective_start_time < now() LIMIT 1"#,
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

/// Row returned by the coins balance query.
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
#[allow(dead_code)]
pub struct CoinsBalanceRow {
    pub total_coins: Option<Decimal>,
    pub expire_coins_7d: Option<Decimal>,
}

/// Row returned by the disk balance query.
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
#[allow(dead_code)]
pub struct DiskBalanceRow {
    pub disk_total_size: Option<Decimal>,
    pub disk_end_time: Option<i64>,
    pub disk_status: Option<String>,
}

/// Query user coins balance: total_coins and expire_coins_7d.
pub async fn query_coins_balance(
    ext_pool: &MySqlPool,
    user_id: i64,
) -> Result<Option<CoinsBalanceRow>, sqlx::Error> {
    sqlx::query_as(
        r#"select
    m7.total_coins, m2.expire_coins_7d
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
        and value>0 AND type=4
    group by user_id
) m2 on m1.user_id = m2.user_id
LEFT JOIN (
    select user_id, IFNULL(sum(value), 0) as total_coins from cc_user_asset_coin
  where user_id = ? and expire_time > UNIX_TIMESTAMP() and value>0 AND type=4
    group by user_id
) m7 on m1.user_id = m7.user_id
"#,
    )
    .bind(user_id)
    .bind(user_id)
    .bind(user_id)
    .fetch_optional(ext_pool)
    .await
}

/// Query user disk balance: disk_total_size, disk_end_time, disk_status.
pub async fn query_disk_balance(
    ext_pool: &MySqlPool,
    user_id: i64,
) -> Result<Option<DiskBalanceRow>, sqlx::Error> {
    sqlx::query_as(
        r#"SELECT user_id, sum(size/1024/1024/1024) as disk_total_size, MAX(end_time) as disk_end_time, status as disk_status
from cc_user_disk
where user_id=? and end_time > UNIX_TIMESTAMP()*1000 and start_time < UNIX_TIMESTAMP()*1000 AND status != 'EXPIRED'
group by user_id"#,
    )
    .bind(user_id)
    .fetch_optional(ext_pool)
    .await
}

/// Row returned by the membership + subscription status query.
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
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
FROM
(select * from cc_user_membership where user_id=? and  effective_end_time > now()) um
LEFT JOIN cc_membership_level ml ON um.membership_level = ml.level_code
LEFT JOIN (
	select * from cc_user_subscription where user_id=?
) us ON um.user_subscription_id = us.id

ORDER BY ml.level_order DESC, um.effective_end_time DESC"#,
    )
    .bind(user_id)
    .bind(user_id)
    .fetch_all(ext_pool)
    .await
}

/// Row returned by the duration card (时长卡) query.
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
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
    pub create_time: Option<chrono::DateTime<chrono::Utc>>,
    /// product_mirror JSON — 提取 fps / gpu 等字段
    pub product_mirror: Option<serde_json::Value>,
}

/// Query duration cards (时长卡) for a user — gold card (type=8) and black gold card (type=9).
pub async fn query_duration_cards(
    ext_pool: &MySqlPool,
    user_id: i64,
) -> Result<Vec<DurationCardRow>, sqlx::Error> {
    sqlx::query_as(
        r#"SELECT
    t1.id                  AS card_asset_id,
    t1.value               AS remain_duration,
    t1.computer_biz_type   AS computer_biz_type,
    t1.expire_time         AS expire_time,
    t1.type                AS card_type,
    CASE t1.type
        WHEN 8 THEN '金卡'
        WHEN 9 THEN '黑金卡'
        ELSE '其他'
    END                     AS card_type_name,
    t1.order_id            AS order_id,
    t1.consume_label       AS consume_label,
    t1.create_time         AS create_time,
    t2.product_mirror      AS product_mirror
FROM (
	select * from cc_user_asset_coin  WHERE user_id = ? AND type IN (8, 9) AND value > 0 AND (
      (type = 8 AND expire_time > UNIX_TIMESTAMP() * 1000)
      OR
      (type = 9 AND (expire_time IS NULL OR expire_time > UNIX_TIMESTAMP() * 1000))
	)
) t1
left join (
	SELECT * from cc_order where user_id = ?
) t2 on t1.order_id = t2.id"#,
    )
    .bind(user_id)
    .bind(user_id)
    .fetch_all(ext_pool)
    .await
    // cc_user_asset_coin t1 join cc_order t2 on t1.order_id = t2.id
    // 排除 t1.customer_label "gameLabelList" 包含 "BOX_CARD",
    // 可以多个
}

// ── Redis-cached wrappers ──

/// Cached version of `user_exists_in_cloud`.
///
/// Split-TTL strategy: if the user exists, cache for a long duration (24h)
/// because accounts never disappear. If the user does not exist, cache only
/// briefly (5min) because they may be a newly registered user.
pub async fn user_exists_in_cloud_cached(
    redis: &redis::Client,
    pool: &MySqlPool,
    user_id: i64,
) -> Result<bool, String> {
    let key = format!("{}:{}", cache_helper::KEY_CLOUD_USER_EXISTS, user_id);

    // 1. Try Redis
    match cache_helper::cached_get::<bool>(redis, &key).await {
        Ok(Some(value)) => return Ok(value),
        Ok(None) => {} // cache miss
        Err(e) => tracing::debug!(%key, error = %e, "cache read failed, falling back to DB"),
    }

    // 2. Fetch from DB
    let exists = user_exists_in_cloud(pool, user_id)
        .await
        .map_err(|e| format!("user_exists_in_cloud: {e}"))?;

    // 3. Write to cache with split TTL
    let ttl = if exists {
        cache_helper::TTL_CLOUD_USER_EXISTS_POSITIVE // 24h — 用户存在，长缓存
    } else {
        cache_helper::TTL_CLOUD_USER_EXISTS_NEGATIVE // 5min — 用户不存在，短缓存
    };
    if let Err(e) = cache_helper::cached_set(redis, &key, &exists, ttl).await {
        tracing::debug!(%key, error = %e, "cache write failed");
    }

    Ok(exists)
}

/// Cached version of `check_vip_membership`.
pub async fn check_vip_membership_cached(
    redis: &redis::Client,
    pool: &MySqlPool,
    user_id: i64,
) -> Result<bool, String> {
    let key = format!("{}:{}", cache_helper::KEY_VIP_STATUS, user_id);
    cached_or_fetch(redis, &key, cache_helper::TTL_VIP_STATUS, || async {
        check_vip_membership(pool, user_id)
            .await
            .map_err(|e| format!("check_vip_membership: {e}"))
    })
    .await
}

/// Cached version of `get_cloud_user_info`.
pub async fn get_cloud_user_info_cached(
    redis: &redis::Client,
    pool: &MySqlPool,
    user_id: i64,
) -> Result<Option<(String, String)>, String> {
    let key = format!("{}:{}", cache_helper::KEY_CLOUD_USER_INFO, user_id);
    cached_or_fetch(redis, &key, cache_helper::TTL_CLOUD_USER_INFO, || async {
        get_cloud_user_info(pool, user_id)
            .await
            .map_err(|e| format!("get_cloud_user_info: {e}"))
    })
    .await
}

/// Cached version of `query_coins_balance`.
pub async fn query_coins_balance_cached(
    redis: &redis::Client,
    ext_pool: &MySqlPool,
    user_id: i64,
) -> Result<Option<CoinsBalanceRow>, String> {
    let key = format!("{}:coins:{}", cache_helper::KEY_BALANCE, user_id);
    cached_or_fetch(redis, &key, cache_helper::TTL_BALANCE, || async {
        query_coins_balance(ext_pool, user_id)
            .await
            .map_err(|e| format!("query_coins_balance: {e}"))
    })
    .await
}

/// Cached version of `query_disk_balance`.
pub async fn query_disk_balance_cached(
    redis: &redis::Client,
    ext_pool: &MySqlPool,
    user_id: i64,
) -> Result<Option<DiskBalanceRow>, String> {
    let key = format!("{}:disk:{}", cache_helper::KEY_BALANCE, user_id);
    cached_or_fetch(redis, &key, cache_helper::TTL_BALANCE, || async {
        query_disk_balance(ext_pool, user_id)
            .await
            .map_err(|e| format!("query_disk_balance: {e}"))
    })
    .await
}

/// Cached version of `query_membership_subscriptions`.
pub async fn query_membership_subscriptions_cached(
    redis: &redis::Client,
    ext_pool: &MySqlPool,
    user_id: i64,
) -> Result<Vec<MembershipSubscriptionRow>, String> {
    let key = format!("{}:{}", cache_helper::KEY_SUBSCRIPTIONS, user_id);
    cached_or_fetch(redis, &key, cache_helper::TTL_SUBSCRIPTIONS, || async {
        query_membership_subscriptions(ext_pool, user_id)
            .await
            .map_err(|e| format!("query_membership_subscriptions: {e}"))
    })
    .await
}

/// Cached version of `query_duration_cards`.
pub async fn query_duration_cards_cached(
    redis: &redis::Client,
    ext_pool: &MySqlPool,
    user_id: i64,
) -> Result<Vec<DurationCardRow>, String> {
    let key = format!("{}:{}", cache_helper::KEY_DURATION_CARDS, user_id);
    cached_or_fetch(redis, &key, cache_helper::TTL_DURATION_CARDS, || async {
        query_duration_cards(ext_pool, user_id)
            .await
            .map_err(|e| format!("query_duration_cards: {e}"))
    })
    .await
}

// ── AI Assistant Chat Limit Config (from external cc_config) ──

/// Daily rate-limit configuration loaded from external cc_config table
/// (label = 'AIassistantChatLimitConfig').
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AssistantChatLimitConfig {
    #[serde(default = "default_vip_ask_times")]
    pub vip_ask_times: i64,
    #[serde(default = "default_normal_ask_times")]
    pub normal_ask_times: i64,
    /// 当剩余次数等于该值时，追加 usage extension 提醒
    #[serde(default)]
    pub remain_ask_time: i64,
    /// 每日重置小时 (0-23 UTC)
    #[serde(default)]
    pub limit_reset_hour: u32,
}

fn default_vip_ask_times() -> i64 {
    50
}
fn default_normal_ask_times() -> i64 {
    10
}

impl Default for AssistantChatLimitConfig {
    fn default() -> Self {
        Self {
            vip_ask_times: 50,
            normal_ask_times: 10,
            remain_ask_time: 2,
            limit_reset_hour: 0,
        }
    }
}

/// Query AI assistant daily limit config from external cc_config table.
pub async fn get_ai_assistant_chat_limit_config(
    ext_pool: &MySqlPool,
) -> Result<Option<AssistantChatLimitConfig>, String> {
    let row: Option<(Option<serde_json::Value>,)> = sqlx::query_as(
        "SELECT content FROM cc_config WHERE label = 'AIassistantChatLimitConfig' AND status = 'ACTIVE'  LIMIT 1",
    )
    .fetch_optional(ext_pool)
    .await
    .map_err(|e| format!("cc_config query: {e}"))?;
    match row.and_then(|r| r.0) {
        Some(json_value) => {
            let config: AssistantChatLimitConfig = serde_json::from_value(json_value)
                .map_err(|e| format!("deserialize AIassistantChatLimitConfig: {e}"))?;
            Ok(Some(config))
        }
        None => Ok(None),
    }
}

/// Cached version of `get_ai_assistant_chat_limit_config`.
pub async fn get_ai_assistant_chat_limit_config_cached(
    redis: &redis::Client,
    ext_pool: &MySqlPool,
) -> Result<AssistantChatLimitConfig, String> {
    let key = cache_helper::KEY_AI_ASSISTANT_CHAT_LIMIT_CONFIG;
    cached_or_fetch(
        redis,
        key,
        cache_helper::TTL_AI_ASSISTANT_CHAT_LIMIT_CONFIG,
        || async {
            get_ai_assistant_chat_limit_config(ext_pool)
                .await
                .map(|opt| opt.unwrap_or_default())
        },
    )
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
