//! Capability API handler — GET /api/capabilities
//!
//! 只读端点：返回宿主 CapabilityRegistry 中的静态列表 + is_dangerous 标记
//! 供前端 Agent 编辑器 CapabilityPicker 渲染。

use axum::{extract::State, routing::get, Router};
use serde::Serialize;

use crate::api::AppState;
use crate::runtime::capability::Capability;
use crate::utils::error::ApiResponse;

pub fn router() -> Router<AppState> {
    Router::new().route("/capabilities", get(list_capabilities))
}

#[derive(Debug, Serialize)]
pub struct CapabilityItem {
    pub name: &'static str,
    pub description: &'static str,
    pub is_dangerous: bool,
}

impl From<&Capability> for CapabilityItem {
    fn from(c: &Capability) -> Self {
        Self {
            name: c.name,
            description: c.description,
            is_dangerous: c.is_dangerous,
        }
    }
}

async fn list_capabilities(
    State(state): State<AppState>,
) -> ApiResponse<Vec<CapabilityItem>> {
    let items: Vec<CapabilityItem> = state
        .runtime_state
        .capabilities
        .all()
        .iter()
        .map(Into::into)
        .collect();
    ApiResponse::success(items)
}
