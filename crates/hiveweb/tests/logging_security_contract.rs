fn assert_source_omits(label: &str, source: &str, forbidden: &[&str]) {
    for pattern in forbidden {
        assert!(
            !source.contains(pattern),
            "{label} still contains unsafe logging pattern `{pattern}`"
        );
    }
}

#[test]
fn request_and_runtime_logging_sources_omit_sensitive_values() {
    assert_source_omits(
        "api/mod.rs",
        include_str!("../src/api/mod.rs"),
        &["DefaultMakeSpan", "error = %e"],
    );
    assert_source_omits(
        "api/runtime.rs",
        include_str!("../src/api/runtime.rs"),
        &["raw_text = ?ui.raw_text"],
    );
    assert_source_omits(
        "api/workflow.rs",
        include_str!("../src/api/workflow.rs"),
        &["raw_text = ?ui.raw_text", "error = %e"],
    );
    assert_source_omits(
        "runtime/orchestrator.rs",
        include_str!("../src/runtime/orchestrator.rs"),
        &[
            "key = %k, value = %v",
            "response_content = %rc",
            "Saving assistant message:",
            "error = %e",
        ],
    );
    assert_source_omits(
        "runtime/llm.rs",
        include_str!("../src/runtime/llm.rs"),
        &["env_name = val", "error = %e"],
    );
    assert_source_omits(
        "runtime/invoker.rs",
        include_str!("../src/runtime/invoker.rs"),
        &["error = %error"],
    );
    assert_source_omits(
        "runtime/builtins/game_info.rs",
        include_str!("../src/runtime/builtins/game_info.rs"),
        &[
            "user_input = %user_input",
            "llm_response = ?",
            "llm_output = %",
            "error = %e",
            "game_ids = ?",
        ],
    );
    assert_source_omits(
        "runtime/builtins/query_balance.rs",
        include_str!("../src/runtime/builtins/query_balance.rs"),
        &[
            "?membership_json",
            "?duration_card_json",
            "?reply",
            "?payload",
            "?result",
            "config = %config",
            "error = %e",
        ],
    );
}

#[test]
fn capability_workflow_and_hook_sources_omit_raw_errors_and_values() {
    assert_source_omits(
        "runtime/capability.rs",
        include_str!("../src/runtime/capability.rs"),
        &[
            "error = %e",
            "ReplyEnvelope::err(4000, e.clone())",
            "format!(\"{label} args: {e}\")",
        ],
    );
    assert_source_omits(
        "runtime/capabilities/db.rs",
        include_str!("../src/runtime/capabilities/db.rs"),
        &["error = %e", "col = ?col.name()"],
    );
    assert_source_omits(
        "runtime/hook.rs",
        include_str!("../src/runtime/hook.rs"),
        &[
            "identifier = %ctx.identifier",
            "hook_name = %hook.name",
            "Unknown category in _agent_context_updates: {category_str}",
            "Failed to apply _agent_context_updates record: {e}",
            "Failed to apply _agent_context_updates extension: {e}",
            "Failed to apply _agent_context_updates metadata: {e}",
        ],
    );
    assert_source_omits(
        "runtime/workflow/generate_answer_node.rs",
        include_str!("../src/runtime/workflow/generate_answer_node.rs"),
        &[
            "input_keys = ?",
            "query_value = ?",
            "error = %err_msg",
            "error = %e",
        ],
    );
    assert_source_omits(
        "runtime/workflow/function_node.rs",
        include_str!("../src/runtime/workflow/function_node.rs"),
        &["error = %msg"],
    );
    assert_source_omits(
        "runtime/workflow/node_executor.rs",
        include_str!("../src/runtime/workflow/node_executor.rs"),
        &["error = %e"],
    );
    assert_source_omits(
        "runtime/workflow/workflow_executor.rs",
        include_str!("../src/runtime/workflow/workflow_executor.rs"),
        &["error = %e"],
    );
}

#[test]
fn tool_and_skill_test_sources_do_not_expose_untrusted_debug_data() {
    assert_source_omits(
        "runtime/tool_test.rs",
        include_str!("../src/runtime/tool_test.rs"),
        &[
            "println!",
            "identifier={}",
            "model={}",
            "executing tool '{}'",
            "tool '{}' not found",
        ],
    );
    assert_source_omits(
        "api/tool.rs",
        include_str!("../src/api/tool.rs"),
        &[
            "req.trace_id.take()",
            "format!(\"{} (debug: {})\", e, log_path)",
        ],
    );
    assert_source_omits(
        "api/skill.rs",
        include_str!("../src/api/skill.rs"),
        &["Err(e) => Err(ApiResponse::err(5000, e))"],
    );
}

#[test]
fn assistant_service_sources_omit_sensitive_words_cache_keys_and_raw_errors() {
    assert_source_omits(
        "api/chat_assistant.rs",
        include_str!("../src/api/chat_assistant.rs"),
        &[
            "triggered_word =",
            "quota_key =",
            "error = %e",
            "error = %error",
            "error = %rollback_error",
        ],
    );
    assert_source_omits(
        "api/chat_messages.rs",
        include_str!("../src/api/chat_messages.rs"),
        &["info = %info", "ext = %ext", "error = %e"],
    );
    assert_source_omits(
        "services/sensitive_filter.rs",
        include_str!("../src/services/sensitive_filter.rs"),
        &["word = %row.word", "error = %e"],
    );
    assert_source_omits(
        "services/cache_helper.rs",
        include_str!("../src/services/cache_helper.rs"),
        &["tracing::debug!(%key", "error = %e"],
    );
    assert_source_omits(
        "services/game_service.rs",
        include_str!("../src/services/game_service.rs"),
        &["tracing::debug!(%key", "error = %e"],
    );
    assert_source_omits(
        "services/membership.rs",
        include_str!("../src/services/membership.rs"),
        &[
            "error = %error",
            "membership_level = ?",
            "effective_end_time = ?",
        ],
    );
    assert_source_omits(
        "main.rs",
        include_str!("../src/main.rs"),
        &[
            "External DB connection failed ({})",
            "builtin functions upsert failed: {e}",
            "error = %e",
        ],
    );
    assert_source_omits(
        "cache/redis.rs",
        include_str!("../src/cache/redis.rs"),
        &["error = %error"],
    );
    assert_source_omits(
        "bin/chat_retention.rs",
        include_str!("../src/bin/chat_retention.rs"),
        &["error = %e"],
    );
}
