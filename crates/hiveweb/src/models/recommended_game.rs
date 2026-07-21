//! Data model for the legacy recommended-game feature.
//!
//! Retained to decode existing rows and serve compatibility APIs.

#![allow(deprecated)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[deprecated(note = "Legacy recommended-game model; retained for compatibility only")]
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct RecommendedGame {
    // 推荐表ID
    pub id: i64,
    // 推荐列表时显示的标签
    pub tag: Option<String>,
    // 推荐列表时显示的名称
    pub name: String,
    // assistant 回复文本
    pub reply: String,
    // 游戏卡片：推荐理由
    pub reason: Option<String>,
    // 游戏卡片：分类
    pub game_category: Option<serde_json::Value>,
    // 游戏卡片：游戏封面
    pub game_image: Option<String>,
    pub sort_value: i32,
    // 游戏卡片：游戏id
    pub game_id: String,
    // 游戏卡片：游戏名称
    pub game_name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
