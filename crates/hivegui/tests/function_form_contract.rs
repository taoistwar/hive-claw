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
        source.contains(".when(self.form_kind == \"custom\""),
        "plugin selectors must only be shown for custom functions"
    );
    assert!(
        !source.contains("kind_input: Option<Entity<InputState>>"),
        "Kind must be selected from fixed options instead of free text"
    );
}

#[test]
fn placeholder_function_kind_only_exposes_schema_configuration() {
    let source = include_str!("../src/ui/function_view.rs");

    assert!(
        source.contains("\"placeholder\" => \"占位\""),
        "placeholder functions must use a readable Kind label"
    );
    assert!(
        source.contains(".when(self.form_kind == \"custom\""),
        "plugin fields must remain exclusive to custom functions"
    );
    assert!(
        source.contains(".when(self.form_kind != \"placeholder\""),
        "placeholder functions must hide Capability configuration"
    );
    assert!(
        source.contains("if kind == \"placeholder\"")
            && source.contains("view.form_capability = None")
            && source.contains("required_capabilities = None"),
        "selecting or saving a placeholder must clear executable relations"
    );
    assert!(
        source.contains(".when(ic.kind != \"placeholder\""),
        "a placeholder without an implementation must not expose the test action"
    );
}

#[test]
fn desktop_kind_selector_no_longer_offers_the_placeholder_type() {
    let source = include_str!("../src/ui/function_view.rs");

    assert!(
        source.contains("[(\"custom\", \"自定义函数\")]"),
        "the desktop Kind selector must only offer the custom function type"
    );
    assert!(
        !source.contains("(\"placeholder\", \"占位\")"),
        "the desktop Kind selector must not offer the placeholder type"
    );
    assert!(
        source.contains("fn default_function_kind(is_prompt_studio: bool)"),
        "the default Kind must be derived from the running application"
    );
}

#[test]
fn prompt_studio_function_form_drops_kind_identifier_and_execution_hints() {
    let source = include_str!("../src/ui/function_view.rs");

    assert!(
        source.contains("pub fn new_prompt_studio(store: Entity<Store>, cx: &mut Context<Self>)"),
        "Prompt Studio must construct its own Function management view"
    );
    assert!(
        source.contains("fn prompt_studio_identifier(name: &str)"),
        "Prompt Studio must derive the identifier from the function name"
    );
    assert!(
        source.contains(".when(!is_prompt_studio, |form| form.child("),
        "Prompt Studio must hide the Kind selector and the Identifier field"
    );
    assert!(
        source.contains("self.form_kind == \"placeholder\" && !is_prompt_studio"),
        "Prompt Studio must not render the schema-only execution hint"
    );
    assert!(
        source.contains("item.kind == \"placeholder\" && !is_prompt_studio"),
        "Prompt Studio must not render the non-executable row state"
    );
    assert!(
        source.contains("let show_identifier = !is_prompt_studio;"),
        "Prompt Studio must not render the redundant Identifier column"
    );
    assert!(
        source.contains("items.retain(|item| item.kind != FunctionKind::Builtin.as_str());"),
        "Prompt Studio must not list Builtin Functions"
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
