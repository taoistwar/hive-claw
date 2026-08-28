use std::env;
use std::sync::Arc;
use std::time::Duration;

use providers::{ChatRequest, RetryMode};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::runtime::builtins::{BuiltinContext, BuiltinResult};

pub const DEFAULT_RAGFLOW_TOP_K: u32 = 10;
pub const DEFAULT_RAGFLOW_PAGE_SIZE: u32 = 5;
pub const DEFAULT_RAGFLOW_TIMEOUT_SECS: u64 = 30;
pub const ENV_RAGFLOW_BASE_URL: &str = "RAGFLOW_BASE_URL";
pub const ENV_RAGFLOW_API_KEY: &str = "RAGFLOW_API_KEY";
pub const ENV_RAGFLOW_DATASET_IDS: &str = "RAGFLOW_DATASET_IDS";
pub const ENV_RAGFLOW_DOCUMENT_IDS: &str = "RAGFLOW_DOCUMENT_IDS";
pub const ENV_RAGFLOW_TOP_K: &str = "RAGFLOW_TOP_K";
pub const ENV_RAGFLOW_PAGE_SIZE: &str = "RAGFLOW_PAGE_SIZE";

#[derive(Debug, Serialize)]
struct RetrievalRequest {
    question: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    dataset_ids: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    document_ids: Vec<String>,
    page: Option<u32>,
    page_size: Option<u32>,
    similarity_threshold: Option<f32>,
    vector_similarity_weight: Option<f32>,
    top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rerank_id: Option<String>,
    keyword: Option<bool>,
    highlight: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct RetrievalResponse {
    code: i32,
    data: Option<RetrievalData>,
}

#[derive(Debug, Deserialize)]
struct RetrievalData {
    chunks: Vec<RetrievalChunk>,
    #[serde(default)]
    total: Option<u32>,
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
    tokio::task::block_in_place(move || {
        tokio::runtime::Handle::current()
        .block_on(async move { rag_answer_async_impl(question, llm.as_ref()).await })
    })
}

async fn rag_answer_async_impl(
    question: String,
    llm: &crate::runtime::llm::LlmRegistry,
) -> BuiltinResult {
    let dataset_ids = env_list(ENV_RAGFLOW_DATASET_IDS);
    let document_ids = env_list(ENV_RAGFLOW_DOCUMENT_IDS);

    let rag_chunks = query_rag_chunks(question.clone(), dataset_ids, document_ids).await;

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

    return Ok(json!({
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
    }));

}

pub fn extract_question(agent_ctx: Option<&std::sync::Arc<agent::context::AgentContext>>) -> String {
    agent_ctx
        .and_then(|ctx| Some(ctx.user_input().raw_text.trim().to_string()))
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
}

pub async fn query_rag_chunks(
    question: String,
    dataset_ids: Vec<String>,
    document_ids: Vec<String>,
) -> Vec<String> {
    perform_rag_request(question, dataset_ids, document_ids).await
}

async fn perform_rag_request(
    question: String,
    dataset_ids: Vec<String>,
    document_ids: Vec<String>,
) -> Vec<String> {
    let base_url = match env::var(ENV_RAGFLOW_BASE_URL) {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => {
            tracing::warn!("RAGFLOW_BASE_URL 未配置，跳过知识检索");
            return Vec::new();
        }
    };

    let api_key = match env::var(ENV_RAGFLOW_API_KEY) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            tracing::warn!("RAGFLOW_API_KEY 未配置，跳过知识检索");
            return Vec::new();
        }
    };

    let request = RetrievalRequest {
        question,
        dataset_ids,
        document_ids,
        page: Some(1),
        page_size: Some(env_num(ENV_RAGFLOW_PAGE_SIZE).unwrap_or(DEFAULT_RAGFLOW_PAGE_SIZE)),
        similarity_threshold: Some(0.2),
        vector_similarity_weight: Some(0.3),
        top_k: Some(env_num(ENV_RAGFLOW_TOP_K).unwrap_or(DEFAULT_RAGFLOW_TOP_K)),
        rerank_id: None,
        keyword: Some(false),
        highlight: Some(false),
    };

    let url = format!("{}/api/v1/retrieval", base_url.trim_end_matches('/'));
    let client = match Client::builder()
        .timeout(Duration::from_secs(DEFAULT_RAGFLOW_TIMEOUT_SECS))
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
        .bearer_auth(api_key)
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

    match llm.build_chain(None) {
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
    prompt.push_str("你是客服助手，请严格基于以下知识库内容回答。回答要简洁、可执行。不要提示用户转人工。\n\n");
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

pub fn env_list(key: &str) -> Vec<String> {
    env::var(key)
        .ok()
        .map(|raw| split_csv(&raw))
        .unwrap_or_default()
}

fn split_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

fn env_num(key: &str) -> Option<u32> {
    env::var(key).ok().and_then(|raw| raw.parse::<u32>().ok())
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
    fn split_csv_basic() {
        assert_eq!(
            split_csv("a,, b ,c "),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }
}
