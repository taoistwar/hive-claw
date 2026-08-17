use std::time::Duration;

use axum::{Router, extract::Query, routing::get};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::runtime::builtins::rag_answer::RetrievalRequest;
use crate::services::ragflow_config::RagflowConfig;
use crate::utils::error::{ApiResponse, AppError};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/knowledge-query", get(search))
        .route("/knowledge-query/defaults", get(defaults))
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
    rerank_id: Option<String>,
    #[serde(default)]
    keyword: Option<bool>,
    #[serde(default)]
    highlight: Option<bool>,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

#[derive(Debug, Serialize)]
struct SearchDefaults {
    page: u32,
    page_size: u32,
    similarity_threshold: f32,
    vector_similarity_weight: f32,
    top_k: u32,
    rerank_id: Option<String>,
    keyword: bool,
    highlight: bool,
    timeout_secs: u64,
}

impl From<&RagflowConfig> for SearchDefaults {
    fn from(config: &RagflowConfig) -> Self {
        Self {
            page: config.page,
            page_size: config.page_size,
            similarity_threshold: config.similarity_threshold,
            vector_similarity_weight: config.vector_similarity_weight,
            top_k: config.top_k,
            rerank_id: config.rerank_id.clone(),
            keyword: config.keyword,
            highlight: config.highlight,
            timeout_secs: config.timeout_secs,
        }
    }
}

#[derive(Debug)]
struct ResolvedSearchQuery {
    request: RetrievalRequest,
    page: u32,
    page_size: u32,
    timeout_secs: u64,
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

async fn defaults() -> ApiResponse<SearchDefaults> {
    ApiResponse::success(SearchDefaults::from(&RagflowConfig::current()))
}

async fn search(
    Query(q): Query<SearchQuery>,
) -> Result<ApiResponse<SearchResponse>, ApiResponse<()>> {
    let config = RagflowConfig::current();
    let resolved = resolve_search_query(q, &config).map_err(|error| error.into_response())?;
    let page = resolved.page;
    let page_size = resolved.page_size;

    let (items, total) = query_rag(&config, resolved.request, resolved.timeout_secs).await;

    Ok(ApiResponse::success(SearchResponse {
        items,
        total,
        page,
        page_size,
        has_knowledge: total > 0,
    }))
}

fn resolve_search_query(
    query: SearchQuery,
    config: &RagflowConfig,
) -> Result<ResolvedSearchQuery, AppError> {
    let question = query.question.trim();
    if question.is_empty() {
        return Err(AppError::BadRequest("question 不能为空".into()));
    }

    let page = query.page.unwrap_or(config.page);
    if page == 0 {
        return Err(AppError::BadRequest("page 必须大于 0".into()));
    }

    let page_size = query.page_size.unwrap_or(config.page_size);
    if !(1..=100).contains(&page_size) {
        return Err(AppError::BadRequest(
            "page_size 必须在 1 到 100 之间".into(),
        ));
    }

    let similarity_threshold = query
        .similarity_threshold
        .unwrap_or(config.similarity_threshold);
    validate_unit_interval("similarity_threshold", similarity_threshold)?;

    let vector_similarity_weight = query
        .vector_similarity_weight
        .unwrap_or(config.vector_similarity_weight);
    validate_unit_interval("vector_similarity_weight", vector_similarity_weight)?;

    let top_k = query.top_k.unwrap_or(config.top_k);
    if top_k == 0 {
        return Err(AppError::BadRequest("top_k 必须大于 0".into()));
    }

    let timeout_secs = query.timeout_secs.unwrap_or(config.timeout_secs);
    if timeout_secs == 0 {
        return Err(AppError::BadRequest("timeout_secs 必须大于 0".into()));
    }

    let dataset_ids = query
        .dataset_ids
        .as_deref()
        .map(split_csv)
        .unwrap_or_else(|| config.dataset_ids.clone());

    let document_ids = query
        .document_ids
        .as_deref()
        .map(split_csv)
        .unwrap_or_else(|| config.document_ids.clone());

    let mut request = RetrievalRequest::from_config(question.to_string(), config);
    request.dataset_ids = dataset_ids;
    request.document_ids = document_ids;
    request.page = Some(page);
    request.page_size = Some(page_size);
    request.similarity_threshold = Some(similarity_threshold);
    request.vector_similarity_weight = Some(vector_similarity_weight);
    request.top_k = Some(top_k);
    request.rerank_id = match query.rerank_id {
        Some(value) => {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        }
        None => config.rerank_id.clone(),
    };
    request.keyword = Some(query.keyword.unwrap_or(config.keyword));
    request.highlight = Some(query.highlight.unwrap_or(config.highlight));

    Ok(ResolvedSearchQuery {
        request,
        page,
        page_size,
        timeout_secs,
    })
}

fn validate_unit_interval(name: &str, value: f32) -> Result<(), AppError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(AppError::BadRequest(format!("{name} 必须在 0 到 1 之间")))
    }
}

fn split_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .map(ToString::to_string)
        .collect()
}

async fn query_rag(
    config: &RagflowConfig,
    request: RetrievalRequest,
    timeout_secs: u64,
) -> (Vec<KnowledgeChunk>, u32) {
    if config.base_url.is_empty() {
        tracing::warn!("RAGFlow base URL 未配置，返回空结果");
        return (Vec::new(), 0);
    }
    if config.api_key.is_empty() {
        tracing::warn!("RAGFlow API key 未配置，返回空结果");
        return (Vec::new(), 0);
    }

    let client = match Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(error = %err, "rag query client create failed");
            return (Vec::new(), 0);
        }
    };

    let url = format!("{}/api/v1/retrieval", config.base_url.trim_end_matches('/'));

    let response = match client
        .post(url)
        .header("content-type", "application/json")
        .bearer_auth(&config.api_key)
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{SearchDefaults, SearchQuery, resolve_search_query};
    use crate::services::ragflow_config::RagflowConfig;

    fn global_config() -> RagflowConfig {
        RagflowConfig {
            base_url: "https://ragflow.example".to_string(),
            api_key: "test-key".to_string(),
            dataset_ids: vec!["dataset-from-global".to_string()],
            document_ids: vec!["document-from-global".to_string()],
            page: 2,
            page_size: 6,
            similarity_threshold: 0.2,
            vector_similarity_weight: 0.3,
            top_k: 10,
            rerank_id: Some("reranker-from-global".to_string()),
            keyword: true,
            highlight: false,
            timeout_secs: 30,
        }
    }

    #[test]
    fn editable_defaults_are_derived_from_resolved_global_config() {
        let defaults = SearchDefaults::from(&global_config());

        assert_eq!(
            serde_json::to_value(defaults).unwrap(),
            json!({
                "page": 2,
                "page_size": 6,
                "similarity_threshold": 0.2_f32,
                "vector_similarity_weight": 0.3_f32,
                "top_k": 10,
                "rerank_id": "reranker-from-global",
                "keyword": true,
                "highlight": false,
                "timeout_secs": 30
            })
        );
    }

    #[test]
    fn every_debug_parameter_overrides_its_global_default() {
        let resolved = resolve_search_query(
            SearchQuery {
                question: "如何重置密码？".to_string(),
                page: Some(4),
                page_size: Some(20),
                dataset_ids: None,
                document_ids: None,
                similarity_threshold: Some(0.45),
                vector_similarity_weight: Some(0.65),
                top_k: Some(25),
                rerank_id: Some("debug-reranker".to_string()),
                keyword: Some(false),
                highlight: Some(true),
                timeout_secs: Some(45),
            },
            &global_config(),
        )
        .unwrap();

        assert_eq!(resolved.page, 4);
        assert_eq!(resolved.page_size, 20);
        assert_eq!(resolved.timeout_secs, 45);
        assert_eq!(resolved.request.page, Some(4));
        assert_eq!(resolved.request.page_size, Some(20));
        assert_eq!(resolved.request.similarity_threshold, Some(0.45));
        assert_eq!(resolved.request.vector_similarity_weight, Some(0.65));
        assert_eq!(resolved.request.top_k, Some(25));
        assert_eq!(
            resolved.request.rerank_id.as_deref(),
            Some("debug-reranker")
        );
        assert_eq!(resolved.request.keyword, Some(false));
        assert_eq!(resolved.request.highlight, Some(true));
    }

    #[test]
    fn invalid_debug_ranges_are_rejected_at_the_http_boundary() {
        let result = resolve_search_query(
            SearchQuery {
                question: "如何重置密码？".to_string(),
                page: Some(1),
                page_size: Some(101),
                dataset_ids: None,
                document_ids: None,
                similarity_threshold: Some(1.1),
                vector_similarity_weight: Some(0.3),
                top_k: Some(10),
                rerank_id: None,
                keyword: Some(true),
                highlight: Some(false),
                timeout_secs: Some(30),
            },
            &global_config(),
        );

        assert!(result.is_err());
    }
}
