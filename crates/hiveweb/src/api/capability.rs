//! Capability API handler — GET /api/capabilities
//!
//! 只读端点：返回宿主 CapabilityRegistry 中的静态列表 + is_dangerous 标记
//! 供前端 Agent 编辑器 CapabilityPicker 渲染。

use axum::{
    extract::{Path, Request, State},
    routing::get,
    Router,
};
use serde::Serialize;

use crate::api::AppState;
use crate::runtime::capability::Capability;
use crate::utils::error::ApiResponse;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/capabilities", get(list_capabilities))
        .route("/capabilities/*name", get(get_capability))
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

#[derive(Debug, Serialize)]
pub struct CapabilityDetail {
    pub name: String,
    pub description: String,
    pub is_dangerous: bool,
}

async fn get_capability(
    State(state): State<AppState>,
    req: Request,
) -> ApiResponse<CapabilityDetail> {
    let path = req.uri().path();
    let name = path
        .strip_prefix("/capabilities/")
        .unwrap_or(path)
        .to_string();

    let registry = &state.runtime_state.capabilities;
    match registry.lookup(&name) {
        Some(cap) => ApiResponse::success(CapabilityDetail {
            name: cap.name.to_string(),
            description: cap.description.to_string(),
            is_dangerous: cap.is_dangerous,
        }),
        None => ApiResponse::<CapabilityDetail>::err(4040, format!("Capability '{}' not found", name)),
    }
}
