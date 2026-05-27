//! Plugin API handlers (T071 / US1)
//!
//! GET / POST(multipart) / PUT / DELETE /api/plugins[/:id]
//! GET /api/plugins/:id/download — 下载 WASM 文件（二进制流）

use axum::{
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::header::{CONTENT_DISPOSITION, CONTENT_TYPE},
    response::Response,
    routing::get,
    Json, Router,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::services::plugin::{
    self as svc, ListFilter, UpdateMeta, UploadMeta,
};
use crate::storage::s3::get_wasm;
use crate::utils::error::{ApiResponse, AppError};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/plugins", get(list_plugins).post(upload_plugin))
        .route(
            "/plugins/:id",
            get(get_plugin).put(update_plugin).delete(delete_plugin),
        )
        .route("/plugins/:id/download", get(download_plugin))
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
    #[serde(default)]
    pub include_deleted: Option<bool>,
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
        include_deleted: q.include_deleted.unwrap_or(false),
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
        Ok(p) => Ok(ApiResponse::success(p)),
        Err(e) => Err(e.into_response()),
    }
}

async fn update_plugin(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(meta): Json<UpdateMeta>,
) -> Result<ApiResponse<crate::models::Plugin>, ApiResponse<()>> {
    match svc::update(&state.pool, id, meta).await {
        Ok(p) => Ok(ApiResponse::success(p)),
        Err(e) => Err(e.into_response()),
    }
}

async fn delete_plugin(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<ApiResponse<()>, ApiResponse<()>> {
    match svc::soft_delete(&state.pool, id).await {
        Ok(()) => Ok(ApiResponse::success(())),
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
