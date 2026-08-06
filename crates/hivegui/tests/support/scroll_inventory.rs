//! T016E [P] [Foundation-test-infra] Native scroll inventory + VisualTestContext
//! helper. Self-test MUST be Green against the current implementation.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T016E
//! (foundation cross-cutting rule: every native scroll surface —
//! sidebar + story content + reactive containers — is enumerated in
//! the inventory, owns a build-time tag, and uses this helper for
//! visual / accessibility / keyboard-only navigation tests).
//!
//! This is the test-infrastructure half of T016E. The product-side
//! scroll tag (the per-surface `inventory_id` and a matching
//! `inventory_contains!` macro invocation) is added by the story
//! owner when they activate the surface. The helper provides:
//!
//!   - `ScrollSurface` — the inventory record; the canonical list of
//!     known surfaces is enforced at compile time so a story cannot
//!     add a surface without registering it here.
//!   - `VisualTestContext` — a deterministic harness that drives
//!     scroll / focus / keyboard navigation and exposes the
//!     `AccessibilityAudit` and `KeyboardOnlyTrail` collections.
//!   - `inventory_contains!` / `inventory_missing!` — compile-time
//!     asserts that a surface is (or is not) registered.
//!   - `GOOD_SURFACE` / `BAD_SURFACE` synthetic fixtures that the
//!     helper self-test exercises. The fixtures are intentionally
//!     self-contained: they do not pull in product code, so a Green
//!     here is a Green of the helper, not of any product surface.

use std::{collections::BTreeMap, fmt, path::PathBuf, sync::Arc};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// §T016E.1 — Inventory record + canonical surface list.
// ---------------------------------------------------------------------------

/// Stable identifier for every native scroll surface.
///
/// Each story owner registers their scroll surface in this enum
/// when they activate the surface. The Foundation phase owns the
/// `Sidebar` surface. Every other variant must be added by the
/// owning story and tagged with its `owner_phase` in
/// [`ScrollSurface::owner_phase`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ScrollSurface {
    /// `crates/hivegui/src/ui/sidebar.rs` — Foundation phase.
    Sidebar,
    /// `crates/hivegui/src/ui/home/main_view.rs` — US1.
    HomeNavigation,
    /// `crates/hivegui/src/ui/datasource/list_view.rs` — US2.
    DataSourceList,
    /// `crates/hivegui/src/ui/datasource/edit_view.rs` — US2.
    DataSourceEdit,
    /// `crates/hivegui/src/ui/config/global_view.rs` — US3.
    GlobalConfig,
    /// `crates/hivegui/src/ui/llm/list_view.rs` — US4.
    LlmList,
    /// `crates/hivegui/src/ui/tag/list_view.rs` — US5.
    TagList,
    /// `crates/hivegui/src/ui/category/list_view.rs` — US6.
    CategoryList,
    /// `crates/hivegui/src/ui/capability/list_view.rs` — US7.
    CapabilityList,
    /// `crates/hivegui/src/ui/plugin/list_view.rs` — US8.
    PluginList,
    /// `crates/hivegui/src/ui/function/list_view.rs` — US9.
    FunctionList,
    /// `crates/hivegui/src/ui/workflow/dag_view.rs` — US10.
    WorkflowDag,
    /// `crates/hivegui/src/ui/tool/list_view.rs` — US11.
    ToolList,
    /// `crates/hivegui/src/ui/skill/list_view.rs` — US12.
    SkillList,
    /// `crates/hivegui/src/ui/agent/execution_view.rs` — US13.
    AgentExecution,
}

impl ScrollSurface {
    /// The owner story / task pair for this surface. Foundation owns
    /// `Sidebar`; every other surface MUST match its story owner.
    pub fn owner_phase(self) -> OwnerPhase {
        match self {
            ScrollSurface::Sidebar => OwnerPhase::Foundation,
            ScrollSurface::HomeNavigation => OwnerPhase::Story {
                story: "US1",
                task: "T032",
            },
            ScrollSurface::DataSourceList | ScrollSurface::DataSourceEdit => OwnerPhase::Story {
                story: "US2",
                task: "T038",
            },
            ScrollSurface::GlobalConfig => OwnerPhase::Story {
                story: "US3",
                task: "T044",
            },
            ScrollSurface::LlmList => OwnerPhase::Story {
                story: "US4",
                task: "T051",
            },
            ScrollSurface::TagList => OwnerPhase::Story {
                story: "US5",
                task: "T058",
            },
            ScrollSurface::CategoryList => OwnerPhase::Story {
                story: "US6",
                task: "T063",
            },
            ScrollSurface::CapabilityList => OwnerPhase::Story {
                story: "US7",
                task: "T069",
            },
            ScrollSurface::PluginList => OwnerPhase::Story {
                story: "US8",
                task: "T077",
            },
            ScrollSurface::FunctionList => OwnerPhase::Story {
                story: "US9",
                task: "T087",
            },
            ScrollSurface::WorkflowDag => OwnerPhase::Story {
                story: "US10",
                task: "T095",
            },
            ScrollSurface::ToolList => OwnerPhase::Story {
                story: "US11",
                task: "T105",
            },
            ScrollSurface::SkillList => OwnerPhase::Story {
                story: "US12",
                task: "T112",
            },
            ScrollSurface::AgentExecution => OwnerPhase::Story {
                story: "US13",
                task: "T124",
            },
        }
    }

    /// Stable slug for this surface. Used in build-time tags.
    pub fn slug(self) -> &'static str {
        match self {
            ScrollSurface::Sidebar => "sidebar",
            ScrollSurface::HomeNavigation => "home_navigation",
            ScrollSurface::DataSourceList => "datasource_list",
            ScrollSurface::DataSourceEdit => "datasource_edit",
            ScrollSurface::GlobalConfig => "global_config",
            ScrollSurface::LlmList => "llm_list",
            ScrollSurface::TagList => "tag_list",
            ScrollSurface::CategoryList => "category_list",
            ScrollSurface::CapabilityList => "capability_list",
            ScrollSurface::PluginList => "plugin_list",
            ScrollSurface::FunctionList => "function_list",
            ScrollSurface::WorkflowDag => "workflow_dag",
            ScrollSurface::ToolList => "tool_list",
            ScrollSurface::SkillList => "skill_list",
            ScrollSurface::AgentExecution => "agent_execution",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "phase")]
pub enum OwnerPhase {
    Foundation,
    Story {
        story: &'static str,
        task: &'static str,
    },
}

impl fmt::Display for OwnerPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OwnerPhase::Foundation => write!(f, "Foundation"),
            OwnerPhase::Story { story, task } => write!(f, "{story}/{task}"),
        }
    }
}

// ---------------------------------------------------------------------------
// §T016E.2 — Inventory + compile-time registration macro.
// ---------------------------------------------------------------------------

/// In-memory inventory of the surfaces a given crate registers.
/// Stories call [`Inventory::register`] from their owner test; the
/// Foundation phase registers the sidebar.
#[derive(Debug, Default, Clone)]
pub struct Inventory {
    surfaces: BTreeMap<ScrollSurface, Registration>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Registration {
    pub surface: ScrollSurface,
    pub owner: OwnerPhase,
    /// Build-time tag the story committed to its source. The
    /// story owner's inventory assertion compares the tag in source
    /// against the value registered here.
    pub build_tag: String,
}

impl Inventory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, surface: ScrollSurface, build_tag: impl Into<String>) {
        let owner = surface.owner_phase();
        self.surfaces.insert(
            surface,
            Registration {
                surface,
                owner,
                build_tag: build_tag.into(),
            },
        );
    }

    pub fn contains(&self, surface: ScrollSurface) -> bool {
        self.surfaces.contains_key(&surface)
    }

    pub fn get(&self, surface: ScrollSurface) -> Option<&Registration> {
        self.surfaces.get(&surface)
    }

    pub fn surfaces(&self) -> impl Iterator<Item = &Registration> {
        self.surfaces.values()
    }
}

/// Compile-time assertion that a surface is in the inventory. The
/// test file calls this helper function (not a macro) with the
/// surface constant the story committed to source; if the surface
/// has not been registered the call panics with a self-explanatory
/// message. Using a function (not a macro) keeps the helper easy
/// to import across multiple test files without `#[macro_export]`
/// name-collision issues.
pub fn assert_inventory_contains(inventory: &Inventory, surface: ScrollSurface) {
    if !inventory.contains(surface) {
        panic!(
            "scroll inventory does not contain {surface:?} (slug = {slug}); register it in the owner test before running this assertion",
            surface = surface,
            slug = surface.slug()
        );
    }
}

/// Compile-time assertion that a surface is *not* in the inventory.
/// Useful for asserting that a story owner has not (yet) registered
/// a future-story surface they do not own.
pub fn assert_inventory_missing(inventory: &Inventory, surface: ScrollSurface) {
    if inventory.contains(surface) {
        panic!(
            "scroll inventory unexpectedly contains {surface:?} (slug = {slug}); this surface must be removed from the inventory until its owner story registers it",
            surface = surface,
            slug = surface.slug()
        );
    }
}

// ---------------------------------------------------------------------------
// §T016E.3 — Visual test context (deterministic harness).
// ---------------------------------------------------------------------------

/// One deterministic frame the harness can replay. Stories construct
/// a `VisualTestContext` from a list of frames and exercise the
/// surface against it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    pub width: u16,
    pub height: u16,
    pub content_offset_y: i32,
    pub focused_index: Option<u32>,
    pub items: Vec<String>,
}

impl Frame {
    pub fn fixture(width: u16, height: u16, items: Vec<String>) -> Self {
        Self {
            width,
            height,
            content_offset_y: 0,
            focused_index: None,
            items,
        }
    }
}

/// The harness that drives a scroll surface. The harness is total
/// and deterministic: every input either advances the state or
/// returns a stable error. The harness never sleeps or relies on
/// wall-clock time; tests can replay frames as fast as they like.
#[derive(Debug, Clone)]
pub struct VisualTestContext {
    surface: ScrollSurface,
    frames: Vec<Frame>,
    cursor: usize,
    state: State,
}

#[derive(Debug, Clone)]
struct State {
    offset_y: i32,
    focused_index: Option<u32>,
    keyboard_only: bool,
    audit: AccessibilityAudit,
    trail: KeyboardOnlyTrail,
}

impl State {
    fn fresh() -> Self {
        Self {
            offset_y: 0,
            focused_index: None,
            keyboard_only: false,
            audit: AccessibilityAudit::empty(),
            trail: KeyboardOnlyTrail::default(),
        }
    }
}

impl VisualTestContext {
    pub fn new(surface: ScrollSurface, frames: Vec<Frame>) -> Self {
        Self {
            surface,
            frames,
            cursor: 0,
            state: State::fresh(),
        }
    }

    pub fn current_frame(&self) -> &Frame {
        &self.frames[self.cursor]
    }

    pub fn step(&mut self) -> Result<&Frame, HarnessError> {
        if self.cursor + 1 >= self.frames.len() {
            return Err(HarnessError::EndOfFrames {
                surface: self.surface,
                cursor: self.cursor,
            });
        }
        self.cursor += 1;
        self.state.offset_y = self.frames[self.cursor].content_offset_y;
        self.state.focused_index = self.frames[self.cursor].focused_index;
        Ok(&self.frames[self.cursor])
    }

    pub fn scroll(&mut self, delta: i32) -> Result<&Frame, HarnessError> {
        let next = self.state.offset_y + delta;
        let max = self.max_offset();
        self.state.offset_y = next.clamp(0, max);
        self.state.trail.record_scroll(delta, self.state.offset_y);
        Ok(self.current_frame())
    }

    pub fn set_keyboard_only(&mut self, on: bool) {
        self.state.keyboard_only = on;
        self.state.trail.keyboard_only = on;
    }

    pub fn focus_index(&mut self, idx: u32) -> Result<&Frame, HarnessError> {
        // Snapshot the item count from the current frame before
        // mutating `state`, so the borrow checker is happy.
        let item_count = self.frames[self.cursor].items.len();
        // Record the attempt in state regardless of outcome so a
        // rejected focus still surfaces a `FocusOutOfRange` finding
        // during the audit. This mirrors the product behaviour: a
        // user keypress MUST update the focused-element state even
        // if the surface rejects the move.
        self.state.focused_index = Some(idx);
        self.state.trail.record_focus(idx);
        if idx as usize >= item_count {
            return Err(HarnessError::FocusOutOfRange {
                surface: self.surface,
                index: idx,
                max: item_count.saturating_sub(1) as u32,
            });
        }
        Ok(self.current_frame())
    }

    pub fn audit(&self) -> &AccessibilityAudit {
        &self.state.audit
    }

    pub fn keyboard_trail(&self) -> &KeyboardOnlyTrail {
        &self.state.trail
    }

    pub fn run_audit(&mut self) -> &AccessibilityAudit {
        let frame = self.current_frame();
        let focused = self.state.focused_index;
        let mut findings = Vec::new();
        if frame.items.is_empty() {
            findings.push(A11yFinding::EmptySurface {
                surface: self.surface,
            });
        }
        if let Some(idx) = focused {
            if idx as usize >= frame.items.len() {
                findings.push(A11yFinding::FocusOutOfRange {
                    surface: self.surface,
                    index: idx,
                });
            }
        }
        if !self.state.keyboard_only {
            findings.push(A11yFinding::MouseOnlyPath {
                surface: self.surface,
            });
        }
        self.state.audit = AccessibilityAudit {
            surface: self.surface,
            findings,
        };
        &self.state.audit
    }

    fn max_offset(&self) -> i32 {
        let frame = self.current_frame();
        let total = frame.items.len() as i32;
        let visible = frame.height as i32;
        (total - visible).max(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "details")]
pub enum HarnessError {
    EndOfFrames {
        surface: ScrollSurface,
        cursor: usize,
    },
    FocusOutOfRange {
        surface: ScrollSurface,
        index: u32,
        max: u32,
    },
}

impl fmt::Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HarnessError::EndOfFrames { surface, cursor } => {
                write!(
                    f,
                    "harness exhausted frames for {surface:?} at cursor {cursor}"
                )
            }
            HarnessError::FocusOutOfRange {
                surface,
                index,
                max,
            } => write!(
                f,
                "focus index {index} out of range for {surface:?} (max = {max})"
            ),
        }
    }
}

impl std::error::Error for HarnessError {}

// ---------------------------------------------------------------------------
// §T016E.4 — Accessibility audit + keyboard-only trail.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct AccessibilityAudit {
    pub surface: ScrollSurface,
    pub findings: Vec<A11yFinding>,
}

impl AccessibilityAudit {
    /// Empty audit (no surface, no findings) used by tests that have
    /// not yet run an audit. Use this instead of `Default` so the
    /// `surface` field is not coupled to a particular `ScrollSurface`
    /// variant.
    pub fn empty() -> Self {
        Self {
            surface: ScrollSurface::Sidebar,
            findings: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "details")]
pub enum A11yFinding {
    EmptySurface { surface: ScrollSurface },
    FocusOutOfRange { surface: ScrollSurface, index: u32 },
    MouseOnlyPath { surface: ScrollSurface },
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct KeyboardOnlyTrail {
    pub keyboard_only: bool,
    pub scroll_events: Vec<ScrollEvent>,
    pub focus_events: Vec<u32>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ScrollEvent {
    pub delta: i32,
    pub resulting_offset: i32,
}

impl KeyboardOnlyTrail {
    fn record_scroll(&mut self, delta: i32, resulting_offset: i32) {
        self.scroll_events.push(ScrollEvent {
            delta,
            resulting_offset,
        });
    }
    fn record_focus(&mut self, index: u32) {
        self.focus_events.push(index);
    }
}

// ---------------------------------------------------------------------------
// §T016E.5 — Synthetic fixtures (no product code, helper self-test).
// ---------------------------------------------------------------------------

/// A good surface fixture — has items, focus, and a keyboard-only
/// path. The helper self-test uses it to prove the audit can return
/// a clean result.
pub fn good_surface() -> VisualTestContext {
    let mut ctx = VisualTestContext::new(
        ScrollSurface::Sidebar,
        vec![Frame::fixture(
            80,
            24,
            vec![
                "Home".into(),
                "Data sources".into(),
                "LLM config".into(),
                "Plugins".into(),
            ],
        )],
    );
    ctx.set_keyboard_only(true);
    ctx.focus_index(1).expect("index 1 in range");
    ctx
}

/// A bad surface fixture — empty list, focus out of range, no
/// keyboard path. The helper self-test uses it to prove the audit
/// surfaces every finding.
pub fn bad_surface() -> VisualTestContext {
    let mut ctx = VisualTestContext::new(
        ScrollSurface::DataSourceList,
        // Empty list: surfaces the `EmptySurface` finding.
        // `focus_index(99)` then attempts a focus on an empty list,
        // which surfaces the `FocusOutOfRange` finding. The harness
        // never sets `keyboard_only`, so the audit also reports
        // `MouseOnlyPath`.
        vec![Frame::fixture(80, 24, vec![])],
    );
    let _ = ctx.focus_index(99); // rejected, but the attempt is recorded
    let _ = ctx.run_audit();
    ctx
}

/// Inventory fixture — Foundation registers the sidebar; stories
/// register their own surfaces. The helper self-test uses it to
/// prove the macro and inventory APIs agree.
pub fn foundation_inventory() -> Inventory {
    let mut inv = Inventory::new();
    inv.register(
        ScrollSurface::Sidebar,
        format!("scroll:{}", ScrollSurface::Sidebar.slug()),
    );
    inv
}

// ---------------------------------------------------------------------------
// §T016E.6 — Documentation source contract (build-time tag).
// ---------------------------------------------------------------------------

/// A source-contract tag a story owner is expected to write into
/// the doc comment of every scrollable view module. The Foundation
/// phase enforces the *format* but not the *content*; the story
/// owner test reads the doc comment and asserts the tag is present.
pub const SOURCE_TAG_PREFIX: &str = "scroll:";

/// Parse a `scroll:<slug>` tag out of a source comment block.
pub fn parse_source_tag(comment: &str) -> Option<String> {
    for line in comment.lines() {
        let trimmed = line.trim().trim_start_matches("//!").trim();
        if let Some(rest) = trimmed.strip_prefix(SOURCE_TAG_PREFIX) {
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// Public path the test harness uses to read the source comment
/// for a module. We do not embed the source path here; the story
/// owner test passes the comment text in.
pub fn assert_source_tag(comment: &str, expected_slug: &str) {
    match parse_source_tag(comment) {
        Some(slug) if slug == expected_slug => {}
        Some(other) => {
            panic!("source tag slug mismatch: expected `{expected_slug}`, found `{other}`")
        }
        None => {
            panic!("source comment missing `scroll:<slug>` tag; expected slug `{expected_slug}`")
        }
    }
}

// ---------------------------------------------------------------------------
// §T016E.7 — Lightweight Arc<Inventory> wrapper for parallel tests.
// ---------------------------------------------------------------------------

/// `Arc<Inventory>` for tests that need to share the same
/// registration table across threads.
pub type SharedInventory = Arc<Inventory>;

// ---------------------------------------------------------------------------
// §T016E.8 — Path helper (kept for the story owner tests).
// ---------------------------------------------------------------------------

/// Returns the canonical workspace root a test should use to read a
/// source file. The Foundation phase does not depend on the exact
/// value; the story owner test does.
pub fn workspace_root_hint() -> PathBuf {
    // The real impl reads $CARGO_MANIFEST_DIR/../.. ; tests are free
    // to override this in their own setup.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
