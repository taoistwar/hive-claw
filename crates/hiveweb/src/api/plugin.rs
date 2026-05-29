//! Plugin API handlers (T071 / US1)
//!
//! GET / POST(multipart) / PUT / DELETE /api/plugins[/:id]
//! GET /api/plugins/:id/download — 下载 WASM 文件（二进制流）

use axum::{
    body::Body,
    extract::{Extension, Multipart, Path, Query, State},
    http::header::{CONTENT_DISPOSITION, CONTENT_TYPE},
    response::Response,
    routing::get,
    Json, Router,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::services::audit::{self as audit_svc, Operation};
use crate::services::plugin::{
    self as svc, ListFilter, UpdateMeta, UploadMeta,
};
use crate::storage::s3::get_wasm;
use crate::utils::error::{ApiResponse, AppError};
use crate::utils::jwt::Claims;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/plugins", get(list_plugins).post(upload_plugin))
        .route(
            "/plugins/:id",
            get(get_plugin).put(update_plugin).delete(delete_plugin),
        )
        .route("/plugins/:id/download", get(download_plugin))
        .route("/plugins/:id/exports", get(list_plugin_exports))
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub category_id: Option<i64>,
    /// 逗号分隔 tag id
    #[serde(default)]
    pub tag_ids: Option<String>,
    /// true=回收站，false/missing=只看未删除（默认）
    #[serde(default)]
    pub deleted_only: Option<bool>,
    #[serde(default)]
    pub identifier: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub repository_url: Option<String>,
    #[serde(default)]
    pub created_at_start: Option<String>,
    #[serde(default)]
    pub created_at_end: Option<String>,
    #[serde(default)]
    pub updated_at_start: Option<String>,
    #[serde(default)]
    pub updated_at_end: Option<String>,
}

async fn list_plugins(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<ApiResponse<svc::PluginList>, ApiResponse<()>> {
    let tag_ids: Vec<i64> = q
        .tag_ids
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let offset = q.offset.unwrap_or(0).max(0);
    let filter = ListFilter {
        offset,
        limit,
        search: q.search.filter(|s| !s.is_empty()),
        category_id: q.category_id,
        tag_ids,
        deleted_only: q.deleted_only.unwrap_or(false),
        identifier: q.identifier.filter(|s| !s.is_empty()),
        name: q.name.filter(|s| !s.is_empty()),
        description: q.description.filter(|s| !s.is_empty()),
        runtime: q.runtime.filter(|s| !s.is_empty()),
        version: q.version.filter(|s| !s.is_empty()),
        author: q.author.filter(|s| !s.is_empty()),
        repository_url: q.repository_url.filter(|s| !s.is_empty()),
        created_at_start: q.created_at_start.filter(|s| !s.is_empty()),
        created_at_end: q.created_at_end.filter(|s| !s.is_empty()),
        updated_at_start: q.updated_at_start.filter(|s| !s.is_empty()),
        updated_at_end: q.updated_at_end.filter(|s| !s.is_empty()),
    };
    match svc::list(&state.pool, filter).await {
        Ok(list) => Ok(ApiResponse::success(list)),
        Err(e) => Err(e.into_response()),
    }
}

async fn get_plugin(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<crate::models::Plugin>, ApiResponse<()>> {
    match svc::fetch_by_id(&state.pool, id).await {
        Ok(p) => Ok(ApiResponse::success(p)),
        Err(e) => Err(e.into_response()),
    }
}

async fn upload_plugin(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    mut multipart: Multipart,
) -> Result<ApiResponse<crate::models::Plugin>, ApiResponse<()>> {
    let mut meta_json: Option<String> = None;
    let mut wasm_bytes: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart read: {e}")).into_response())?
    {
        match field.name() {
            Some("meta") => {
                let bytes = field.bytes().await.map_err(|e| {
                    AppError::BadRequest(format!("meta field read: {e}")).into_response()
                })?;
                meta_json = Some(String::from_utf8(bytes.to_vec()).map_err(|e| {
                    AppError::BadRequest(format!("meta not utf-8: {e}")).into_response()
                })?);
            }
            Some("file") => {
                let bytes = field.bytes().await.map_err(|e| {
                    AppError::BadRequest(format!("file field read: {e}")).into_response()
                })?;
                wasm_bytes = Some(bytes.to_vec());
            }
            _ => {}
        }
    }

    let meta_str = meta_json
        .ok_or_else(|| AppError::BadRequest("missing 'meta' field".into()).into_response())?;
    let bytes = wasm_bytes
        .ok_or_else(|| AppError::BadRequest("missing 'file' field".into()).into_response())?;
    let meta: UploadMeta = serde_json::from_str(&meta_str)
        .map_err(|e| AppError::BadRequest(format!("meta json: {e}")).into_response())?;

    match svc::upload(&state.pool, &state.s3, meta, bytes).await {
        Ok(plugin) => {
            if let Err(e) = audit_event(
                &state.pool,
                &claims,
                Operation::Create,
                Some(plugin.id),
                &plugin.identifier,
                serde_json::json!({ "name": plugin.name, "version": plugin.version }),
            )
            .await
            {
                tracing::error!("Failed to write audit log for plugin upload: {}", e);
            }
            Ok(ApiResponse::success(plugin))
        }
        Err(e) => Err(e.into_response()),
    }
}

async fn update_plugin(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
    Json(meta): Json<UpdateMeta>,
) -> Result<ApiResponse<crate::models::Plugin>, ApiResponse<()>> {
    match svc::update(&state.pool, id, meta).await {
        Ok(plugin) => {
            if let Err(e) = audit_event(
                &state.pool,
                &claims,
                Operation::Update,
                Some(plugin.id),
                &plugin.identifier,
                serde_json::json!({ "name": plugin.name, "version": plugin.version }),
            )
            .await
            {
                tracing::error!("Failed to write audit log for plugin update: {}", e);
            }
            Ok(ApiResponse::success(plugin))
        }
        Err(e) => Err(e.into_response()),
    }
}

async fn delete_plugin(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    let prev = svc::fetch_by_id(&state.pool, id).await.ok();
    let target_id = prev.as_ref().map(|p| p.id);
    let target_ident = prev.as_ref().map(|p| &p.identifier).cloned().unwrap_or_default();

    match svc::soft_delete(&state.pool, id).await {
        Ok(()) => {
            if let Err(e) = audit_event(
                &state.pool,
                &claims,
                Operation::Delete,
                target_id,
                &target_ident,
                serde_json::json!({}),
            )
            .await
            {
                tracing::error!("Failed to write audit log for plugin delete: {}", e);
            }
            Ok(ApiResponse::success(()))
        }
        Err(e) => Err(e.into_response()),
    }
}

async fn download_plugin(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response<Body>, ApiResponse<()>> {
    let plugin = svc::fetch_by_id(&state.pool, id)
        .await
        .map_err(|e| e.into_response())?;

    if plugin.deleted_at.is_some() {
        return Err(AppError::NotFound("Plugin has been deleted".into()).into_response());
    }

    let bytes = get_wasm(&state.s3, &plugin.s3_key)
        .await
        .map_err(|e| {
            AppError::Internal(format!("S3 get WASM: {e}")).into_response()
        })?;

    let filename = format!("{}-{}.wasm", plugin.identifier, plugin.version);
    let response = Response::builder()
        .header(CONTENT_TYPE, "application/wasm")
        .header(CONTENT_DISPOSITION, format!("attachment; filename=\"{filename}\""))
        .body(Body::from(bytes))
        .map_err(|e| {
            AppError::Internal(format!("build response: {e}")).into_response()
        })?;

    Ok(response)
}

#[derive(Debug, serde::Serialize)]
pub struct PluginExportsResp {
    pub exports: Vec<String>,
}

/// 从 S3 下载插件 WASM 并解析其导出函数名列表 (T092 扩展)
async fn list_plugin_exports(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<PluginExportsResp>, ApiResponse<()>> {
    let plugin = svc::fetch_by_id(&state.pool, id)
        .await
        .map_err(|e| e.into_response())?;

    if plugin.deleted_at.is_some() {
        return Err(AppError::NotFound("Plugin has been deleted".into()).into_response());
    }

    let bytes = get_wasm(&state.s3, &plugin.s3_key)
        .await
        .map_err(|e| {
            AppError::Internal(format!("S3 get WASM: {e}")).into_response()
        })?;

    let exports = crate::runtime::wasm_exports::extract_wasm_exports(&bytes)
        .map_err(|e| AppError::Internal(format!("WASM 解析失败: {e}")).into_response())?;

    Ok(ApiResponse::success(PluginExportsResp { exports }))
}

/// 写入审计日志。失败时记录 tracing 日志但不影响主流程。
async fn audit_event(
    pool: &sqlx::MySqlPool,
    claims: &Claims,
    op: Operation,
    target_id: Option<i64>,
    target_name: &str,
    detail: serde_json::Value,
) -> anyhow::Result<()> {
    use crate::services::admin as admin_svc;
    let operator = admin_svc::get_admin_by_id(pool, claims.admin_id).await?;
    let operator_phone = operator.map(|a| a.phone).unwrap_or_default();
    if let Err(e) = audit_svc::record(
        pool,
        claims.admin_id,
        &operator_phone,
        target_id,
        target_name,
        op,
        Some(detail),
    )
    .await
    {
        tracing::error!("Failed to write audit log: {}", e);
        return Err(e);
    }
    Ok(())
}
