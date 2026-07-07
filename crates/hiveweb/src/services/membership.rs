//! External database queries for VIP membership and cloud user validation.
//!
//! These functions query the external (non-hive) database for user membership
//! status, balance/coins, and subscription information.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;

use super::cache_helper;
use super::cache_helper::cached_or_fetch;

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
    us.end_time                 AS subscription_end_time,
    t4.price                    AS next_price
FROM
(select * from cc_user_membership where user_id=? and  effective_end_time > now()) um
LEFT JOIN cc_membership_level ml ON um.membership_level = ml.level_code
LEFT JOIN (
	select * from cc_user_subscription where user_id=?
) us ON um.user_subscription_id = us.id
LEFT JOIN cc_product t4 ON um.product_id = t4.id
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
	       OR NOT JSON_CONTAINS(consume_label, '"FREE_CARD"', '$.gameLabelList'))
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
                if let Some(code) = item.as_str() {
                    if !codes.contains(&code.to_string()) {
                        codes.push(code.to_string());
                    }
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
        if let Some(list) = card.get_mut("game_label_list").and_then(|v| v.as_array_mut()) {
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
    if exists {
        // 24h — 用户存在，长缓存
        let ttl = cache_helper::TTL_CLOUD_USER_EXISTS_POSITIVE;
        if let Err(e) = cache_helper::cached_set(redis, &key, &exists, ttl).await {
            tracing::debug!(%key, error = %e, "cache write failed");
        }
    }

    Ok(exists)
}

/// VIP 状态查询（智能缓存）：
/// - 已经是 VIP → 缓存，下次直接返回 true
/// - 不是 VIP → 不缓存，每次查 DB（确保充值后立即识别）
pub async fn check_vip_membership_cached(
    redis: &redis::Client,
    pool: &MySqlPool,
    user_id: i64,
) -> Result<bool, String> {
    let key = format!("{}:{}", cache_helper::KEY_VIP_STATUS, user_id);

    // 1. 先查缓存
    if let Ok(Some(cached)) = cache_helper::cached_get::<bool>(redis, &key).await {
        if cached {
            return Ok(true);
        }
    }

    // 2. 查 DB
    let is_vip = check_vip_membership(pool, user_id)
        .await
        .map_err(|e| format!("check_vip_membership: {e}"))?;

    // 3. 只有 VIP 才缓存
    if is_vip {
        if let Err(e) = cache_helper::cached_set(redis, &key, &true, cache_helper::TTL_VIP_STATUS).await {
            tracing::debug!(%key, error = %e, "VIP cache write failed");
        }
    }

    Ok(is_vip)
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
