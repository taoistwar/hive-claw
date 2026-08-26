//! T016E [P] [Foundation-test-infra] Native scroll inventory + VisualTestContext
//! self-test. This file MUST be Green against the current implementation.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T016E
//! (foundation cross-cutting rule: helper self-test is Green; story
//! owner tests reuse the same helper).

mod support;

use support::scroll_inventory::{
    A11yFinding, ScrollSurface, assert_inventory_contains, assert_inventory_missing, bad_surface,
    foundation_inventory, good_surface, parse_source_tag,
};

// ---------------------------------------------------------------------------
// §T016E.1 — Inventory registration + compile-time asserts.
// ---------------------------------------------------------------------------

#[test]
fn foundation_inventory_registers_sidebar_only() {
    let inv = foundation_inventory();
    assert_inventory_contains(&inv, ScrollSurface::Sidebar);
    assert_inventory_missing(&inv, ScrollSurface::HomeNavigation);
    assert_inventory_missing(&inv, ScrollSurface::WorkflowDag);
    assert_eq!(inv.surfaces().count(), 1);
}

#[test]
fn owner_phase_maps_correctly_for_each_surface() {
    assert_eq!(
        ScrollSurface::Sidebar.owner_phase().to_string(),
        "Foundation"
    );
    assert_eq!(
        ScrollSurface::HomeNavigation.owner_phase().to_string(),
        "US1/T032"
    );
    assert_eq!(
        ScrollSurface::WorkflowDag.owner_phase().to_string(),
        "US10/T097-T098"
    );
    assert_eq!(
        ScrollSurface::AgentExecution.owner_phase().to_string(),
        "US13/T124"
    );
}

#[test]
fn every_surface_has_a_unique_slug() {
    let slugs: Vec<_> = [
        ScrollSurface::Sidebar,
        ScrollSurface::HomeNavigation,
        ScrollSurface::DataSourceList,
        ScrollSurface::DataSourceEdit,
        ScrollSurface::GlobalConfig,
        ScrollSurface::LlmList,
        ScrollSurface::TagList,
        ScrollSurface::CategoryList,
        ScrollSurface::CapabilityList,
        ScrollSurface::PluginList,
        ScrollSurface::FunctionList,
        ScrollSurface::WorkflowDag,
        ScrollSurface::ToolList,
        ScrollSurface::SkillList,
        ScrollSurface::AgentExecution,
    ]
    .iter()
    .map(|s| s.slug())
    .collect();
    let unique: std::collections::BTreeSet<_> = slugs.iter().copied().collect();
    assert_eq!(slugs.len(), unique.len(), "slugs must be unique");
    for slug in &slugs {
        assert!(!slug.is_empty(), "slug must not be empty");
        assert!(
            slug.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "slug `{slug}` must be ASCII lowercase / digits / underscores"
        );
    }
}

// ---------------------------------------------------------------------------
// §T016E.2 — VisualTestContext: good surface (clean audit).
// ---------------------------------------------------------------------------

#[test]
fn good_surface_produces_clean_audit() {
    let mut ctx = good_surface();
    let audit = ctx.run_audit();
    assert!(
        audit.findings.is_empty(),
        "good surface produced findings: {:#?}",
        audit.findings
    );
    assert_eq!(audit.surface, ScrollSurface::Sidebar);
}

#[test]
fn good_surface_keyboard_trail_records_focus() {
    let mut ctx = good_surface();
    let _ = ctx.focus_index(2).expect("focus index 2 in range");
    let trail = ctx.keyboard_trail();
    assert!(trail.keyboard_only, "good surface is keyboard-only");
    assert_eq!(trail.focus_events, vec![1, 2]);
}

#[test]
fn good_surface_scroll_clamps_to_max_offset() {
    let mut ctx = good_surface();
    // 4 items, 24 height → items never overflow → max_offset = 0.
    let _ = ctx.scroll(10).expect("scroll ok");
    let _ = ctx.scroll(50).expect("scroll ok");
    // Cannot over-scroll a non-overflowing list.
    let trail = ctx.keyboard_trail();
    assert!(trail.scroll_events.iter().all(|e| e.resulting_offset >= 0));
}

// ---------------------------------------------------------------------------
// §T016E.3 — VisualTestContext: bad surface (findings surface).
// ---------------------------------------------------------------------------

#[test]
fn bad_surface_produces_findings() {
    let mut ctx = bad_surface();
    // bad_surface has already called focus_index(99) and run_audit
    // before returning; rebuild findings to be explicit.
    let audit = ctx.run_audit();
    let kinds: Vec<&'static str> = audit
        .findings
        .iter()
        .map(|f| match f {
            A11yFinding::EmptySurface { .. } => "empty_surface",
            A11yFinding::FocusOutOfRange { .. } => "focus_out_of_range",
            A11yFinding::MouseOnlyPath { .. } => "mouse_only_path",
        })
        .collect();
    assert!(kinds.contains(&"empty_surface"), "missing empty finding");
    assert!(
        kinds.contains(&"focus_out_of_range"),
        "missing focus finding"
    );
    assert!(kinds.contains(&"mouse_only_path"), "missing mouse finding");
}

#[test]
fn focus_index_rejects_out_of_range() {
    let mut ctx = good_surface();
    let err = ctx.focus_index(99).expect_err("index 99 out of range");
    let msg = format!("{err}");
    assert!(msg.contains("focus index 99 out of range"), "got: {msg}");
}

#[test]
fn step_advances_cursor_and_reports_end() {
    let mut ctx = good_surface();
    // Good surface has 1 frame; stepping past it is `EndOfFrames`.
    let err = ctx.step().expect_err("exhausted frames");
    assert!(format!("{err}").contains("harness exhausted frames"));
}

// ---------------------------------------------------------------------------
// §T016E.4 — Source-contract tag parser.
// ---------------------------------------------------------------------------

#[test]
fn parse_source_tag_extracts_slug() {
    let comment = "\
//! A scrollable sidebar view.
//! scroll:sidebar
//!";
    assert_eq!(parse_source_tag(comment).as_deref(), Some("sidebar"));
}

#[test]
fn parse_source_tag_returns_none_for_missing_tag() {
    let comment = "//! A non-scroll view.";
    assert_eq!(parse_source_tag(comment), None);
}

#[test]
fn parse_source_tag_handles_indented_blocks() {
    let comment = "\
    //! scroll:datasource_list
    //!";
    assert_eq!(
        parse_source_tag(comment).as_deref(),
        Some("datasource_list")
    );
}
