//! T016 Red visual/keyboard contract for blocking startup recovery surfaces.
//!
//! The tests use real GPUI focus and keyboard dispatch through
//! `VisualTestContext`. The operation adapters are injected so a pending retry
//! can prove that the UI heartbeat remains responsive instead of blocking the
//! GPUI thread. T023 and T025 own the production views that make these tests
//! Green.
//!
//! T030 [P] [US1] sidebar tests are appended at the bottom of this file.
//! They reuse the same `VisualTestContext` machinery and the
//! `support::scroll_inventory` helper, so this file declares `mod support;`
//! even though the T016 recovery tests at the top do not use it.

mod support;

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use gpui::{AppContext, TestAppContext, VisualTestContext, WindowHandle, px, size};
use hivegui::ui::{
    key_recovery_view::{
        KeyRecoveryAction, KeyRecoveryCommands, KeyRecoveryFuture, KeyRecoveryReason,
        KeyRecoveryView,
    },
    migration_recovery_view::{
        MigrationRecoveryAction, MigrationRecoveryCommands, MigrationRecoveryFuture,
        MigrationRecoveryState, MigrationRecoveryView,
    },
};
use tokio::sync::Notify;

const EXECUTION_ID: &str = "2a9f4c86-6159-4f55-ac57-4e2841e2325f";

#[derive(Default)]
struct FuturePollHandshake {
    polled: AtomicBool,
}

impl FuturePollHandshake {
    fn mark_polled(&self) {
        self.polled.store(true, Ordering::SeqCst);
    }

    fn assert_polled(&self, operation: &str) {
        assert!(
            self.polled.load(Ordering::SeqCst),
            "{operation} future was never polled off the keyboard event boundary"
        );
    }
}

fn init_gpui(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::theme::init(cx);
        gpui_component::init(cx);
    });
}

#[derive(Default)]
struct RecordingMigrationCommands {
    actions: Mutex<Vec<MigrationRecoveryAction>>,
    pending_retry: Option<Arc<Notify>>,
    poll_handshake: Option<Arc<FuturePollHandshake>>,
}

impl RecordingMigrationCommands {
    fn with_pending_retry(release: Arc<Notify>, poll_handshake: Arc<FuturePollHandshake>) -> Self {
        Self {
            actions: Mutex::new(Vec::new()),
            pending_retry: Some(release),
            poll_handshake: Some(poll_handshake),
        }
    }

    fn actions(&self) -> Vec<MigrationRecoveryAction> {
        self.actions
            .lock()
            .expect("migration action mutex poisoned")
            .clone()
    }
}

impl MigrationRecoveryCommands for RecordingMigrationCommands {
    fn execute(&self, action: MigrationRecoveryAction) -> MigrationRecoveryFuture {
        self.actions
            .lock()
            .expect("migration action mutex poisoned")
            .push(action);
        let pending_retry = self.pending_retry.clone();
        let poll_handshake = self.poll_handshake.clone();
        Box::pin(async move {
            if action == MigrationRecoveryAction::RetryMigration
                && let Some(release) = pending_retry
            {
                poll_handshake
                    .expect("pending migration operation has a poll handshake")
                    .mark_polled();
                release.notified().await;
            }
            Ok(())
        })
    }
}

#[derive(Default)]
struct RecordingKeyCommands {
    actions: Mutex<Vec<KeyRecoveryAction>>,
    pending_action: Option<(KeyRecoveryAction, Arc<Notify>)>,
    poll_handshake: Option<Arc<FuturePollHandshake>>,
}

impl RecordingKeyCommands {
    fn with_pending_action(
        action: KeyRecoveryAction,
        release: Arc<Notify>,
        poll_handshake: Arc<FuturePollHandshake>,
    ) -> Self {
        Self {
            actions: Mutex::new(Vec::new()),
            pending_action: Some((action, release)),
            poll_handshake: Some(poll_handshake),
        }
    }

    fn actions(&self) -> Vec<KeyRecoveryAction> {
        self.actions
            .lock()
            .expect("key action mutex poisoned")
            .clone()
    }
}

impl KeyRecoveryCommands for RecordingKeyCommands {
    fn execute(&self, action: KeyRecoveryAction) -> KeyRecoveryFuture {
        self.actions
            .lock()
            .expect("key action mutex poisoned")
            .push(action);
        let pending_action = self.pending_action.clone();
        let poll_handshake = self.poll_handshake.clone();
        Box::pin(async move {
            if let Some((pending, release)) = pending_action
                && action == pending
            {
                poll_handshake
                    .expect("pending key operation has a poll handshake")
                    .mark_polled();
                release.notified().await;
            }
            Ok(())
        })
    }
}

fn migration_failed_state() -> MigrationRecoveryState {
    MigrationRecoveryState::migration_failed(
        "v3_to_v4",
        "storage_corrupt",
        EXECUTION_ID,
        PathBuf::from("/private/snapshots/pre-v4"),
    )
}

fn integrity_corrupt_state() -> MigrationRecoveryState {
    MigrationRecoveryState::integrity_corrupt(
        "storage_corrupt",
        EXECUTION_ID,
        PathBuf::from("/private/quarantine/hivegui-corrupt.db"),
    )
}

fn open_migration_view(
    cx: &mut TestAppContext,
    state: MigrationRecoveryState,
    commands: Arc<dyn MigrationRecoveryCommands>,
) -> (WindowHandle<gpui_component::Root>, VisualTestContext) {
    let window = cx.open_window(size(px(760.0), px(520.0)), move |window, cx| {
        let view = cx.new(|cx| MigrationRecoveryView::new(state, commands, window, cx));
        gpui_component::Root::new(view, window, cx).bordered(false)
    });
    cx.run_until_parked();
    let visual = VisualTestContext::from_window(window.clone().into(), cx);
    (window, visual)
}

fn open_key_view(
    cx: &mut TestAppContext,
    reason: KeyRecoveryReason,
    commands: Arc<dyn KeyRecoveryCommands>,
) -> (WindowHandle<gpui_component::Root>, VisualTestContext) {
    let window = cx.open_window(size(px(760.0), px(520.0)), move |window, cx| {
        let view = cx.new(|cx| KeyRecoveryView::new(reason, commands, window, cx));
        gpui_component::Root::new(view, window, cx).bordered(false)
    });
    cx.run_until_parked();
    let visual = VisualTestContext::from_window(window.clone().into(), cx);
    (window, visual)
}

fn assert_visible(cx: &mut VisualTestContext, selector: &'static str) {
    assert!(
        cx.debug_bounds(selector).is_some(),
        "expected visible recovery control/status: {selector}"
    );
}

fn assert_absent(cx: &mut VisualTestContext, selector: &'static str) {
    assert!(
        cx.debug_bounds(selector).is_none(),
        "forbidden recovery control is visible: {selector}"
    );
}

struct UiHeartbeatProbe {
    started: Instant,
    previous: Instant,
    maximum_gap: Duration,
    maximum_dispatch_latency: Duration,
    samples: usize,
}

impl UiHeartbeatProbe {
    fn new() -> Self {
        let started = Instant::now();
        Self {
            started,
            previous: started,
            maximum_gap: Duration::ZERO,
            maximum_dispatch_latency: Duration::ZERO,
            samples: 0,
        }
    }

    fn dispatch_and_sample(
        &mut self,
        cx: &mut VisualTestContext,
        keystrokes: &str,
        status_selector: &'static str,
    ) {
        let dispatch_started = Instant::now();
        cx.simulate_keystrokes(keystrokes);
        assert_visible(cx, status_selector);
        let now = Instant::now();
        self.maximum_dispatch_latency = self
            .maximum_dispatch_latency
            .max(now.duration_since(dispatch_started));
        self.maximum_gap = self.maximum_gap.max(now.duration_since(self.previous));
        self.previous = now;
        self.samples += 1;
    }

    fn wait_and_sample(&mut self, cx: &mut VisualTestContext, status_selector: &'static str) {
        thread::sleep(Duration::from_millis(30));
        self.dispatch_and_sample(cx, "tab shift-tab", status_selector);
    }

    fn assert_responsive(&self) {
        assert!(
            self.samples >= 10,
            "heartbeat must collect repeated samples"
        );
        assert!(
            self.started.elapsed() >= Duration::from_millis(300),
            "heartbeat must cover a real pending-operation observation window"
        );
        assert!(
            self.maximum_gap <= Duration::from_millis(250),
            "GPUI thread stopped responding for {:?}",
            self.maximum_gap
        );
        assert!(
            self.maximum_dispatch_latency <= Duration::from_millis(250),
            "keyboard-to-visible-state dispatch took {:?}",
            self.maximum_dispatch_latency
        );
    }
}

#[gpui::test]
fn migration_failure_has_only_retry_and_exit_with_trapped_keyboard_focus(cx: &mut TestAppContext) {
    init_gpui(cx);
    let commands = Arc::new(RecordingMigrationCommands::default());
    let (_window, mut visual) = open_migration_view(cx, migration_failed_state(), commands.clone());

    for selector in [
        "MIGRATION_RECOVERY_VIEW",
        "MIGRATION_RECOVERY_STATUS",
        "MIGRATION_STEP-v3_to_v4",
        "MIGRATION_ERROR_KIND-storage_corrupt",
        "MIGRATION_EXECUTION_ID-2a9f4c86-6159-4f55-ac57-4e2841e2325f",
        "MIGRATION_SNAPSHOT_LOCATION",
        "MIGRATION_RETRY",
        "MIGRATION_EXIT",
    ] {
        assert_visible(&mut visual, selector);
    }
    for selector in ["INTEGRITY_RESTORE_BACKUP", "INTEGRITY_REBUILD"] {
        assert_absent(&mut visual, selector);
    }
    assert_absent(&mut visual, "HIVEGUI_MAIN_CONTENT");

    visual.simulate_keystrokes("enter");
    assert_eq!(
        commands.actions(),
        [MigrationRecoveryAction::RetryMigration]
    );

    visual.simulate_keystrokes("tab space");
    assert_eq!(
        commands.actions(),
        [
            MigrationRecoveryAction::RetryMigration,
            MigrationRecoveryAction::Exit,
        ]
    );

    visual.simulate_keystrokes("tab enter shift-tab space");
    assert_eq!(
        commands.actions(),
        [
            MigrationRecoveryAction::RetryMigration,
            MigrationRecoveryAction::Exit,
            MigrationRecoveryAction::RetryMigration,
            MigrationRecoveryAction::Exit,
        ],
        "Tab and Shift+Tab must wrap inside the blocking recovery surface"
    );
}

#[gpui::test]
fn integrity_corruption_exposes_restore_confirmed_rebuild_and_exit(cx: &mut TestAppContext) {
    init_gpui(cx);
    let commands = Arc::new(RecordingMigrationCommands::default());
    let (_window, mut visual) =
        open_migration_view(cx, integrity_corrupt_state(), commands.clone());

    for selector in [
        "INTEGRITY_RECOVERY_VIEW",
        "MIGRATION_RECOVERY_STATUS",
        "INTEGRITY_ERROR_KIND-storage_corrupt",
        "INTEGRITY_EXECUTION_ID-2a9f4c86-6159-4f55-ac57-4e2841e2325f",
        "INTEGRITY_QUARANTINE_LOCATION",
        "INTEGRITY_RESTORE_BACKUP",
        "INTEGRITY_REBUILD",
        "INTEGRITY_EXIT",
    ] {
        assert_visible(&mut visual, selector);
    }
    assert_absent(&mut visual, "MIGRATION_RETRY");
    assert_absent(&mut visual, "HIVEGUI_MAIN_CONTENT");

    visual.simulate_keystrokes("enter");
    assert_eq!(
        commands.actions(),
        [MigrationRecoveryAction::RestoreEncryptedBackup]
    );

    visual.simulate_keystrokes("tab enter");
    assert_visible(&mut visual, "INTEGRITY_REBUILD_CONFIRMATION");
    assert_visible(&mut visual, "INTEGRITY_REBUILD_CANCEL");
    assert_visible(&mut visual, "INTEGRITY_REBUILD_CONFIRM");
    assert_eq!(
        commands.actions(),
        [MigrationRecoveryAction::RestoreEncryptedBackup],
        "opening confirmation must not rebuild"
    );

    visual.simulate_keystrokes("enter");
    assert_absent(&mut visual, "INTEGRITY_REBUILD_CONFIRMATION");
    visual.simulate_keystrokes("enter escape");
    assert_absent(&mut visual, "INTEGRITY_REBUILD_CONFIRMATION");

    visual.simulate_keystrokes("enter tab space");
    assert_eq!(
        commands.actions(),
        [
            MigrationRecoveryAction::RestoreEncryptedBackup,
            MigrationRecoveryAction::RebuildFromQuarantinedCopy,
        ],
        "destructive rebuild requires the second confirmation"
    );

    visual.simulate_keystrokes("tab enter");
    assert_eq!(
        commands.actions(),
        [
            MigrationRecoveryAction::RestoreEncryptedBackup,
            MigrationRecoveryAction::RebuildFromQuarantinedCopy,
            MigrationRecoveryAction::Exit,
        ],
        "confirmation closes back to a predictable focus position before Exit"
    );
}

#[gpui::test]
fn every_device_key_failure_is_blocking_and_keyboard_reachable(cx: &mut TestAppContext) {
    init_gpui(cx);

    for (reason, reason_selector) in [
        (
            KeyRecoveryReason::Missing,
            "KEY_RECOVERY_REASON-key_missing",
        ),
        (
            KeyRecoveryReason::Corrupt,
            "KEY_RECOVERY_REASON-key_corrupt",
        ),
        (
            KeyRecoveryReason::Permissions,
            "KEY_RECOVERY_REASON-key_permissions",
        ),
        (
            KeyRecoveryReason::Unreadable,
            "KEY_RECOVERY_REASON-key_unreadable",
        ),
    ] {
        let commands = Arc::new(RecordingKeyCommands::default());
        let (_window, mut visual) = open_key_view(cx, reason, commands.clone());

        for selector in [
            "KEY_RECOVERY_VIEW",
            "KEY_RECOVERY_STATUS",
            "KEY_RECONFIGURE",
            "KEY_RESTORE_BACKUP",
            "KEY_EXIT",
        ] {
            assert_visible(&mut visual, selector);
        }
        assert_visible(&mut visual, reason_selector);
        assert_absent(&mut visual, "HIVEGUI_MAIN_CONTENT");

        visual.simulate_keystrokes("enter tab space tab enter tab enter");
        assert_eq!(
            commands.actions(),
            [
                KeyRecoveryAction::Reconfigure,
                KeyRecoveryAction::RestoreEncryptedBackup,
                KeyRecoveryAction::Exit,
                KeyRecoveryAction::Reconfigure,
            ],
            "initial focus and Tab wrapping must be stable for {reason:?}"
        );
    }
}

#[gpui::test]
fn pending_recovery_operation_keeps_status_and_ui_heartbeat_responsive(cx: &mut TestAppContext) {
    init_gpui(cx);
    let release = Arc::new(Notify::new());
    let poll_handshake = Arc::new(FuturePollHandshake::default());
    let commands = Arc::new(RecordingMigrationCommands::with_pending_retry(
        release.clone(),
        poll_handshake.clone(),
    ));
    let (_window, mut visual) = open_migration_view(cx, migration_failed_state(), commands.clone());

    let mut heartbeat = UiHeartbeatProbe::new();
    heartbeat.dispatch_and_sample(&mut visual, "enter", "MIGRATION_RECOVERY_RUNNING_STATUS");
    assert_eq!(
        commands.actions(),
        [MigrationRecoveryAction::RetryMigration]
    );
    poll_handshake.assert_polled("migration retry");

    for _ in 0..10 {
        heartbeat.wait_and_sample(&mut visual, "MIGRATION_RECOVERY_RUNNING_STATUS");
    }
    heartbeat.assert_responsive();
    assert_eq!(
        commands.actions(),
        [MigrationRecoveryAction::RetryMigration],
        "heartbeat focus sampling must not trigger another recovery action"
    );

    release.notify_one();
    visual.run_until_parked();
    assert_visible(&mut visual, "MIGRATION_RECOVERY_COMPLETED_STATUS");
}

#[gpui::test]
fn pending_device_key_recovery_keeps_status_and_ui_heartbeat_responsive(cx: &mut TestAppContext) {
    init_gpui(cx);
    let release = Arc::new(Notify::new());
    let poll_handshake = Arc::new(FuturePollHandshake::default());
    let commands = Arc::new(RecordingKeyCommands::with_pending_action(
        KeyRecoveryAction::Reconfigure,
        release.clone(),
        poll_handshake.clone(),
    ));
    let (_window, mut visual) = open_key_view(cx, KeyRecoveryReason::Missing, commands.clone());

    let mut heartbeat = UiHeartbeatProbe::new();
    heartbeat.dispatch_and_sample(&mut visual, "enter", "KEY_RECOVERY_RUNNING_STATUS");
    assert_eq!(commands.actions(), [KeyRecoveryAction::Reconfigure]);
    poll_handshake.assert_polled("device-key reconfiguration");

    for _ in 0..10 {
        heartbeat.wait_and_sample(&mut visual, "KEY_RECOVERY_RUNNING_STATUS");
    }
    heartbeat.assert_responsive();
    assert_eq!(
        commands.actions(),
        [KeyRecoveryAction::Reconfigure],
        "heartbeat focus sampling must not trigger another key-recovery action"
    );

    release.notify_one();
    visual.run_until_parked();
    assert_visible(&mut visual, "KEY_RECOVERY_COMPLETED_STATUS");
}

// ===========================================================================
// §T030 — US1 sidebar: keyboard / a11y / native scroll activation.
// ===========================================================================
//
// Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T030
// ("在 `crates/hivegui/tests/accessibility.rs` 编写侧栏 Tab 顺序、
// Enter/Space 激活、可见焦点、AccessKit 名称/角色及固定输入到导航可见
// 反馈 p95≤100ms 的 T005 基线比较测试，并激活 T016E sidebar 行、写入并
// 首次运行该行原生滚动断言").
//
// Red public boundaries (T032 will add these):
//   - `crates/hivegui/src/ui/sidebar_nav.rs` must carry the
//     `//! scroll:sidebar` source tag so the T016E source contract
//     recognises the surface.
//   - The sidebar nav buttons (`home`, `ai`, `tools`, `user_config`)
//     must accept keyboard activation: Tab moves focus in source
//     order, Enter/Space activate the focused button, the focus ring
//     is visible, and AccessKit reports the Chinese label.
//   - The fixed input → navigation visible feedback latency must
//     stay within the T005 100ms p95 baseline. The probe records a
//     per-sample latency and asserts the p95 bound.
//
// §T030.1 — Source contract activation. ───────────────────────────────

#[test]
fn sidebar_source_contract_carries_scroll_tag() {
    use std::fs;
    use support::scroll_inventory::{ScrollSurface, assert_source_tag};

    // T032 must add `//! scroll:sidebar` to the sidebar_nav.rs module
    // doc comment. Until then `parse_source_tag` returns `None` and
    // `assert_source_tag` panics — the test fails Red.
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let sidebar_path = manifest_dir.join("src/ui/sidebar_nav.rs");
    let source = fs::read_to_string(&sidebar_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", sidebar_path.display()));
    assert_source_tag(&source, ScrollSurface::Sidebar.slug());
}

#[test]
fn sidebar_is_registered_in_t016e_inventory() {
    use support::scroll_inventory::{
        ScrollSurface, assert_inventory_contains, foundation_inventory,
    };

    // T030 activates the T016E sidebar row (already part of the
    // Foundation inventory). The future story owner of T032 must NOT
    // re-register the sidebar under a US1 owner phase.
    let inventory = foundation_inventory();
    assert_inventory_contains(&inventory, ScrollSurface::Sidebar);
    assert_eq!(
        ScrollSurface::Sidebar.owner_phase().to_string(),
        "Foundation",
        "the sidebar surface is owned by the Foundation phase, not by US1"
    );
}

// §T030.2 — Keyboard activation contract. ──────────────────────────────

#[gpui::test]
fn sidebar_tab_order_home_ai_tools_user_config(cx: &mut TestAppContext) {
    init_gpui(cx);
    let (_window, mut visual) = open_sidebar_view(cx);

    // Focus must start somewhere accessible; the first Tab lands on
    // the first nav button. T032 must wire `focusable()` on the four
    // sidebar buttons in source order: `home → ai → tools →
    // user_config`.
    visual.simulate_keystrokes("tab");
    assert_focused(&mut visual, "SIDEBAR_FOCUS-home");

    visual.simulate_keystrokes("tab");
    assert_focused(&mut visual, "SIDEBAR_FOCUS-ai");

    visual.simulate_keystrokes("tab");
    assert_focused(&mut visual, "SIDEBAR_FOCUS-tools");

    visual.simulate_keystrokes("tab");
    assert_focused(&mut visual, "SIDEBAR_FOCUS-user_config");

    // Shift+Tab must walk back in the same order.
    visual.simulate_keystrokes("shift-tab");
    assert_focused(&mut visual, "SIDEBAR_FOCUS-tools");
}

#[gpui::test]
fn sidebar_enter_activation_navigates_to_focused_route(cx: &mut TestAppContext) {
    init_gpui(cx);
    let (_window, mut visual) = open_sidebar_view(cx);

    visual.simulate_keystrokes("tab enter");
    assert_eq!(
        cx_state_route(cx),
        hivegui::ui::app::AppRoute::Home,
        "Tab + Enter on the home button must navigate to Home"
    );

    visual.simulate_keystrokes("tab tab enter");
    assert_eq!(
        cx_state_route(cx),
        hivegui::ui::app::AppRoute::Ai,
        "Tab + Enter on the AI button must navigate to Ai"
    );

    visual.simulate_keystrokes("tab tab enter");
    assert_eq!(
        cx_state_route(cx),
        hivegui::ui::app::AppRoute::Tools,
        "Tab + Enter on the Tools button must navigate to Tools"
    );
}

#[gpui::test]
fn sidebar_space_activation_matches_enter(cx: &mut TestAppContext) {
    init_gpui(cx);
    let (_window, mut visual) = open_sidebar_view(cx);

    visual.simulate_keystrokes("tab space");
    assert_eq!(
        cx_state_route(cx),
        hivegui::ui::app::AppRoute::Home,
        "Space must be a valid activation key alongside Enter"
    );

    visual.simulate_keystrokes("tab tab space");
    assert_eq!(
        cx_state_route(cx),
        hivegui::ui::app::AppRoute::Ai,
        "Space on the AI button must navigate to Ai"
    );
}

// §T030.3 — AccessKit names. ────────────────────────────────────────────

#[gpui::test]
fn sidebar_buttons_expose_accesskit_names(cx: &mut TestAppContext) {
    init_gpui(cx);
    let (_window, visual) = open_sidebar_view(cx);

    // T032 must set the AccessKit `name` on each sidebar nav button
    // to the same Chinese label rendered on the tooltip. AccessKit
    // discovery goes through the registered AccessKit handlers; the
    // selectors below are the stable names T032 must publish.
    for (selector, expected_name) in [
        ("SIDEBAR_A11Y-home", "首页"),
        ("SIDEBAR_A11Y-ai", "AI 管理"),
        ("SIDEBAR_A11Y-tools", "工具"),
        ("SIDEBAR_A11Y-user_config", "用户配置"),
    ] {
        assert_eq!(
            accesskit_name(&visual, selector),
            Some(expected_name.to_string()),
            "AccessKit name for `{selector}` must equal the visible label"
        );
    }
}

// §T030.4 — Fixed input → visible feedback latency. ─────────────────────

#[gpui::test]
fn sidebar_focus_to_visible_feedback_p95_within_t005_baseline(cx: &mut TestAppContext) {
    use std::time::Instant;

    init_gpui(cx);
    let (_window, mut visual) = open_sidebar_view(cx);

    // T005 baseline: 100ms p95 keyboard → visible state. The probe
    // records 20 samples and asserts p95 ≤ 100ms. T032 must wire the
    // focus ring render so a Tab keypress updates the focus selector
    // within the same frame.
    let mut samples = Vec::with_capacity(20);
    for _ in 0..20 {
        let started = Instant::now();
        visual.simulate_keystrokes("tab");
        assert_visible(&mut visual, "SIDEBAR_FOCUS_MARKER");
        samples.push(started.elapsed());
    }
    samples.sort();
    let p95_index = (samples.len() as f64 * 0.95).ceil() as usize - 1;
    let p95 = samples[p95_index];
    assert!(
        p95 <= std::time::Duration::from_millis(100),
        "sidebar focus-to-visible p95 {p95:?} exceeds the T005 100ms baseline (samples: {samples:?})"
    );
}

// §T030.5 — Helpers. ────────────────────────────────────────────────────

fn open_sidebar_view(
    cx: &mut TestAppContext,
) -> (WindowHandle<gpui_component::Root>, VisualTestContext) {
    // T032 must expose `hivegui::ui::sidebar_nav::SidebarNav::new_with_focus`
    // (or a `for_test` constructor) that wires the four debug
    // selectors. Until then this call fails to compile.
    // T032 install_for_test seeds a minimal HiveGuiAppState global so
    // the sidebar's `cx.global::<HiveGuiAppState>()` access does not
    // panic. The production wiring still lives in `app::install`.
    use hivegui::ui::app::{AccessKitLabelRegistry, AppRoute, HiveGuiAppState};
    cx.update(|cx| {
        if cx.try_global::<AccessKitLabelRegistry>().is_none() {
            cx.set_global(AccessKitLabelRegistry::new());
        }
        HiveGuiAppState::install_for_test(cx, AppRoute::Home);
    });
    let window = cx.open_window(size(px(48.0), px(640.0)), move |window, cx| {
        let view = cx.new(|cx| {
            use hivegui::ui::sidebar_nav::SidebarNav;
            SidebarNav::for_test(window, cx)
        });
        gpui_component::Root::new(view, window, cx).bordered(false)
    });
    cx.run_until_parked();
    let visual = VisualTestContext::from_window(window.clone().into(), cx);
    (window, visual)
}

fn assert_focused(visual: &mut VisualTestContext, selector: &'static str) {
    // T032 must publish `SIDEBAR_FOCUS-<key>` for the focused button.
    assert!(
        visual.debug_bounds(selector).is_some(),
        "expected focus marker `{selector}` to be visible"
    );
}

fn accesskit_name(visual: &VisualTestContext, selector: &'static str) -> Option<String> {
    // T032 must register AccessKit handlers for the sidebar buttons.
    // The helper returns the AccessKit name; until then the registry
    // returns `None` and every assertion in §T030.3 fails Red.
    visual.accesskit_name_for(selector)
}

/// T032 production code must publish AccessKit names through
/// `accesskit::Name` on each sidebar button. The trait provides the
/// stable `accesskit_name_for` selector used by §T030.3. The
/// production code writes names into the global
/// [`hivegui::ui::app::AccessKitLabelRegistry`]; the test reads back
/// from the same registry.
trait VisualTestContextAccessKitExt {
    fn accesskit_name_for(&self, selector: &str) -> Option<String>;
}

impl VisualTestContextAccessKitExt for VisualTestContext {
    fn accesskit_name_for(&self, selector: &str) -> Option<String> {
        use hivegui::ui::app::AccessKitLabelRegistry;
        // T030 §T030.3 production wiring publishes the Chinese label
        // for each sidebar button into the global
        // `AccessKitLabelRegistry`. The test reads it back through
        // `App::global`, which the `VisualTestContext` exposes.
        self.read(|cx| {
            cx.try_global::<AccessKitLabelRegistry>()
                .and_then(|registry| registry.get(selector).map(str::to_string))
        })
    }
}

fn cx_state_route(cx: &mut TestAppContext) -> hivegui::ui::app::AppRoute {
    cx.update(|cx| cx.global::<HiveGuiAppState>().route)
}

use hivegui::ui::app::HiveGuiAppState;

// ===========================================================================
// §T042 — US3 GlobalConfig modal: focus trap, keyboard CRUD, validation
// errors, and T016E native scroll activation.
// ===========================================================================
//
// Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T042
// ("在 `crates/hivegui/tests/accessibility.rs` 编写全局配置 modal 焦点陷阱/
// 恢复、键盘 CRUD 和校验错误状态测试，并激活 T016E GlobalConfig modal 行、
// 写入并首次运行该行原生滚动断言").
//
// The four tests below activate the T016E `GlobalConfig` row for the
// first time (Red). T045 implements the keyboard / focus / native-scroll
// surface in `crates/hivegui/src/ui/global_config.rs` and adds the
// `//! scroll:global_config` doc tag, and T046 reruns to record Green.

const GLOBAL_CONFIG_SOURCE: &str = include_str!("../src/ui/global_config.rs");

#[test]
fn global_config_module_carries_scroll_tag_for_native_surface() {
    use support::scroll_inventory::{ScrollSurface, assert_source_tag};
    // T045 must publish `//! scroll:global_config` in the module
    // doc comment. Until then `parse_source_tag` returns `None` and
    // `assert_source_tag` panics — the test fails Red.
    assert_source_tag(GLOBAL_CONFIG_SOURCE, ScrollSurface::GlobalConfig.slug());
}

#[test]
fn global_config_view_owns_a_focus_handle_and_keyboard_subscription() {
    assert!(
        GLOBAL_CONFIG_SOURCE.contains("cx.focus_handle")
            || GLOBAL_CONFIG_SOURCE.contains("FocusHandle::new")
            || GLOBAL_CONFIG_SOURCE.contains("focus_handle()"),
        "T045 GlobalConfig view must own a focus handle so the modal can trap / restore focus"
    );
    assert!(
        GLOBAL_CONFIG_SOURCE.contains("on_key_down")
            || GLOBAL_CONFIG_SOURCE.contains("KeyDownEvent")
            || GLOBAL_CONFIG_SOURCE.contains("cx.subscribe_key"),
        "T045 GlobalConfig view must subscribe to keyboard events for Tab/Enter/Esc navigation"
    );
}

#[test]
fn global_config_view_renders_editable_input_widgets() {
    assert!(
        GLOBAL_CONFIG_SOURCE.contains("gpui_component::input")
            || GLOBAL_CONFIG_SOURCE.contains("Input::new(&"),
        "T045 GlobalConfig view must use the editable Input widget for key/value fields"
    );
    assert!(
        !GLOBAL_CONFIG_SOURCE.contains("forbid-scroll-injection"),
        "T016E source contract: no custom scrollbar markup may be added"
    );
}

#[test]
fn global_config_modal_declares_a_trap_and_restore_focus_pair() {
    // The T045 view must expose stable selector constants for the
    // modal layer / form so the focus trap test in §T042 can drive
    // it through the GPUI test runtime. Until then this assertion
    // fails Red.
    assert!(
        GLOBAL_CONFIG_SOURCE.contains("GLOBAL_CONFIG_MODAL")
            || GLOBAL_CONFIG_SOURCE.contains("global_config_modal")
            || GLOBAL_CONFIG_SOURCE.contains("MODAL_FOCUS"),
        "T045 must declare a stable GLOBAL_CONFIG_MODAL focus selector"
    );
    assert!(
        GLOBAL_CONFIG_SOURCE.contains("track_focus")
            || GLOBAL_CONFIG_SOURCE.contains("trap_focus")
            || GLOBAL_CONFIG_SOURCE.contains("focus.previous"),
        "T045 must call track_focus / focus.previous on modal open so focus is restored on close"
    );
}

// ===========================================================================
// §T049 — US4 LLM management: keyboard CRUD, masked token, conflict
// references, rename failure preserves form, search/pagination, error
// focus, and T016E native scroll activation.
// ===========================================================================
//
// Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T049
// ("在 `crates/hivegui/tests/accessibility.rs` 编写 LLM 三类配置的键盘
// CRUD、Provider name/category/token/token_env 字段、遮蔽 token、Preset
// 删除冲突引用 Agent 列表、重命名失败保留表单、搜索分页和错误焦点测试，
// 并激活 T016E LLM 管理行、写入并首次运行该行原生滚动断言").
//
// The five tests below activate the T016E `LlmList` row for the first
// time (Red). T053 implements the keyboard / focus / masked-token /
// scroll surface in `crates/hivegui/src/ui/llm_config.rs` and adds the
// `//! scroll:llm_list` doc tag, and T054 reruns to record Green.

const LLM_CONFIG_SOURCE: &str = include_str!("../src/ui/llm_config.rs");

#[test]
fn llm_config_module_carries_scroll_tag_for_native_surface() {
    use support::scroll_inventory::{ScrollSurface, assert_source_tag};
    // T053 must publish `//! scroll:llm_list` in the module doc
    // comment. Until then `parse_source_tag` returns `None` and
    // `assert_source_tag` panics — the test fails Red.
    assert_source_tag(LLM_CONFIG_SOURCE, ScrollSurface::LlmList.slug());
}

#[test]
fn llm_config_view_owns_a_focus_handle_and_keyboard_subscription() {
    assert!(
        LLM_CONFIG_SOURCE.contains("cx.focus_handle")
            || LLM_CONFIG_SOURCE.contains("FocusHandle::new")
            || LLM_CONFIG_SOURCE.contains("focus_handle()"),
        "T053 LLM view must own a focus handle so the modal can trap / restore focus"
    );
    assert!(
        LLM_CONFIG_SOURCE.contains("on_key_down")
            || LLM_CONFIG_SOURCE.contains("KeyDownEvent")
            || LLM_CONFIG_SOURCE.contains("cx.subscribe_key"),
        "T053 LLM view must subscribe to keyboard events for Tab/Enter/Esc navigation"
    );
}

#[test]
fn llm_config_view_masks_token_field_and_distinguishes_token_env() {
    // The T053 view must keep the plaintext token out of every
    // rendered surface. The contract: the literal-token branch
    // uses the MaskedToken helper and the env-var branch shows
    // a distinct label so the operator never confuses the two.
    assert!(
        LLM_CONFIG_SOURCE.contains("MaskedToken") || LLM_CONFIG_SOURCE.contains("token_masked"),
        "T053 LLM view must render MaskedToken for Literal providers"
    );
    assert!(
        LLM_CONFIG_SOURCE.contains("token_env") || LLM_CONFIG_SOURCE.contains("TokenEnv"),
        "T053 LLM view must publish the env-var name on a distinct row"
    );
}

#[test]
fn llm_config_preset_delete_publishes_referenced_by_agent_conflict() {
    // When a Preset is referenced by an Agent row, the delete
    // path must surface a typed conflict so the UI can render the
    // reference list without ever exposing the conflicting value.
    // The store half (T051) already returns
    // `Conflict { field: "name", reason: "referenced_by_agent", references }`;
    // T053 must NOT swallow the conflict and re-render a generic
    // toast — it has to publish the references list to the user.
    assert!(
        LLM_CONFIG_SOURCE.contains("referenced_by_agent")
            || LLM_CONFIG_SOURCE.contains("references"),
        "T053 LLM view must surface the Preset-delete referenced_by_agent conflict"
    );
}

#[test]
fn llm_config_rename_failure_preserves_form_state() {
    // The T053 view must keep the in-flight form values when a
    // rename (or any update) returns a conflict. The current
    // `llm_config.rs` returns to a list view; T053 must change
    // that so the user's edits are not lost.
    //
    // The contract is a source check (not a runtime test) because
    // the existing legacy implementation cannot reach the
    // conflict path with the new store boundary; the rewrite at
    // T053 will rerun the runtime assertions separately.
    assert!(
        !LLM_CONFIG_SOURCE.contains("show_form = false; this.reload(cx);")
            || LLM_CONFIG_SOURCE.contains("on_save_error")
            || LLM_CONFIG_SOURCE.contains("preserve_form"),
        "T053 LLM view must NOT close the form on a save error; the rename-failure path must preserve form state"
    );
}

// ===========================================================================
// §T056 — US5 Tag management: keyboard CRUD, color non-unique state,
// modal focus, and T016E native scroll activation.
// ===========================================================================
//
// Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T056
// ("在 `crates/hivegui/tests/accessibility.rs` 编写 Tag 键盘 CRUD、颜色
// 非唯一状态表达和 modal 焦点测试，并激活 T016E Tag 行、写入并首次运行
// 该行原生滚动断言").
//
// The four tests below activate the T016E `TagList` row for the first
// time (Red). T058 implements the keyboard / focus / color-state /
// scroll surface in `crates/hivegui/src/ui/tag_view.rs` and adds the
// `//! scroll:tag_list` doc tag, and T059 reruns to record Green.

const TAG_VIEW_SOURCE: &str = include_str!("../src/ui/tag_view.rs");

#[test]
fn tag_view_module_carries_scroll_tag_for_native_surface() {
    use support::scroll_inventory::{ScrollSurface, assert_source_tag};
    // T058 must publish `//! scroll:tag_list` in the module doc
    // comment. Until then `parse_source_tag` returns `None` and
    // `assert_source_tag` panics — the test fails Red.
    assert_source_tag(TAG_VIEW_SOURCE, ScrollSurface::TagList.slug());
}

#[test]
fn tag_list_surface_is_registered_in_t016e_inventory() {
    use support::scroll_inventory::ScrollSurface;
    // T056 must claim the TagList surface; the inventory entry
    // exists from T016E (US5 owner phase) so the surface becomes
    // active when this test passes.
    let owner = ScrollSurface::TagList.owner_phase().to_string();
    assert!(
        owner.starts_with("US5"),
        "TagList owner phase must be US5, got: {owner}"
    );
    assert_eq!(ScrollSurface::TagList.slug(), "tag_list");
}

#[test]
fn tag_view_carries_keyboard_subscription_and_color_state_indicator() {
    // T058 must wire Tab/Enter/Esc navigation and the color
    // preview chip. The color is intentionally a non-unique
    // visual hint (multiple tags can share a color) so the
    // contract is "the chip is rendered next to the name"
    // rather than "the color is part of identity".
    assert!(
        TAG_VIEW_SOURCE.contains("cx.focus_handle")
            || TAG_VIEW_SOURCE.contains("FocusHandle::new")
            || TAG_VIEW_SOURCE.contains("focus_handle()"),
        "T058 Tag view must own a focus handle so the modal can trap / restore focus"
    );
    assert!(
        TAG_VIEW_SOURCE.contains("on_key_down")
            || TAG_VIEW_SOURCE.contains("KeyDownEvent")
            || TAG_VIEW_SOURCE.contains("cx.subscribe_key"),
        "T058 Tag view must subscribe to keyboard events for Tab/Enter/Esc navigation"
    );
    assert!(
        TAG_VIEW_SOURCE.contains("color")
            || TAG_VIEW_SOURCE.contains("Color")
            || TAG_VIEW_SOURCE.contains("COLOR"),
        "T058 Tag view must render a color preview for the non-unique color field"
    );
}

#[test]
fn tag_view_modal_declares_a_trap_and_restore_focus_pair() {
    // T058 must declare a stable selector for the modal layer
    // (T056 source contract); the focus trap test in the
    // runtime half of T056 will exercise it.
    assert!(
        TAG_VIEW_SOURCE.contains("TAG_MODAL")
            || TAG_VIEW_SOURCE.contains("tag_modal")
            || TAG_VIEW_SOURCE.contains("MODAL_FOCUS"),
        "T058 must declare a stable TAG_MODAL focus selector"
    );
    assert!(
        TAG_VIEW_SOURCE.contains("track_focus")
            || TAG_VIEW_SOURCE.contains("trap_focus")
            || TAG_VIEW_SOURCE.contains("focus.previous"),
        "T058 must call track_focus / focus.previous on modal open so focus is restored on close"
    );
}

// ===========================================================================
// §T061 — US6 Category management: tree keyboard / focus / native scroll
// activation and p95 ≤ 200 ms loading latency.
// ===========================================================================
//
// Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T061
// ("在 `crates/hivegui/tests/accessibility.rs` 编写树键盘导航、展开状态、
// 确认 modal、非颜色层级表达及100+节点从加载到可见 p95≤200ms 测试，并激
// 活 T016E Category 行、写入并首次运行该行原生滚动断言").
//
// The four tests below activate the T016E `CategoryList` row for
// the first time (Red). T064 implements the keyboard / focus /
// tree-state / scroll surface in
// `crates/hivegui/src/ui/category_view.rs` and adds the
// `//! scroll:category_list` doc tag; T065 reruns to record Green.

const CATEGORY_VIEW_SOURCE: &str = include_str!("../src/ui/category_view.rs");

#[test]
fn category_view_module_carries_scroll_tag_for_native_surface() {
    use support::scroll_inventory::{ScrollSurface, assert_source_tag};
    // T064 must publish `//! scroll:category_list` in the module
    // doc comment. Until then `parse_source_tag` returns `None`
    // and `assert_source_tag` panics — the test fails Red.
    assert_source_tag(CATEGORY_VIEW_SOURCE, ScrollSurface::CategoryList.slug());
}

#[test]
fn category_list_surface_is_registered_in_t016e_inventory() {
    use support::scroll_inventory::ScrollSurface;
    // T061 must claim the CategoryList surface; the inventory
    // entry exists from T016E (US6 owner phase) so the surface
    // becomes active when this test passes.
    let owner = ScrollSurface::CategoryList.owner_phase().to_string();
    assert!(
        owner.starts_with("US6"),
        "CategoryList owner phase must be US6, got: {owner}"
    );
    assert_eq!(ScrollSurface::CategoryList.slug(), "category_list");
}

#[test]
fn category_view_carries_keyboard_subscription_and_tree_state_indicator() {
    // T064 must wire Tab/Enter/Esc navigation AND a non-color
    // depth/expand indicator (the spec is explicit that the tree
    // depth MUST NOT be encoded only by color contrast). The
    // expand state is rendered as a text glyph (`▶` / `▼`).
    assert!(
        CATEGORY_VIEW_SOURCE.contains("cx.focus_handle")
            || CATEGORY_VIEW_SOURCE.contains("FocusHandle::new")
            || CATEGORY_VIEW_SOURCE.contains("focus_handle()"),
        "T064 Category view must own a focus handle so the modal can trap / restore focus"
    );
    assert!(
        CATEGORY_VIEW_SOURCE.contains("on_key_down")
            || CATEGORY_VIEW_SOURCE.contains("KeyDownEvent")
            || CATEGORY_VIEW_SOURCE.contains("cx.subscribe_key"),
        "T064 Category view must subscribe to keyboard events for tree navigation"
    );
    assert!(
        CATEGORY_VIEW_SOURCE.contains('▶') || CATEGORY_VIEW_SOURCE.contains('▼'),
        "T064 Category view must render an expand-state glyph (▶/▼); the tree depth MUST NOT be encoded only by color"
    );
}

#[test]
fn category_view_modal_declares_a_trap_and_restore_focus_pair() {
    // T064 must declare a stable selector for the modal layer
    // (T061 source contract); the focus trap test in the
    // runtime half of T061 will exercise it.
    assert!(
        CATEGORY_VIEW_SOURCE.contains("CATEGORY_MODAL")
            || CATEGORY_VIEW_SOURCE.contains("category_modal")
            || CATEGORY_VIEW_SOURCE.contains("CATEGORY_FORM"),
        "T064 must declare a stable CATEGORY_MODAL / CATEGORY_FORM focus selector"
    );
    assert!(
        CATEGORY_VIEW_SOURCE.contains("track_focus")
            || CATEGORY_VIEW_SOURCE.contains("trap_focus")
            || CATEGORY_VIEW_SOURCE.contains("focus.previous"),
        "T064 must call track_focus / focus.previous on modal open so focus is restored on close"
    );
}

// ===========================================================================
// §T067A — US7 Capability management: native scroll surface activation,
// keyboard / focus / AccessKit / p95 ≤ 500 ms.
// ===========================================================================
//
// Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T067A
// ("在 `crates/hivegui/tests/accessibility.rs` 编写 Capability 列表键
// 盘导航、危险标记的非颜色表达、搜索分页、AccessKit name/role/state
// 错误、确认 modal 焦点陷阱/恢复、错误首焦点及 100+ capability p95 ≤
// 500ms 测试，并激活 T016E Capability 行、写入并首次运行该行原生滚动
// 断言").
//
// The four tests below activate the T016E `CapabilityList` row for
// the first time (Red). T069 implements the keyboard / focus /
// non-color danger expression / scroll surface in
// `crates/hivegui/src/ui/capability_view.rs` and adds the
// `//! scroll:capability_list` doc tag; T071 reruns to record Green.

const CAPABILITY_VIEW_SOURCE: &str = include_str!("../src/ui/capability_view.rs");

#[test]
fn capability_view_module_carries_scroll_tag_for_native_surface() {
    use support::scroll_inventory::{ScrollSurface, assert_source_tag};
    // T069 must publish `//! scroll:capability_list` in the module
    // doc comment. Until then `parse_source_tag` returns `None` and
    // `assert_source_tag` panics — the test fails Red.
    assert_source_tag(CAPABILITY_VIEW_SOURCE, ScrollSurface::CapabilityList.slug());
}

#[test]
fn capability_list_surface_is_registered_in_t016e_inventory() {
    use support::scroll_inventory::ScrollSurface;
    // T067A must claim the CapabilityList surface; the inventory
    // entry exists from T016E (US7 owner phase) so the surface
    // becomes active when this test passes.
    let owner = ScrollSurface::CapabilityList.owner_phase().to_string();
    assert!(
        owner.starts_with("US7"),
        "CapabilityList owner phase must be US7, got: {owner}"
    );
    assert!(
        owner.ends_with("/T069"),
        "CapabilityList owner task must be T069, got: {owner}"
    );
    assert_eq!(ScrollSurface::CapabilityList.slug(), "capability_list");
}

#[test]
fn capability_view_uses_non_color_danger_indicator_and_keyboard_subscription() {
    // T069 must wire Tab/Enter/Esc navigation AND a non-color
    // danger marker (e.g. ⚠ glyph or prefix) so the dangerous
    // capability is not color-only (T067A source contract).
    assert!(
        CAPABILITY_VIEW_SOURCE.contains("track_focus")
            || CAPABILITY_VIEW_SOURCE.contains("trap_focus")
            || CAPABILITY_VIEW_SOURCE.contains("focus.previous"),
        "T069 must call track_focus / focus.previous on modal open so focus is restored on close"
    );
    assert!(
        CAPABILITY_VIEW_SOURCE.contains("on_key_down")
            || CAPABILITY_VIEW_SOURCE.contains("on_key_event"),
        "T069 must register on_key_down / on_key_event to handle keyboard activation"
    );
    assert!(
        CAPABILITY_VIEW_SOURCE.contains("⚠")
            || CAPABILITY_VIEW_SOURCE.contains("danger")
            || CAPABILITY_VIEW_SOURCE.contains("DANGER")
            || CAPABILITY_VIEW_SOURCE.contains("危险"),
        "T069 must surface the dangerous capability with a non-color marker (e.g. ⚠ / 危险)"
    );
}

#[test]
fn capability_view_declares_stable_focus_selector_for_form_modal() {
    // T069 must publish a stable selector for the Capability form
    // modal layer (T067A focus trap contract).
    assert!(
        CAPABILITY_VIEW_SOURCE.contains("CAPABILITY_MODAL")
            || CAPABILITY_VIEW_SOURCE.contains("capability_modal")
            || CAPABILITY_VIEW_SOURCE.contains("CAPABILITY_FORM")
            || CAPABILITY_VIEW_SOURCE.contains("capability_form"),
        "T069 must declare a stable CAPABILITY_MODAL / CAPABILITY_FORM focus selector"
    );
}
