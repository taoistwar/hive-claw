//! External database queries for VIP membership and cloud user validation.
//!
//! These functions query the external (non-hive) database for user membership
//! status, balance/coins, and subscription information.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;

use super::cache_helper;
use super::cache_helper::cached_or_fetch;
use crate::cache::redis::RedisClient;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
struct CcUserMembership {
    id: i64,
    membership_level: Option<String>,
    effective_end_time: Option<chrono::NaiveDateTime>,
}

const ACTIVE_MEMBERSHIP_SQL: &str = r#"SELECT id, membership_level, effective_end_time
FROM cc_user_membership
WHERE user_id = ? AND effective_end_time > ?
AND effective_start_time <= ? LIMIT 1"#;

/// Check if a user has an active VIP membership in the external database.
pub async fn check_vip_membership(pool: &MySqlPool, user_id: i64) -> Result<bool, sqlx::Error> {
    let started_at = std::time::Instant::now();
    let china_offset =
        chrono::FixedOffset::east_opt(8 * 60 * 60).expect("UTC+8 is a valid fixed timezone offset");
    let current_time = chrono::Utc::now()
        .with_timezone(&china_offset)
        .naive_local();
    tracing::debug!(
        operation = "check_vip_membership",
        user_id,
        current_time = ?current_time,
        "checking active VIP membership"
    );

    let row = sqlx::query_as::<_, CcUserMembership>(ACTIVE_MEMBERSHIP_SQL)
        .bind(user_id)
        .bind(current_time)
        .bind(current_time)
        .fetch_optional(pool)
        .await;

    match row {
        Ok(Some(_membership)) => {
            tracing::debug!(
                operation = "check_vip_membership",
                outcome = "membership_found",
                user_id,
                duration_ms = started_at.elapsed().as_millis(),
                "finished checking VIP membership"
            );
            Ok(true)
        }
        Ok(None) => {
            tracing::debug!(
                operation = "check_vip_membership",
                outcome = "membership_not_found",
                user_id,
                duration_ms = started_at.elapsed().as_millis(),
                "finished checking VIP membership"
            );
            Ok(false)
        }
        Err(error) => {
            tracing::debug!(
                operation = "check_vip_membership",
                outcome = "query_error",
                user_id,
                error_kind = "membership_query_failed",
                duration_ms = started_at.elapsed().as_millis(),
                "failed to check VIP membership"
            );
            Err(error)
        }
    }
}

/// Get cloud_user uid and nickname for a given user ID.
pub async fn get_cloud_user_info(
    pool: &MySqlPool,
    user_id: i64,
) -> Result<Option<(String, String)>, sqlx::Error> {
    sqlx::query_as::<_, (String, String)>("SELECT uid, nickname FROM cloud_user WHERE ID = ?")
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
        and value>0 AND type IN (0,1,2,3,4,5,6,7,10,12,13,17,18,19,20)
    group by user_id
) m2 on m1.user_id = m2.user_id
LEFT JOIN (
    select user_id, IFNULL(sum(value), 0) as total_coins from cc_user_asset_coin
    where user_id = ? and expire_time > UNIX_TIMESTAMP() and value>0 AND type IN (0,1,2,3,4,5,6,7,10,12,13,17,18,19,20)
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
group by user_id, status"#,
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
    /// cc_product.price — 下次扣款费用（单位：分）
    pub next_price: Option<i32>,
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
        WHEN 'SUBSCRIPTION' THEN '连续订阅'
        WHEN 'ONE_TIME' THEN '单次购买'
        ELSE um.membership_category
    END                         AS membership_category_name,
    um.effective_start_time     AS effective_start_time,
    um.effective_end_time       AS effective_end_time,
    um.product_title            AS product_title,
    us.id                       AS subscription_id,
    CASE um.membership_category
        WHEN 'SUBSCRIPTION' THEN us.status
        ELSE null
    END                        AS subscription_status,
    CASE um.membership_category
        WHEN 'SUBSCRIPTION' THEN '生效中'
        ELSE null
    END                        AS subscription_status_name,

    CASE um.membership_category
        WHEN 'SUBSCRIPTION' THEN us.next_billing_time
        ELSE null
    END AS next_billing_time,
    us.auto_renew               AS auto_renew,
    us.payment_method           AS payment_method,
    us.start_time               AS subscription_start_time,
    us.end_time                 AS subscription_end_time,
    CASE um.membership_category
        WHEN 'SUBSCRIPTION' THEN o.order_price
        ELSE null
    END                        AS next_price
FROM
(select * from cc_user_membership where user_id=? and  effective_end_time > now()) um
LEFT JOIN cc_membership_level ml ON um.membership_level = ml.level_code
LEFT JOIN (
	select * from cc_user_subscription where user_id=? and status ='ACTIVE'
) us ON um.user_id = us.user_id AND um.product_id = us.product_id
LEFT JOIN (
    select * from cc_subscription_order where user_id=?
) so ON so.user_subscription_id = us.id AND us.product_id = so.product_id
LEFT JOIN (
    select * from cc_order where user_id = ?
) o ON o.id = so.order_id AND o.asset_product_id = so.product_id

ORDER BY ml.level_order DESC, um.effective_end_time DESC"#,
    )
    .bind(user_id)
    .bind(user_id)
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
    pub order_id: Option<i64>,
    pub consume_label: Option<serde_json::Value>,
    pub create_time: Option<chrono::DateTime<chrono::Utc>>,
    /// game_label_list JSON — 提取 fps / gpu 等字段
    pub game_label_list: Option<serde_json::Value>,
    /// cc_product.title — 商品名称（如 "金卡"、"黑金卡"）
    pub product_title: Option<String>,
    /// cc_product.value — 商品时长
    pub product_duration: Option<String>,
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
    t1.order_id            AS order_id,
    t1.consume_label       AS consume_label,
    t1.create_time         AS create_time,
    t4.game_label_list      AS game_label_list,
    t3.title               AS product_title,
    t3.value               AS product_duration
FROM (
	SELECT * FROM cc_user_asset_coin
	WHERE user_id = ?
	  AND value > 0
	  AND type IN (8, 9)
	  AND (
	    (type = 8 AND expire_time > UNIX_TIMESTAMP() * 1000)
	    OR
	    (type = 9 AND (expire_time IS NULL OR expire_time > UNIX_TIMESTAMP() * 1000))
	  )
	  AND (consume_label IS NULL
	       OR
           (
                NOT JSON_CONTAINS(consume_label, '"FREE_CARD"', '$.gameLabelList')
                AND
                    NOT JSON_CONTAINS(consume_label, '"BOX_CARD"', '$.gameLabelList')
                AND
                    NOT JSON_CONTAINS(consume_label, '"BOX_CARD_MEMBER"', '$.gameLabelList')
           ))
) t1
LEFT JOIN (
	SELECT * FROM cc_order WHERE user_id = ?
) t2 ON t1.order_id = t2.id
LEFT JOIN cc_product t3 ON t2.asset_product_id = t3.id
LEFT JOIN cc_product_ext t4 ON t3.id = t4.product_id
"#,
    )
    .bind(user_id)
    .bind(user_id)
    .fetch_all(ext_pool)
    .await
    // cc_user_asset_coin t1 join cc_order t2 on t1.order_id = t2.id
    // 排除 t1.customer_label "gameLabelList" 包含 "BOX_CARD",
    // 可以多个
}

/// Resolve game_label_list codes to human-readable names via cc_label table.
/// Input: [{"game_label_list": ["ARM_GAME", "PC_GAME"]}, ...]
/// Output: the same JSON but with codes replaced by names (e.g. ["手游", "PC游戏"])
pub async fn resolve_game_label_names(
    ext_pool: &MySqlPool,
    duration_card_json: &mut [serde_json::Value],
) -> Result<(), String> {
    use std::collections::HashMap;

    // Collect all unique label codes
    let mut codes: Vec<String> = Vec::new();
    for card in duration_card_json.iter() {
        if let Some(list) = card.get("game_label_list").and_then(|v| v.as_array()) {
            for item in list {
                if let Some(code) = item.as_str()
                    && !codes.contains(&code.to_string())
                {
                    codes.push(code.to_string());
                }
            }
        }
    }

    if codes.is_empty() {
        return Ok(());
    }

    // Query cc_label for names
    let placeholders: Vec<String> = codes.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT value, name FROM cc_label WHERE value IN ({})",
        placeholders.join(",")
    );
    let mut query = sqlx::query_as::<_, (String, String)>(&sql);
    for code in &codes {
        query = query.bind(code);
    }
    let rows: Vec<(String, String)> = query
        .fetch_all(ext_pool)
        .await
        .map_err(|e| format!("cc_label query: {e}"))?;

    let map: HashMap<String, String> = rows.into_iter().collect();

    // Replace codes with names
    for card in duration_card_json.iter_mut() {
        if let Some(list) = card
            .get_mut("game_label_list")
            .and_then(|v| v.as_array_mut())
        {
            let resolved: Vec<serde_json::Value> = list
                .iter()
                .map(|item| {
                    let code = item.as_str().unwrap_or("");
                    let name = map.get(code).map(|s| s.as_str()).unwrap_or(code);
                    serde_json::Value::String(name.to_string())
                })
                .collect();
            *list = resolved;
        }
    }

    Ok(())
}

/// Cached version of `get_cloud_user_info`.
pub async fn get_cloud_user_info_cached(
    redis: &RedisClient,
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
    redis: &RedisClient,
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
    redis: &RedisClient,
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
    redis: &RedisClient,
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
    redis: &RedisClient,
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

/// Query AIDiscountedProducts config from cc_config table.
/// Returns the raw JSON content (None if not found).
pub async fn get_discounted_products_config(
    ext_pool: &MySqlPool,
) -> Result<Option<serde_json::Value>, String> {
    let row: Option<(Option<serde_json::Value>,)> = sqlx::query_as(
        "SELECT content FROM cc_config WHERE label = 'AIDiscountedProducts' AND status = 'ACTIVE' LIMIT 1",
    )
    .fetch_optional(ext_pool)
    .await
    .map_err(|e| format!("cc_config query: {e}"))?;
    Ok(row.and_then(|r| r.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_membership_query_uses_bound_time_instead_of_database_now() {
        assert_eq!(ACTIVE_MEMBERSHIP_SQL.matches('?').count(), 3);
        assert!(!ACTIVE_MEMBERSHIP_SQL.to_ascii_uppercase().contains("NOW()"));
    }

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
