#[test]
fn tool_form_uses_kind_and_source_selectors_with_exclusive_targets() {
    let source = include_str!("../src/ui/tool_view.rs");

    for required in [
        "form_kind: i64",
        "kind_select_open",
        "source_select_open",
        "selector_field",
        "(1_i64, \"函数\")",
        "(2_i64, \"工作流\")",
        "(\"workspace\", \"workspace\")",
        "(\"builtin\", \"builtin\")",
    ] {
        assert!(
            source.contains(required),
            "tool form is missing selector behavior: {required}"
        );
    }

    assert!(
        !source.contains("kind_input: Option<Entity<InputState>>")
            && !source.contains("source_input: Option<Entity<InputState>>"),
        "Kind and Source must not remain free-text inputs"
    );
    assert!(
        source.contains(".when(self.form_kind == 1")
            && source.contains("Function ID *")
            && source.contains(".when(self.form_kind == 2")
            && source.contains("Workflow ID *"),
        "only the target field matching the selected Kind may be visible"
    );
    assert!(
        source.contains("view.form_workflow_id.clear()")
            && source.contains("view.form_function_id.clear()"),
        "switching Kind must clear the now-hidden target"
    );
    assert!(
        source.contains("let fid: Option<i64> = if kind == 1")
            && source.contains("let wid: Option<i64> = if kind == 2"),
        "saving must ignore stale values belonging to the other Kind"
    );
}
