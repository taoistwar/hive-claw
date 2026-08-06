//! Skill test runner (T128 / US8)
//!
//! 为单个 Skill 创建隔离测试环境：加载目标 Skill + Always Tools + Always Skills → 执行单轮对话 → 返回结果

use providers::{ChatRequest, LLMProvider, LlmCallOptions};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::MySqlPool;
use std::sync::Arc;

use super::orchestrator::{OrchestratorDeps, build_tools_schema_simple, handle_workspace_tool};
use crate::runtime::execution_context::RuntimeExecutionContext;
use crate::runtime::llm::{LlmAdapterError, LlmRegistry};
use crate::runtime::llm_audit::{LlmAuditGuard, LlmAuditSource};
use crate::services::agent::{AgentContent, ToolRef};
use agent::context::{AgentContext, ContextConfig, UserInput};

type SkillTestToolRow = (
    i64,
    String,
    String,
    String,
    i8,
    Option<i64>,
    Option<i64>,
    Value,
    Option<i64>,
    Option<String>,
    Option<Value>,
);

#[derive(Debug, Deserialize)]
pub struct TestSkillRequest {
    pub message: String,
    pub model_preset: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TestSkillResult {
    pub assistant_content: String,
    pub has_tool_calls: bool,
    pub tool_calls: Vec<TestSkillToolCallRecord>,
}

#[derive(Debug, Serialize)]
pub struct TestSkillToolCallRecord {
    pub tool_name: String,
    pub arguments: Value,
    pub result: TestSkillToolCallOutcome,
}

#[derive(Debug, Serialize)]
pub struct TestSkillToolCallOutcome {
    pub success: bool,
    pub content: Value,
    pub error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SkillTestError {
    #[error("model preset unknown: {0}")]
    ModelPresetUnknown(String),
    #[error("{0}")]
    Failed(String),
}

impl From<String> for SkillTestError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

struct ResolvedSkillTestLlmTarget {
    provider: Arc<dyn LLMProvider>,
    model: String,
    preset_name: String,
    max_tokens: u32,
    temperature: f32,
}

trait SkillTestLlmRegistry {
    fn resolve_preset(
        &self,
        requested_preset: Option<&str>,
    ) -> Result<(String, u32, f32), LlmAdapterError>;

    fn build_chain(
        &self,
        preset_name: Option<&str>,
    ) -> Result<(Arc<dyn LLMProvider>, String), LlmAdapterError>;
}

impl SkillTestLlmRegistry for LlmRegistry {
    fn resolve_preset(
        &self,
        requested_preset: Option<&str>,
    ) -> Result<(String, u32, f32), LlmAdapterError> {
        let entry = self.resolve(requested_preset)?;
        Ok((entry.name.clone(), entry.max_tokens, entry.temperature))
    }

    fn build_chain(
        &self,
        preset_name: Option<&str>,
    ) -> Result<(Arc<dyn LLMProvider>, String), LlmAdapterError> {
        LlmRegistry::build_chain(self, preset_name)
    }
}

fn resolve_skill_test_llm_target<R: SkillTestLlmRegistry + ?Sized>(
    registry: &R,
    execution_context: &RuntimeExecutionContext,
    skill_id: i64,
    requested_preset: Option<&str>,
) -> Result<ResolvedSkillTestLlmTarget, SkillTestError> {
    let (preset_name, max_tokens, temperature) = match registry.resolve_preset(requested_preset) {
        Ok(preset) => preset,
        Err(LlmAdapterError::Unknown(name)) => {
            let mut audit = LlmAuditGuard::new(
                execution_context.clone(),
                None,
                requested_preset,
                LlmAuditSource::SkillTest { skill_id },
            );
            audit.finish_model_preset_unknown();
            return Err(SkillTestError::ModelPresetUnknown(name));
        }
        Err(error) => {
            return Err(SkillTestError::Failed(format!(
                "LLM preset resolution failed: {error}"
            )));
        }
    };
    let (provider, model) = registry
        .build_chain(Some(&preset_name))
        .map_err(|error| SkillTestError::Failed(format!("LLM provider: {error}")))?;

    Ok(ResolvedSkillTestLlmTarget {
        provider,
        model,
        preset_name,
        max_tokens,
        temperature,
    })
}

pub async fn run_skill_test(
    pool: &MySqlPool,
    deps: &OrchestratorDeps,
    skill_id: i64,
    req: TestSkillRequest,
) -> Result<TestSkillResult, SkillTestError> {
    // 1. 查询目标 skill
    let skill_row: Option<(String,)> = sqlx::query_as("SELECT content FROM skills WHERE id = ?")
        .bind(skill_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("skill lookup: {e}"))?;

    let Some((skill_content,)) = skill_row else {
        return Err(SkillTestError::Failed(format!(
            "skill id={skill_id} not found"
        )));
    };

    if skill_content.trim().is_empty() {
        return Err(SkillTestError::Failed("skill content 不能为空".into()));
    }

    // 2. 加载 all tools（全部工具暴露给 Skill 测试，让 Skill 有机会调用任何工具）
    let tool_rows: Vec<SkillTestToolRow> = sqlx::query_as(
        r#"SELECT t.id, t.identifier, t.name, t.description, t.kind,
                      t.function_id, t.workflow_id, t.input_schema,
                      f.plugin_id, f.plugin_export,
                      t.required_capabilities
               FROM tools t
               LEFT JOIN functions f ON f.id = t.function_id"#,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| format!("tools: {e}"))?;

    // 3. 构建 ToolRef 列表（全部 tools）
    let tools: Vec<ToolRef> = tool_rows
        .into_iter()
        .map(
            |(id, ident, name, desc, kind, fid, wid, input_schema, pid, pexport, caps_raw)| {
                let is_builtin = kind == 1 && pid.is_none();
                let is_meta = kind == 1 && fid.is_none();
                let required_capabilities: Vec<String> = caps_raw
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default();
                ToolRef {
                    id,
                    identifier: ident,
                    name,
                    description: desc,
                    kind,
                    function_id: fid,
                    function_identifier: None,
                    workflow_id: wid,
                    input_schema,
                    plugin_id: pid,
                    plugin_export: pexport,
                    is_builtin_function: is_builtin,
                    is_meta_tool: is_meta,
                    required_capabilities,
                }
            },
        )
        .collect();

    // 4. 加载 always skills（排除目标 skill）+ 目标 skill 拼入 system prompt
    let always_skills: Vec<(String,)> =
        sqlx::query_as("SELECT content FROM skills WHERE is_always = 1 AND id != ?")
            .bind(skill_id)
            .fetch_all(pool)
            .await
            .map_err(|e| format!("always skills: {e}"))?;

    let mut system_prompt = "You are a test assistant. Follow the skill instructions carefully and use available tools when appropriate.".to_string();

    // 先拼入 always skills
    for (md,) in &always_skills {
        system_prompt.push_str("\n\n--- ALWAYS SKILL ---\n\n");
        system_prompt.push_str(md);
    }

    // 再拼入目标 skill（核心）
    system_prompt.push_str("\n\n--- TARGET SKILL TO TEST ---\n\n");
    system_prompt.push_str(&skill_content);

    // 测试模式：授予所有 capability 权限，以便插件能自由调用 network.http 等能力
    let permissions: Vec<String> = crate::runtime::capability::CAPABILITIES
        .iter()
        .map(|c| c.name.to_string())
        .collect();

    let ctx = AgentContent {
        agent_id: 0, // 测试模式，不需要真实 agent_id
        identifier: format!("test_skill_{}", skill_id),
        system_prompt,
        model_preset: req.model_preset,
        tools,
        permissions,
        children: vec![],
        hooks: std::collections::HashMap::new(),
    };

    // 5. 构建 LLM request
    let requested_preset = ctx.model_preset.as_deref();
    let ResolvedSkillTestLlmTarget {
        provider,
        model,
        preset_name: resolved_preset,
        max_tokens,
        temperature,
    } = resolve_skill_test_llm_target(
        deps.llm.as_ref(),
        &deps.execution_context,
        skill_id,
        requested_preset,
    )?;

    let tools_schema = build_tools_schema_simple(&ctx.tools);

    let messages: Vec<Value> = vec![
        json!({"role": "system", "content": ctx.system_prompt}),
        json!({"role": "user", "content": req.message}),
    ];

    let chat_req = ChatRequest {
        model: Some(model),
        messages,
        max_tokens,
        temperature,
        tools: if tools_schema.is_empty() {
            None
        } else {
            Some(tools_schema)
        },
        tool_choice: None,
        reasoning_effort: None,
    };

    let mut llm_audit = LlmAuditGuard::new(
        deps.execution_context.clone(),
        None,
        Some(&resolved_preset),
        LlmAuditSource::SkillTest { skill_id },
    );
    let options = LlmCallOptions::default().with_fallback_callback(llm_audit.on_fallback());
    let resp = provider
        .chat_stream_with_options(chat_req, None, None, options)
        .await;
    llm_audit.finish_response(&resp);

    if resp.is_error() {
        let err_msg = resp.content.clone().unwrap_or_else(|| "unknown".into());
        return Err(SkillTestError::Failed(format!("LLM error: {err_msg}")));
    }

    let assistant_content = resp.content.unwrap_or_default();
    let tool_calls = resp.tool_calls;

    if tool_calls.is_empty() {
        return Ok(TestSkillResult {
            assistant_content,
            has_tool_calls: false,
            tool_calls: vec![],
        });
    }

    // 6. 执行工具调用
    let mut records = Vec::new();
    for tc in tool_calls {
        let tool_name = tc.name.clone();
        let args = tc.arguments.clone();

        let tool_ref = ctx.tools.iter().find(|t| t.identifier == tc.name);
        let Some(tool_ref) = tool_ref else {
            records.push(TestSkillToolCallRecord {
                tool_name,
                arguments: Value::Object(args),
                result: TestSkillToolCallOutcome {
                    success: false,
                    content: Value::Null,
                    error: Some("tool not found in test context".into()),
                },
            });
            continue;
        };

        let agent_ctx = Arc::new(AgentContext::new(
            "skill-test".into(),
            UserInput {
                raw_text: String::new(),
                session_id: None,
                message_id: None,
                timestamp: chrono::Utc::now(),
                metadata: std::collections::HashMap::new(),
            },
            ContextConfig::default(),
        ));
        let outcome = handle_workspace_tool(deps, &ctx, tool_ref, &tc, 0, agent_ctx).await;

        let (success, content, error) = match outcome {
            o if matches!(o.payload, Value::Object(_)) && !o.payload.get("error").is_some() => {
                (true, o.payload, None)
            }
            o => {
                let error = o
                    .payload
                    .get("error")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                (false, o.payload, error)
            }
        };

        records.push(TestSkillToolCallRecord {
            tool_name,
            arguments: Value::Object(args),
            result: TestSkillToolCallOutcome {
                success,
                content,
                error,
            },
        });
    }

    Ok(TestSkillResult {
        assistant_content,
        has_tool_calls: true,
        tool_calls: records,
    })
}

#[cfg(test)]
mod tests {
    use super::{SkillTestError, SkillTestLlmRegistry, resolve_skill_test_llm_target};
    use crate::runtime::execution_context::RuntimeExecutionContext;
    use crate::runtime::llm::LlmAdapterError;
    use async_trait::async_trait;
    use providers::{ChatRequest, LLMProvider, LLMResponse};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct NeverCalledProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl LLMProvider for NeverCalledProvider {
        fn default_model(&self) -> String {
            "global-default-must-not-run".to_string()
        }

        async fn chat(&self, _request: ChatRequest) -> LLMResponse {
            self.calls.fetch_add(1, Ordering::SeqCst);
            LLMResponse::default()
        }
    }

    struct UnknownPresetRegistry {
        provider: Arc<NeverCalledProvider>,
        build_calls: AtomicUsize,
    }

    impl SkillTestLlmRegistry for UnknownPresetRegistry {
        fn resolve_preset(
            &self,
            requested_preset: Option<&str>,
        ) -> Result<(String, u32, f32), LlmAdapterError> {
            Err(LlmAdapterError::Unknown(
                requested_preset.unwrap_or("global-default").to_string(),
            ))
        }

        fn build_chain(
            &self,
            _preset_name: Option<&str>,
        ) -> Result<(Arc<dyn LLMProvider>, String), LlmAdapterError> {
            self.build_calls.fetch_add(1, Ordering::SeqCst);
            Ok((
                Arc::clone(&self.provider) as Arc<dyn LLMProvider>,
                "global-default-model".to_string(),
            ))
        }
    }

    #[test]
    fn explicit_empty_and_unknown_skill_test_presets_are_typed_without_default_provider() {
        let provider = Arc::new(NeverCalledProvider {
            calls: AtomicUsize::new(0),
        });
        let registry = UnknownPresetRegistry {
            provider: Arc::clone(&provider),
            build_calls: AtomicUsize::new(0),
        };
        let execution_context =
            RuntimeExecutionContext::best_effort(Some("skill-test-unknown".into()), None)
                .for_hook();

        for requested_preset in ["", "removed-preset"] {
            let error = match resolve_skill_test_llm_target(
                &registry,
                &execution_context,
                19,
                Some(requested_preset),
            ) {
                Ok(_) => panic!("explicit unknown preset must fail closed"),
                Err(error) => error,
            };
            assert!(matches!(
                error,
                SkillTestError::ModelPresetUnknown(name) if name == requested_preset
            ));
        }

        assert_eq!(registry.build_calls.load(Ordering::SeqCst), 0);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }
}
