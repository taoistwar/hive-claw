//! Balance query builtin — queries user balance and membership from external database.

use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use serde_json::Value;

use agent::context::AgentContext;

// ---------- query_balance ----------

use serde_json::json;

use crate::runtime::builtins::BuiltinContext;
use crate::runtime::builtins::BuiltinError;
use crate::runtime::builtins::BuiltinResult;

/// sync wrapper for query_balance — bridges async DB queries inside tokio runtime.
/// user_id is extracted from AgentContext, not from LLM args.
pub fn query_balance(_args: Value, ctx: &BuiltinContext) -> BuiltinResult {
    // Try to get user_id from context; if not available (e.g. test without user_input), return a structured response
    let user_id: Option<i64> = ctx
        .agent_ctx
        .as_ref()
        .and_then(|agent_ctx| agent_ctx.get_user_metadata("actor_id"))
        .and_then(|s| s.parse().ok());

    let Some(user_id) = user_id else {
        return Ok(json!({
            "message": "未配置用户ID，请在测试输入中提供 actor_id"
        }));
    };

    let ext_pool = ctx
        .ext_pool
        .ok_or_else(|| BuiltinError::Exec("外部数据库未配置".into()))?
        .clone();
    let redis = ctx.redis.cloned();
    let agent_ctx_clone = ctx.agent_ctx.clone();

    tokio::task::block_in_place(move || {
        tokio::runtime::Handle::current().block_on(async move {
            query_balance_async_impl(user_id, &ext_pool, redis.as_ref(), agent_ctx_clone.as_deref()).await
        })
    })
}

pub async fn query_balance_async_impl(
    user_id: i64,
    ext_pool: &sqlx::MySqlPool,
    redis: Option<&redis::Client>,
    _agent_ctx: Option<&AgentContext>,
) -> BuiltinResult {
    // 1. 查询用户余额（Redis 缓存优先）
    let membership = if let Some(r) = redis {
        crate::services::membership::query_membership_balance_cached(r, ext_pool, user_id)
            .await
            .map_err(|e| BuiltinError::Exec(format!("会员查询失败: {e}")))?
    } else {
        crate::services::membership::query_membership_balance(ext_pool, user_id)
            .await
            .map_err(|e| BuiltinError::Exec(format!("会员查询失败: {e}")))?
    };

    if membership.is_none() {
        return Ok(json!({
            "message": "暂无该用户的资产数据，请稍后再试"
        }));
    }

    // 1.5 查询会员与订阅状态（Redis 缓存优先）
    let membership_subscriptions = if let Some(r) = redis {
        crate::services::membership::query_membership_subscriptions_cached(r, ext_pool, user_id)
            .await
            .map_err(|e| BuiltinError::Exec(format!("会员订阅查询失败: {e}")))?
    } else {
        crate::services::membership::query_membership_subscriptions(ext_pool, user_id)
            .await
            .map_err(|e| BuiltinError::Exec(format!("会员订阅查询失败: {e}")))?
    };

    // Serialize membership+subscription rows to JSON
    let membership_json: Vec<Value> = membership_subscriptions
        .iter()
        .map(|row| {
            json!({
                "membership_level": row.membership_level,
                "level_name": row.level_name,
                "membership_category": row.membership_category,
                "membership_category_name": row.membership_category_name,
                "effective_start_time": row.effective_start_time.map(|t| t.to_string()),
                "effective_end_time": row.effective_end_time.map(|t| t.to_string()),
                "product_title": row.product_title,
                "subscription_id": row.subscription_id,
                "subscription_status": row.subscription_status,
                "subscription_status_name": row.subscription_status_name,
                "next_billing_time": row.next_billing_time.map(|t| t.to_string()),
                "auto_renew": row.auto_renew.map(|v| v != 0),
                "payment_method": row.payment_method,
                "subscription_start_time": row.subscription_start_time.map(|t| t.to_string()),
                "subscription_end_time": row.subscription_end_time.map(|t| t.to_string()),
            })
        })
        .collect();

    // 1.8 查询时长卡（Redis 缓存优先）
    let duration_cards = if let Some(r) = redis {
        crate::services::membership::query_duration_cards_cached(r, ext_pool, user_id)
            .await
            .map_err(|e| BuiltinError::Exec(format!("时长卡查询失败: {e}")))?
    } else {
        crate::services::membership::query_duration_cards(ext_pool, user_id)
            .await
            .map_err(|e| BuiltinError::Exec(format!("时长卡查询失败: {e}")))?
    };

    let duration_card_json: Vec<Value> = duration_cards
        .iter()
        .map(|row| {
            json!({
                "card_asset_id": row.card_asset_id,
                "remain_duration": row.remain_duration,
                "computer_biz_type": row.computer_biz_type,
                "expire_time": row.expire_time,
                "card_type": row.card_type,
                "card_type_name": row.card_type_name,
                "order_id": row.order_id,
                "consume_label": row.consume_label,
                "extra": row.extra,
                "create_time": row.create_time.map(|t| t.to_string()),
            })
        })
        .collect();

    // Derive card type from membership_subscriptions (highest-priority active row)
    let active_membership = membership_subscriptions
        .iter()
        .find(|row| {
            row.effective_end_time
                .map(|end| end >= chrono::Utc::now().naive_utc())
                .unwrap_or(false)
        });

    let has_membership = active_membership.is_some();
    let days_until_expiry = active_membership
        .and_then(|m| m.effective_end_time)
        .map(|end| (end - chrono::Utc::now().naive_utc()).num_days());
    let expiring_soon = days_until_expiry.map(|d| d <= 7).unwrap_or(false);

    let upgrade_suggested = has_membership
        && active_membership
            .and_then(|m| m.membership_category.as_deref())
            != Some("LEGEND");

    // 2. 构造返回结果
    let mut result = json!({});
    let m = membership.unwrap();
    let reply = json!({
        "disk_end_time": m.disk_end_time.unwrap_or(0),
        "disk_total_size": m.disk_total_size
            .and_then(|s| s.to_f64())
            .unwrap_or(0.0),
        "total_coins": m.total_coins.unwrap_or(Decimal::ZERO).to_f64().unwrap_or(0.0),
        "expire_coins_7d": m.expire_coins_7d.unwrap_or(Decimal::ZERO).to_f64().unwrap_or(0.0),
    });

    if let Value::Object(ref mut map) = result {
        // 构造扩展卡片
        let mut extension_list: Vec<Value> = Vec::new();

        if !has_membership {
            // 无有效会员 → firstPay 卡
            extension_list.push(json!({
                "content_type": "card",
                "payload": {
                    "type": "subscribe",
                    "info": reply,
                    "membership": membership_json,
                    "duration_card": duration_card_json,
                },
            }));
        } else if expiring_soon {
            // 会员即将到期（≤ 7 天）→ repay 卡
            extension_list.push(json!({
                "content_type": "card",
                "payload": {
                    "type": "repay",
                    "info": reply,
                    "membership": membership_json,
                    "duration_card": duration_card_json,
                },
            }));
        } else if upgrade_suggested {
            // 升级建议卡
            extension_list.push(json!({
                "content_type": "card",
                "payload": {
                    "type": "upgrade",
                    "info": reply,
                    "membership": membership_json,
                    "duration_card": duration_card_json,
                },
            }));
        } else {
            extension_list.push(json!({
                "content_type": "card",
                "payload": {
                    "type": "sufficient",
                    "info": reply,
                    "membership": membership_json,
                    "duration_card": duration_card_json,
                },
            }));
        }

        // Build _agent_context_updates with extensions (if any) and loop-break signal
        let mut updates = json!({
            "metadata": {
                "agent_loop_break": "true"
            }
        });
        if !extension_list.is_empty() {
            updates["extensions"] = json!(extension_list);
        }
        map.insert("_agent_context_updates".into(), updates);
    }

    tracing::debug!("HOP tool call result: {:?}", result);

    Ok(result)
}

pub const QUERY_BALANCE_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {}
}"#;

pub const QUERY_BALANCE_OUTPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "message": {
      "type": "string",
      "description": "查询结果的文本描述，适合直接展示给用户。"
    }
  }
}"#;
