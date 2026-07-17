fn assert_theme_aware(name: &str, source: &str) {
    assert!(
        source.contains("cx.theme()") || source.contains("ManagementStyle::current(cx)"),
        "{name} does not read colors from the active theme"
    );

    for legacy in ["rgb(0x", "rgba(0x"] {
        assert!(
            !source.contains(legacy),
            "{name} still contains a hard-coded color via {legacy}"
        );
    }
}

#[test]
fn datasource_management_surfaces_follow_the_active_theme() {
    for (name, source) in [
        (
            "datasource_view",
            include_str!("../src/ui/datasource_view.rs"),
        ),
        (
            "datasource_form",
            include_str!("../src/ui/datasource_form.rs"),
        ),
        ("tree_nav", include_str!("../src/ui/tree_nav.rs")),
        ("table_viewer", include_str!("../src/ui/table_viewer.rs")),
    ] {
        assert_theme_aware(name, source);
    }
}

#[test]
fn datasource_form_renders_editable_input_widgets() {
    let source = include_str!("../src/ui/datasource_form.rs");

    assert!(
        source.contains("gpui_component::input::{Input, InputState}"),
        "datasource form must import the editable Input widget"
    );
    assert!(
        source.contains("Input::new(&input)"),
        "datasource form fields must render InputState through Input"
    );
    assert!(
        !source.contains(".child(input)"),
        "rendering InputState directly leaves datasource fields read-only"
    );
}
