use std::path::PathBuf;

#[test]
fn every_hiveweb_runtime_llm_call_uses_the_fallback_chain() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let call_sites = [
        "src/runtime/orchestrator.rs",
        "src/runtime/capabilities/llm.rs",
        "src/runtime/skill_test.rs",
        "src/runtime/tool_test.rs",
        "src/runtime/builtins/game_info.rs",
        "src/runtime/workflow/generate_answer_node.rs",
    ];

    for relative_path in call_sites {
        let path = manifest_dir.join(relative_path);
        let source = std::fs::read_to_string(&path).expect("read runtime LLM call site");
        assert!(
            source.contains(".build_chain("),
            "{relative_path} must construct the configured primary + fallback chain"
        );
        assert!(
            !source.contains(".build_primary("),
            "{relative_path} must not bypass the configured fallback chain"
        );
    }
}

#[test]
fn every_hiveweb_runtime_llm_call_uses_one_bounded_chain_traversal() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let call_sites = [
        ("src/runtime/orchestrator.rs", "chat_stream_with_options"),
        ("src/runtime/capabilities/llm.rs", "chat_with_options"),
        ("src/runtime/skill_test.rs", "chat_stream_with_options"),
        ("src/runtime/tool_test.rs", "chat_stream_with_options"),
        ("src/runtime/builtins/game_info.rs", "chat_with_options"),
        (
            "src/runtime/workflow/generate_answer_node.rs",
            "chat_with_options",
        ),
    ];

    for (relative_path, bounded_entrypoint) in call_sites {
        let path = manifest_dir.join(relative_path);
        let source = std::fs::read_to_string(&path).expect("read runtime LLM call site");
        assert!(
            source.contains(bounded_entrypoint),
            "{relative_path} must call {bounded_entrypoint}"
        );
        assert!(
            !source.contains("chat_with_retry") && !source.contains("chat_stream_with_retry"),
            "{relative_path} must not retry the complete fallback chain"
        );
    }
}
