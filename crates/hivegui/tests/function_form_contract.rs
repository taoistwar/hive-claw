#[test]
fn function_form_uses_linked_selectors_and_persists_relations() {
    let source = include_str!("../src/ui/function_view.rs");

    for required in [
        "kind_select_open",
        "form_plugin_id",
        "plugin_select_open",
        "form_plugin_export",
        "export_select_open",
        "form_capability",
        "capability_select_open",
        "Plugin::list",
        "Capability::list",
        "内置函数",
        "自定义函数",
        "关联插件",
        "插件导出函数名",
        "所属 Capability",
        "required_capabilities",
        "let plugin_id =",
        "let plugin_export =",
        "let required_capabilities =",
    ] {
        assert!(
            source.contains(required),
            "function form is missing required selector behavior: {required}"
        );
    }

    assert!(
        source.contains(".when(self.form_kind == 2"),
        "plugin selectors must only be shown for custom functions"
    );
    assert!(
        !source.contains("kind_input: Option<Entity<InputState>>"),
        "Kind must be selected from fixed options instead of free text"
    );
}

#[test]
fn plugin_upload_records_wasm_exports_for_function_selection() {
    let source = include_str!("../src/ui/plugin_view.rs");

    assert!(
        source.contains("extract_wasm_exports"),
        "plugin upload must extract exported WASM function names"
    );
    assert!(
        source.contains("\"exports\"") && source.contains("form_manifest"),
        "plugin export names must be persisted in manifest metadata"
    );
}
