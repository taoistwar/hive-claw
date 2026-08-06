use std::env;
use std::time::Duration;

use axum::{extract::{Query, State}, routing::get, Router};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::runtime::builtins::rag_answer::{
    env_list,
    env_num,
    RetrievalRequest,
    DEFAULT_RAGFLOW_PAGE_SIZE,
    DEFAULT_RAGFLOW_TIMEOUT_SECS,
    DEFAULT_RAGFLOW_TOP_K,
    ENV_RAGFLOW_API_KEY,
    ENV_RAGFLOW_BASE_URL,
    ENV_RAGFLOW_DATASET_IDS,
    ENV_RAGFLOW_DOCUMENT_IDS,
    ENV_RAGFLOW_TOP_K,
};
use crate::utils::error::{ApiResponse, AppError};

pub fn router() -> Router<AppState> {
    Router::new().route("/knowledge-query", get(search))
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    question: String,
    #[serde(default)]
    page: Option<u32>,
    #[serde(default)]
    page_size: Option<u32>,
    #[serde(default)]
    dataset_ids: Option<String>,
    #[serde(default)]
    document_ids: Option<String>,
    #[serde(default)]
    similarity_threshold: Option<f32>,
    #[serde(default)]
    vector_similarity_weight: Option<f32>,
    #[serde(default)]
    top_k: Option<u32>,
    #[serde(default)]
    keyword: Option<bool>,
    #[serde(default)]
    highlight: Option<bool>,
}

#[derive(Debug, Serialize)]
struct SearchResponse {
    items: Vec<KnowledgeChunk>,
    total: u32,
    page: u32,
    page_size: u32,
    has_knowledge: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct KnowledgeChunk {
    id: String,
    content: String,
    content_ltks: Option<String>,
    dataset_id: Option<String>,
    document_id: Option<String>,
    document_keyword: Option<String>,
    highlight: Option<String>,
    image_id: Option<String>,
    important_keywords: Option<Vec<String>>,
    positions: Option<Vec<serde_json::Value>>,
    similarity: Option<f32>,
    term_similarity: Option<f32>,
    vector_similarity: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct RagflowResponse {
    code: i32,
    data: Option<RagflowResponseData>,
}

#[derive(Debug, Deserialize)]
struct RagflowResponseData {
    chunks: Vec<KnowledgeChunk>,
    #[serde(default)]
    total: Option<u32>,
}

async fn search(
    Query(q): Query<SearchQuery>,
    State(_): State<AppState>,
) -> Result<ApiResponse<SearchResponse>, ApiResponse<()>> {
    let question = q.question.trim();
    if question.is_empty() {
        return Err(AppError::BadRequest("question 不能为空".into()).into_response());
    }

    let page = q.page.unwrap_or(1).max(1);
    let page_size = q
        .page_size
        .unwrap_or(DEFAULT_RAGFLOW_PAGE_SIZE)
        .max(1)
        .min(100);

    let dataset_ids = q
        .dataset_ids
        .as_deref()
        .map(split_csv)
        .unwrap_or_else(|| env_list(ENV_RAGFLOW_DATASET_IDS));

    let document_ids = q
        .document_ids
        .as_deref()
        .map(split_csv)
        .unwrap_or_else(|| env_list(ENV_RAGFLOW_DOCUMENT_IDS));

    let (items, total) = query_rag(
        question,
        dataset_ids,
        document_ids,
        page,
        page_size,
        q.similarity_threshold,
        q.vector_similarity_weight,
        q.top_k,
        q.keyword,
        q.highlight,
    )
    .await;

    Ok(ApiResponse::success(SearchResponse {
        items,
        total,
        page,
        page_size,
        has_knowledge: total > 0,
    }))
}

fn split_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .map(ToString::to_string)
        .collect()
}

async fn query_rag(
    question: &str,
    dataset_ids: Vec<String>,
    document_ids: Vec<String>,
    page: u32,
    page_size: u32,
    similarity_threshold: Option<f32>,
    vector_similarity_weight: Option<f32>,
    top_k: Option<u32>,
    keyword: Option<bool>,
    highlight: Option<bool>,
) -> (Vec<KnowledgeChunk>, u32) {
    let base_url = match env::var(ENV_RAGFLOW_BASE_URL) {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => {
            tracing::warn!("RAGFLOW_BASE_URL 未配置，返回空结果");
            return (Vec::new(), 0);
        }
    };

    let api_key = match env::var(ENV_RAGFLOW_API_KEY) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            tracing::warn!("RAGFLOW_API_KEY 未配置，返回空结果");
            return (Vec::new(), 0);
        }
    };

    let request = RetrievalRequest {
        question: question.to_string(),
        dataset_ids,
        document_ids,
        page: Some(page),
        page_size: Some(page_size),
        similarity_threshold: Some(similarity_threshold.unwrap_or(0.2)),
        vector_similarity_weight: Some(vector_similarity_weight.unwrap_or(0.3)),
        top_k: Some(top_k.unwrap_or_else(|| env_num(ENV_RAGFLOW_TOP_K).unwrap_or(DEFAULT_RAGFLOW_TOP_K))),
        rerank_id: None,
        keyword: Some(keyword.unwrap_or(false)),
        highlight: Some(highlight.unwrap_or(false)),
    };

    let client = match Client::builder()
        .timeout(Duration::from_secs(DEFAULT_RAGFLOW_TIMEOUT_SECS))
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(error = %err, "rag query client create failed");
            return (Vec::new(), 0);
        }
    };

    let url = format!("{}/api/v1/retrieval", base_url.trim_end_matches('/'));

    let response = match client
        .post(url)
        .header("content-type", "application/json")
        .bearer_auth(api_key)
        .json(&request)
        .send()
        .await
    {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(error = %err, "rag query request failed");
            return (Vec::new(), 0);
        }
    };

    if !response.status().is_success() {
        tracing::warn!(status = %response.status(), "rag query returned non-2xx");
        return (Vec::new(), 0);
    }

    let payload = match response.json::<RagflowResponse>().await {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(error = %err, "rag query decode failed");
            return (Vec::new(), 0);
        }
    };

    if payload.code != 0 {
        tracing::warn!(code = payload.code, "rag query returned non-zero code");
        return (Vec::new(), 0);
    }

    let Some(data) = payload.data else {
        return (Vec::new(), 0);
    };

    let total = data.total.unwrap_or(0);
    let items = data
        .chunks
        .into_iter()
        .map(|chunk| KnowledgeChunk {
            id: chunk.id,
            content: if chunk.content.is_empty() {
                String::from("—")
            } else {
                chunk.content
            },
            content_ltks: chunk.content_ltks,
            dataset_id: chunk.dataset_id,
            document_id: chunk.document_id,
            document_keyword: chunk.document_keyword,
            highlight: chunk.highlight,
            image_id: chunk.image_id,
            important_keywords: chunk.important_keywords,
            positions: chunk.positions,
            similarity: chunk.similarity,
            term_similarity: chunk.term_similarity,
            vector_similarity: chunk.vector_similarity,
        })
        .collect();

    (items, total)
}
