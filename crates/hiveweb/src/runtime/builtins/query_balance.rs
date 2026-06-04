//! Builtin function implementations (T079 / FR-010 v5)
//!
//! 5 个不依赖 WASM 的"胶水"函数，启动期 upsert 到 `functions` 表（kind=1）。
//! 调用入口：当 orchestrator 选中 kind=1 Tool 时直接走宿主代码，绕过 Plugin invoker。
//!
//! 内置不可删除；可被禁用（disabled 字段暂未引入 — 后续 schema 扩展时加）。

use regex::Regex;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::MySqlPool;
use std::sync::Arc;

use agent::context::AgentContext;

// ---------- query_balance ----------

use serde_json::json;

use crate::runtime::builtins::BuiltinContext;
use crate::runtime::builtins::BuiltinError;
use crate::runtime::builtins::BuiltinResult;

/// sync wrapper for query_balance — bridges async DB queries inside tokio runtime.
/// user_id is extracted from AgentContext, not from LLM args.
pub fn query_balance(_args: Value, ctx: &BuiltinContext) -> BuiltinResult {
    let agent_ctx = ctx
        .agent_ctx
        .as_ref()
        .ok_or_else(|| BuiltinError::Exec("AgentContext 不可用".into()))?;
    let user_id: i64 = agent_ctx
        .get_user_metadata("actor_id")
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| BuiltinError::Exec("无法从上下文获取用户ID".into()))?;
    let ext_pool = ctx
        .ext_pool
        .ok_or_else(|| BuiltinError::Exec("外部数据库未配置".into()))?
        .clone();
    let agent_ctx_clone = ctx.agent_ctx.clone();

    tokio::task::block_in_place(move || {
        tokio::runtime::Handle::current().block_on(async move {
            query_balance_async_impl(user_id, &ext_pool, agent_ctx_clone.as_deref()).await
        })
    })
}

pub async fn query_balance_async_impl(
    user_id: i64,
    ext_pool: &sqlx::MySqlPool,
    _agent_ctx: Option<&AgentContext>,
) -> BuiltinResult {
    // 1. 先查询会员等级
    #[derive(Debug, sqlx::FromRow)]
    #[allow(dead_code)]
    struct MembershipRow {
        total_coins: Option<Decimal>,
        expire_coins_7d: Option<Decimal>,
        effective_end_time: Option<chrono::NaiveDateTime>,
        membership_category: Option<String>,
        level_name: Option<String>,
        disk_total_size: Option<Decimal>,
        avg_daily_coin: Option<Decimal>,
        play_times_7d: Option<i64>,
    }

    let membership: Option<MembershipRow> = sqlx::query_as(
            r#"select
	m7.total_coins, m2.expire_coins_7d, m3.effective_end_time, m3.membership_category, m3.level_name,m4.disk_total_size,m5.avg_daily_coin,m6.play_times_7d
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
	select user_id, effective_end_time,membership_category, level_name from (
	    select
	        t1.user_id as user_id, t1.effective_end_time as  effective_end_time, t1.membership_category as membership_category,
	        t2.level_order as level_order, t2.level_name as level_name
	    from (
	        select user_id, membership_level, effective_end_time,membership_category from cc_user_membership
	        where user_id = ?
	        and effective_end_time > now()
	    ) t1 left join cc_membership_level t2
	    on t1.membership_level =t2.level_code
	) t3
	order by t3.level_order desc
	limit 1
) m3 on m1.user_id = m3.user_id
left JOIN(
	SELECT
		user_id,
		sum(size/1024/1024/1024) as disk_total_size
	from cc_user_disk
	where user_id=?
	and end_time > UNIX_TIMESTAMP()*1000
	group by user_id
) m4 on m1.user_id = m4.user_id
LEFT JOIN (
	SELECT
	    user_id,
	        CASE
	        WHEN COUNT(DISTINCT DATE(create_time)) > 0
	        THEN ROUND(COALESCE(SUM(value), 0) / COUNT(DISTINCT DATE(create_time)), 2)
	        ELSE 0
	        END AS avg_daily_coin
	FROM cc_order_consume_detail
	WHERE user_id = ?
	  AND create_time BETWEEN DATE_SUB(NOW(), INTERVAL 7 DAY) AND now()
	  AND value > 0
	group by user_id
) m5 on m1.user_id = m5.user_id
LEFT JOIN (
	SELECT
		user_id, count(1) as play_times_7d
	FROM cc_game_history
	WHERE user_id = ?
	  AND create_time > DATE_SUB(NOW(), INTERVAL 7 DAY) AND now()
	GROUP BY user_id
) m6 on m1.user_id = m6.user_id
LEFT JOIN (
	select user_id, IFNULL(sum(value), 0) as total_coins from cc_user_asset_coin where user_id = ? and expire_time > UNIX_TIMESTAMP() and value>0
	group by user_id
) m7 on m1.user_id = m7.user_id
 "#,
        )
        .bind(user_id)
        .bind(user_id)
        .bind(user_id)
        .bind(user_id)
        .bind(user_id)
        .bind(user_id)
        .bind(user_id)
        .fetch_optional(ext_pool)
        .await
        .map_err(|e| BuiltinError::Exec(format!("会员查询失败: {e}")))?;

    if membership.is_none() {
        return Ok(json!({
            "message": "暂无该用户的资产数据，请稍后再试"
        }));
    }

    let has_membership = membership
        .as_ref()
        .and_then(|m| m.effective_end_time)
        .is_some();
    let days_until_expiry = membership
        .as_ref()
        .and_then(|m| m.effective_end_time)
        .map(|end| (end - chrono::Utc::now().naive_utc()).num_days());
    let expiring_soon = days_until_expiry.map(|d| d <= 7).unwrap_or(false);

    let upgrade_suggested = has_membership
        && membership
            .as_ref()
            .and_then(|m| m.membership_category.as_deref())
            != Some("LEGEND");

    let total_coins = membership
        .as_ref()
        .and_then(|m| m.total_coins)
        .unwrap_or(Decimal::ZERO);

    let coins_low = total_coins < Decimal::from(500);
    let play_times_7d = membership
        .as_ref()
        .and_then(|m| m.play_times_7d)
        .unwrap_or(0);

    // 2. 构造返回结果
    let mut result = json!({});
    let m = membership.unwrap();
    let reply = json!({
        "effective_end_time": m.effective_end_time.map(|t| t.to_string()).unwrap_or_default(),
        "membership_category": m.membership_category.as_deref().unwrap_or(""),
        "level_name": m.level_name.as_deref().unwrap_or(""),
        "total_coins": m.total_coins.unwrap_or(Decimal::ZERO).to_f64().unwrap_or(0.0),
        "avg_daily_coin": m.avg_daily_coin.unwrap_or(Decimal::ZERO).to_f64().unwrap_or(0.0),
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
                    "type": "goPay",
                    "info":reply,
                },
            }));
        } else if coins_low {
            // 金币不足（余额 < 500）→ nowPay 卡
            extension_list.push(json!({
                "content_type": "card",
                "payload": {
                    "type": "subscribe",
                    "info":reply,
                },
            }));
        } else if expiring_soon {
            // 会员即将到期（≤ 7 天）→ repay 卡
            let days_left = days_until_expiry.unwrap_or(0);
            let text = format!("会员将于 {} 天后到期", days_left.max(0));
            extension_list.push(json!({
                "content_type": "card",
                "payload": {"type": "repay", "reply":reply,},
            }));
        } else if upgrade_suggested {
            // 升级建议卡
            extension_list.push(json!({
                "content_type": "card",
                "payload": {"type": "upgrade", "info":reply,},
            }));
        } else {
            extension_list.push(json!({
                "content_type": "card",
                "payload": {"type": "sufficient", "info":reply,},
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

    println!("HOP tool call result: {:?}", result);

    Ok(result)
}
