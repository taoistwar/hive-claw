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
/// category is optionally provided in args to filter the payload:
///   membership | coins | duration_card | disk | discount | benefits (default: all fields)
///
/// category values: membership | coins | duration_card | disk | discount | benefits
pub fn query_balance(args: Value, ctx: &BuiltinContext) -> BuiltinResult {
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

    let category = args
        .get("category")
        .and_then(|v| v.as_str())
        .unwrap_or("benefits")
        .to_string();

    tokio::task::block_in_place(move || {
        tokio::runtime::Handle::current().block_on(async move {
            query_balance_async_impl(
                user_id,
                &ext_pool,
                redis.as_ref(),
                agent_ctx_clone.as_deref(),
                &category,
            )
            .await
        })
    })
}

// ─── discount category: 优惠产品 ─────────────────────────────────────────

/// Handle discount category:
/// 1. Read client_type & channel from AgentContext
/// 2. Query AIDiscountedProducts config from cc_config
/// 3. Navigate JSON: client_type → channel → product_id → settings
/// 4. Query cc_product for product info
/// 5. Return single discount extension
/// 任何步骤失败时返回 "暂无产品优惠"
async fn handle_discount(
    user_id: i64,
    ext_pool: &sqlx::MySqlPool,
    agent_ctx: Option<&AgentContext>,
) -> BuiltinResult {
    let result = try_handle_discount(user_id, ext_pool, agent_ctx).await;
    match result {
        Ok(r) => Ok(r),
        Err(e) => {
            tracing::warn!(error = %e, "[handle_discount] failed, returning no discount");
            Ok(serde_json::json!({ "message": "暂无产品优惠活动" }))
        }
    }
}

async fn try_handle_discount(
    user_id: i64,
    ext_pool: &sqlx::MySqlPool,
    agent_ctx: Option<&AgentContext>,
) -> BuiltinResult {
    let _ = user_id;

    // 1. 从 AgentContext 获取 client_type 和 channel
    let client_type = agent_ctx
        .and_then(|ac| ac.get_user_metadata("client_type"))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| BuiltinError::Exec("client_type 未配置".into()))?;
    let channel = agent_ctx
        .and_then(|ac| ac.get_user_metadata("channel"))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| BuiltinError::Exec("channel 未配置".into()))?;
    tracing::debug!(%client_type, %channel, "[handle_discount] step1: client_type & channel");

    // 2. 读取 AIDiscountedProducts 配置
    let config = crate::services::membership::get_discounted_products_config(ext_pool)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "AIDiscountedProducts 配置查询失败");
            BuiltinError::Exec(format!("{e}"))
        })?
        .ok_or_else(|| {
            tracing::warn!("AIDiscountedProducts 配置未找到");
            BuiltinError::Exec("AIDiscountedProducts 配置未找到".into())
        })?;
    tracing::debug!(config = %config, "[handle_discount] step2: AIDiscountedProducts config");

    // 3. 导航 JSON: client_type → channel → products
    let discount = resolve_discount_products(&config, &client_type, &channel)?;
    tracing::debug!(
        ?discount,
        "[handle_discount] step3: resolved discount for {}/{}",
        client_type,
        channel
    );
    if discount.as_object().map_or(true, |o| o.is_empty()) {
        return Ok(serde_json::json!({
            "message": "暂无产品优惠活动"
        }));
    }

    // 4. 仅保留指定字段，避免前端收到未预期的配置
    let discount = filter_discount_fields(&discount);

    // 5. 直接返回配置作为 discount payload
    let mut result = serde_json::json!({
        "found": true,
    });

    if let serde_json::Value::Object(ref mut map) = result {
        map.insert(
            "_agent_context_updates".into(),
            serde_json::json!({
                "extensions": [{
                    "content_type": "card",
                    "payload": {
                        "type": "discount",
                        "discount": discount,
                    },
                }],
                "metadata": {
                    "agent_loop_break": "true"
                }
            }),
        );
    }

    Ok(result)
}

/// Resolve discount products from the nested config:
/// config[client_type][channel] → product map
fn resolve_discount_products(
    config: &serde_json::Value,
    client_type: &str,
    channel: &str,
) -> Result<serde_json::Value, BuiltinError> {
    /// 忽略大小写查找 JSON object 中的 key
    fn find_ignore_case<'a>(obj: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
        obj.as_object().and_then(|map| {
            map.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .or_else(|| map.iter().find(|(k, _)| k.eq_ignore_ascii_case("__DEFAULT__")))
                .map(|(_, v)| v)
        })
    }

    let ct_obj = find_ignore_case(config, client_type).ok_or_else(|| {
        BuiltinError::Exec(format!(
            "AIDiscountedProducts: 未找到 client_type={client_type} 或 __DEFAULT__"
        ))
    })?;

    let ch_obj = find_ignore_case(ct_obj, channel).ok_or_else(|| {
        BuiltinError::Exec(format!(
            "AIDiscountedProducts: 未找到 channel={channel} 或 __DEFAULT__ in client_type={client_type}"
        ))
    })?;

    Ok(ch_obj.clone())
}

/// Filter discount config to only allow specific fields, preventing frontend
/// from receiving unexpected structure changes.
fn filter_discount_fields(discount: &Value) -> Value {
    const ALLOWED: &[&str] = &[
        "link", "bgimg", "buttonImg", "titleDesc", "buttonDesc",
        "titleColor", "buttonColor", "description", "descriptionColor",
    ];
    if let Some(obj) = discount.as_object() {
        let filtered: serde_json::Map<String, Value> = obj
            .iter()
            .filter(|(k, _)| ALLOWED.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Value::Object(filtered)
    } else {
        discount.clone()
    }
}

pub async fn query_balance_async_impl(
    user_id: i64,
    ext_pool: &sqlx::MySqlPool,
    _redis: Option<&redis::Client>,
    _agent_ctx: Option<&AgentContext>,
    category: &str,
) -> BuiltinResult {
    let need_coins = matches!(category, "coins" | "benefits");
    let need_disk = matches!(category, "disk" | "benefits");
    let need_duration = matches!(category, "duration_card" | "benefits");
    tracing::debug!(%user_id, %category, need_coins, need_disk, need_duration, "[query_balance] step0: start");

    // ★ discount: 从配置表 + 资费表获取优惠产品信息
    if category == "discount" {
        return handle_discount(user_id, ext_pool, _agent_ctx).await;
    }

    // 1. 查询金币余额（仅 coins / benefits 需要）
    let coins_row = if need_coins {
        let row =
            crate::services::membership::query_coins_balance(ext_pool, user_id)
                .await
                .map_err(|e| {
                    tracing::error!(user_id = %user_id, error = %e, "金币查询失败");
                    BuiltinError::Exec(format!("金币查询失败: {e}"))
                })?;
        if row.is_none() {
            tracing::debug!(%user_id, "[query_balance] step1 coins: no data");
            return Ok(json!({
                "message": "暂无该用户的资产数据，请稍后再试"
            }));
        }
        tracing::debug!(%user_id, ?row, "[query_balance] step1 coins: row");
        row
    } else {
        None
    };

    // 1.2 查询云硬盘信息（仅 disk / benefits 需要）
    let disk_row = if need_disk {
        let row =
            crate::services::membership::query_disk_balance(ext_pool, user_id)
                .await
                .map_err(|e| {
                    tracing::error!(user_id = %user_id, error = %e, "云硬盘查询失败");
                    BuiltinError::Exec(format!("云硬盘查询失败: {e}"))
                })?;
        tracing::debug!(%user_id, ?row, "[query_balance] step1.2 disk: row");
        row // disk 可能为空（用户无云硬盘），不在这里报错
    } else {
        None
    };

    // 1.5 查询会员与订阅状态
    let membership_subscriptions =
        crate::services::membership::query_membership_subscriptions(ext_pool, user_id)
            .await
            .map_err(|e| {
                tracing::error!(user_id = %user_id, error = %e, "会员订阅查询失败");
                BuiltinError::Exec(format!("会员订阅查询失败: {e}"))
            })?;
    tracing::debug!(%user_id, count = membership_subscriptions.len(), "[query_balance] step1.5 membership: {} rows", membership_subscriptions.len());

    // Serialize membership+subscription rows to JSON
    let mut membership_json: Vec<Value> = membership_subscriptions
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

    if membership_json.is_empty() {
        membership_json.push(json!({ "level_name": "普通用户" }));
    }
    tracing::debug!(?membership_json, "[query_balance] step1.5 membership_json");

    // 1.8 查询时长卡（仅 duration_card / benefits 需要）
    let duration_cards = if need_duration {
        let cards =
            crate::services::membership::query_duration_cards(ext_pool, user_id)
                .await
                .map_err(|e| {
                    tracing::error!(user_id = %user_id, error = %e, "时长卡查询失败");
                    BuiltinError::Exec(format!("时长卡查询失败: {e}"))
                })?;
        tracing::debug!(%user_id, count = cards.len(), "[query_balance] step1.8 duration_cards: {} rows", cards.len());
        cards
    } else {
        Vec::new()
    };

    let mut duration_card_json: Vec<Value> = duration_cards
        .iter()
        .map(|row| {

            json!({
                "card_asset_id": row.card_asset_id,
                "remain_duration": row.remain_duration,
                "computer_biz_type": row.computer_biz_type,
                "expire_time": row.expire_time,
                "order_id": row.order_id,
                "consume_label": row.consume_label,
                "create_time": row.create_time.map(|t| t.to_string()),
                "game_label_list": row.game_label_list,
                "product_title": row.product_title,
                "product_duration": row.product_duration
            })
        })
        .collect();
    tracing::debug!(
        ?duration_card_json,
        "[query_balance] step1.8 duration_card_json"
    );

    // Resolve game_label_list codes to human-readable names via cc_label
    if need_duration && !duration_card_json.is_empty() {
        if let Err(e) = crate::services::membership::resolve_game_label_names(
            ext_pool,
            &mut duration_card_json,
        )
        .await
        {
            tracing::warn!(error = %e, "[query_balance] resolve_game_label_names failed");
        }
    }

    let has_membership = !membership_subscriptions.is_empty();
    let now = chrono::Utc::now().naive_utc();
    let expiring_soon = membership_subscriptions
        .iter()
        .filter(|row| row.effective_end_time.map_or(false, |end| end >= now))
        .any(|row| {
            row.effective_end_time
                .map(|end| (end - now).num_days() <= 7)
                .unwrap_or(false)
        });

    let upgrade_suggested = has_membership
        && !membership_subscriptions
            .iter()
            .any(|m| m.membership_level.as_deref() == Some("LEGEND"));
    tracing::debug!(
        has_membership,
        expiring_soon,
        upgrade_suggested,
        "[query_balance] step2: card state"
    );

    // 2. 构造返回结果
    let mut result = json!({});
    let reply = {
        let total_coins = coins_row
            .as_ref()
            .and_then(|r| r.total_coins)
            .unwrap_or(Decimal::ZERO)
            .to_f64()
            .unwrap_or(0.0);
        let expire_coins_7d = coins_row
            .as_ref()
            .and_then(|r| r.expire_coins_7d)
            .unwrap_or(Decimal::ZERO)
            .to_f64()
            .unwrap_or(0.0);
        let disk_end_time = disk_row.as_ref().and_then(|r| r.disk_end_time).unwrap_or(0);
        let disk_total_size = disk_row
            .as_ref()
            .and_then(|r| r.disk_total_size)
            .and_then(|s| s.to_f64())
            .unwrap_or(0.0);
        let disk_status = disk_row
            .as_ref()
            .and_then(|r| r.disk_status.clone())
            .unwrap_or_default();
        json!({
            "total_coins": total_coins,
            "expire_coins_7d": expire_coins_7d,
            "disk_end_time": disk_end_time,
            "disk_total_size": disk_total_size,
            "disk_status": disk_status,
        })
    };
    tracing::debug!(?reply, "[query_balance] step3: reply");

    if let Value::Object(ref mut map) = result {
        // 构造扩展卡片
        let mut extension_list: Vec<Value> = Vec::new();

        // 根据 category 构建 payload：只包含该类别需要的字段
        let build_payload = || -> Value {
            let card_type = if !has_membership {
                "subscribe"
            } else if expiring_soon {
                "repay"
            } else if upgrade_suggested {
                "upgrade"
            } else {
                "sufficient"
            };
            tracing::debug!(card_type, "[query_balance] step4: card_type");

            let mut payload = json!({"type": card_type});
            if let Value::Object(ref mut p) = payload {
                match category {
                    "membership" => {
                        p.insert("membership".into(), json!(membership_json));
                    }
                    "coins" => {
                        p.insert(
                            "info".into(),
                            json!({
                                "total_coins": reply.get("total_coins"),
                                "expire_coins_7d": reply.get("expire_coins_7d"),
                            }),
                        );
                    }
                    "duration_card" => {
                        p.insert("duration_card".into(), json!(duration_card_json));
                    }
                    "disk" => {
                        p.insert(
                            "info".into(),
                            json!({
                                "disk_end_time": reply.get("disk_end_time"),
                                "disk_total_size": reply.get("disk_total_size"),
                                "disk_status": reply.get("disk_status"),
                            }),
                        );
                    }
                    _ => {
                        // "benefits" 或未指定 → 保持全部信息
                        p.insert("info".into(), reply);
                        p.insert("membership".into(), json!(membership_json));
                        p.insert("duration_card".into(), json!(duration_card_json));
                    }
                }
            }
            payload
        };

        let payload = build_payload();
        tracing::debug!(?payload, "[query_balance] step4: payload");
        extension_list.push(json!({
            "content_type": "card",
            "category": category,
            "payload": payload,
        }));

        // Build _agent_context_updates with extensions (if any) and loop-break signal
        let mut updates = json!({
            "metadata": {
                "agent_loop_break": "false"
            }
        });
        if !extension_list.is_empty() {
            updates["extensions"] = json!(extension_list);
        }
        map.insert("_agent_context_updates".into(), updates);
    }

    tracing::debug!(?result, "[query_balance] step5: final result");

    Ok(result)
}

pub const QUERY_BALANCE_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "category": {
        "type": "string",
        "description": "查询余额的类别, 可选值: membership, coins, duration_card, disk, benefits, discount"
    }
  }
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
