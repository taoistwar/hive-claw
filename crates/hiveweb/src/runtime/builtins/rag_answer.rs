use std::sync::Arc;
use std::time::Duration;

use providers::{ChatRequest, RetryMode};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::runtime::builtins::{BuiltinContext, BuiltinResult};
use crate::services::ragflow_config::RagflowConfig;

#[derive(Debug, Serialize)]
pub struct RetrievalRequest {
    pub question: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dataset_ids: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub document_ids: Vec<String>,
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    pub similarity_threshold: Option<f32>,
    pub vector_similarity_weight: Option<f32>,
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerank_id: Option<String>,
    pub keyword: Option<bool>,
    pub highlight: Option<bool>,
}

impl RetrievalRequest {
    /// Builds a retrieval payload entirely from the resolved shared config.
    pub fn from_config(question: String, config: &RagflowConfig) -> Self {
        Self {
            question,
            dataset_ids: config.dataset_ids.clone(),
            document_ids: config.document_ids.clone(),
            page: Some(config.page),
            page_size: Some(config.page_size),
            similarity_threshold: Some(config.similarity_threshold),
            vector_similarity_weight: Some(config.vector_similarity_weight),
            top_k: Some(config.top_k),
            rerank_id: config.rerank_id.clone(),
            keyword: Some(config.keyword),
            highlight: Some(config.highlight),
        }
    }
}

#[derive(Debug, Deserialize)]
struct RetrievalResponse {
    code: i32,
    data: Option<RetrievalData>,
}

#[derive(Debug, Deserialize)]
struct RetrievalData {
    chunks: Vec<RetrievalChunk>,
}

#[derive(Debug, Deserialize)]
struct RetrievalChunk {
    content: Option<String>,
}

/// sync wrapper: bridges blocking RAG request + async LLM call with tokio runtime.
pub fn rag_answer(_args: Value, ctx: &BuiltinContext) -> BuiltinResult {
    let question = extract_question(ctx.agent_ctx.as_ref());
    if question.is_empty() {
        return Ok(json!({
            "message": "未从用户输入中提取到问题文本",
            "has_knowledge": false,
        }));
    }

    let llm = Arc::clone(&ctx.llm);
    let config = RagflowConfig::current();
    tokio::task::block_in_place(move || {
        tokio::runtime::Handle::current()
            .block_on(async move { rag_answer_async_impl(question, llm.as_ref(), config).await })
    })
}

async fn rag_answer_async_impl(
    question: String,
    llm: &crate::runtime::llm::LlmRegistry,
    config: RagflowConfig,
) -> BuiltinResult {
    let rag_chunks = query_rag_chunks(question.clone(), config).await;

    let has_knowledge = !rag_chunks.is_empty();

    let llm_answer = ask_llm_to_answer(&question, &rag_chunks, has_knowledge, llm).await;
    if !llm_answer.trim().is_empty() {
        let answer = llm_answer.trim().to_string();
        return Ok(json!({
            "_agent_context_updates": {
                "metadata": {
                    "agent_loop_break": "true",
                    "agent_loop_reply": answer
                }
            }
        }));
    }

    Ok(json!({
        "_agent_context_updates": {
            "extensions": [{
                "content_type": "card",
                "payload": {
                    "type": "support"
                },
            }],
            "metadata": {
                "agent_loop_break": "true",
                "agent_loop_reply": "抱歉，我无法回答您的问题。你可以通过下方「联系客服」继续反馈，我们会尽力协助处理。"
            }
        }
    }))
}

pub fn extract_question(
    agent_ctx: Option<&std::sync::Arc<agent::context::AgentContext>>,
) -> String {
    agent_ctx
        .map(|ctx| ctx.user_input().raw_text.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
}

pub async fn query_rag_chunks(question: String, config: RagflowConfig) -> Vec<String> {
    perform_rag_request(question, config).await
}

async fn perform_rag_request(question: String, config: RagflowConfig) -> Vec<String> {
    if config.base_url.is_empty() {
        tracing::warn!("RAGFlow base URL 未配置，跳过知识检索");
        return Vec::new();
    }
    if config.api_key.is_empty() {
        tracing::warn!("RAGFlow API key 未配置，跳过知识检索");
        return Vec::new();
    }

    let request = RetrievalRequest::from_config(question, &config);

    let url = format!("{}/api/v1/retrieval", config.base_url.trim_end_matches('/'));
    let client = match Client::builder()
        .timeout(Duration::from_secs(config.timeout_secs))
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(error = %err, "rag client build failed");
            return Vec::new();
        }
    };

    let response = match client
        .post(url)
        .header("content-type", "application/json")
        .bearer_auth(&config.api_key)
        .json(&request)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(error = %err, "rag retrieval request failed");
            return Vec::new();
        }
    };

    if !response.status().is_success() {
        tracing::warn!(status = %response.status(), "rag retrieval returned non-2xx");
        return Vec::new();
    }

    let payload = match response.json::<RetrievalResponse>().await {
        Ok(payload) => payload,
        Err(err) => {
            tracing::warn!(error = %err, "rag retrieval decode failed");
            return Vec::new();
        }
    };

    if payload.code != 0 {
        tracing::warn!(code = payload.code, "rag retrieval returned non-zero code");
        return Vec::new();
    }

    payload
        .data
        .map(|d| {
            d.chunks
                .into_iter()
                .filter_map(|chunk| chunk.content)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub async fn ask_llm_to_answer(
    question: &str,
    chunks: &[String],
    has_knowledge: bool,
    llm: &crate::runtime::llm::LlmRegistry,
) -> String {
    let prompt = if has_knowledge {
        build_prompt_with_knowledge(question, chunks)
    } else {
        build_prompt_without_knowledge(question)
    };

    let messages = vec![json!({
        "role": "user",
        "content": prompt,
    })];

    match llm.build_primary(None) {
        Ok((provider, model)) => {
            let req = ChatRequest {
                model: Some(model.clone()),
                messages,
                max_tokens: 1024,
                temperature: 0.25,
                tools: None,
                tool_choice: None,
                reasoning_effort: None,
            };
            let resp = provider
                .chat_with_retry(req, RetryMode::Standard, None)
                .await;
            let content = resp.content.unwrap_or_default();
            if content.trim().is_empty() {
                return String::new();
            }
            content
        }
        Err(err) => {
            tracing::warn!(error = %err, "build LLM primary failed");
            String::new()
        }
    }
}

fn build_prompt_with_knowledge(question: &str, chunks: &[String]) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "你是客服助手，请严格基于以下知识库内容回答。回答要简洁、可执行。不要提示用户转人工。\n\n",
    );
    prompt.push_str(&format!("用户问题：{}\n\n", question));
    prompt.push_str("知识片段：\n");
    for (idx, chunk) in chunks.iter().take(6).enumerate() {
        prompt.push_str(&format!("{}. {}\n", idx + 1, chunk));
    }
    prompt.push_str("\n请直接输出一段对用户的自然语言回复。");
    prompt
}

fn build_prompt_without_knowledge(question: &str) -> String {
    format!(
        "你是客服助手。用户提问：{question}\n\n请先回复一句“未检索到相关知识”，然后再基于常识给一个简洁、负责、可执行的答复。\n\n不要说你无能或无从回答。"
    )
}

pub const RAG_ANSWER_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {}
}"#;

pub const RAG_ANSWER_OUTPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "message": {
      "type": "string",
      "description": "最终回复文本"
    }
  },
  "required": []
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retrieval_request_uses_every_resolved_config_value() {
        let config = RagflowConfig {
            base_url: "https://ragflow.example".to_string(),
            api_key: "test-key".to_string(),
            dataset_ids: vec!["dataset".to_string()],
            document_ids: vec!["document".to_string()],
            page: 2,
            page_size: 7,
            similarity_threshold: 0.4,
            vector_similarity_weight: 0.6,
            top_k: 12,
            rerank_id: Some("reranker".to_string()),
            keyword: false,
            highlight: true,
            timeout_secs: 45,
        };

        let request = RetrievalRequest::from_config("question".to_string(), &config);

        assert_eq!(request.dataset_ids, config.dataset_ids);
        assert_eq!(request.document_ids, config.document_ids);
        assert_eq!(request.page, Some(config.page));
        assert_eq!(request.page_size, Some(config.page_size));
        assert_eq!(
            request.similarity_threshold,
            Some(config.similarity_threshold)
        );
        assert_eq!(
            request.vector_similarity_weight,
            Some(config.vector_similarity_weight)
        );
        assert_eq!(request.top_k, Some(config.top_k));
        assert_eq!(request.rerank_id, config.rerank_id);
        assert_eq!(request.keyword, Some(config.keyword));
        assert_eq!(request.highlight, Some(config.highlight));
    }
}
