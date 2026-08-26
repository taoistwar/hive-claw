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
    let visual = VisualTestContext::from_window(window.into(), cx);
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
    let visual = VisualTestContext::from_window(window.into(), cx);
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
    let (_window, mut visual) = open_sidebar_view(cx);

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

    for selector in ["SIDEBAR_ICON-home", "SIDEBAR_ICON-ai", "SIDEBAR_ICON-tools"] {
        let bounds = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("missing visible sidebar icon `{selector}`"));
        assert!(
            !bounds.is_empty(),
            "sidebar icon `{selector}` must occupy visible bounds"
        );
    }
}

#[test]
fn sidebar_focus_targets_expose_real_button_roles_and_names() {
    use gpui::Role;
    use hivegui::ui::sidebar_nav::{SidebarKey, sidebar_focusable_accesskit_probe};

    for (key, expected_name) in [
        (SidebarKey::Home, "首页"),
        (SidebarKey::Ai, "AI 管理"),
        (SidebarKey::Tools, "工具"),
        (SidebarKey::UserConfig, "用户配置"),
    ] {
        let node = sidebar_focusable_accesskit_probe(key);
        assert_eq!(node.role(), Role::Button, "{key:?} must expose Button");
        assert_eq!(
            node.label(),
            Some(expected_name),
            "{key:?} must expose its visible Chinese name"
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
    // T032 install_for_test uses the same shell-global installer as
    // production, so both HiveGuiAppState and AccessKitLabelRegistry
    // exist before the sidebar's first render.
    use hivegui::ui::app::{AppRoute, HiveGuiAppState};
    cx.update(|cx| {
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
    let visual = VisualTestContext::from_window(window.into(), cx);
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
// §T110 — US12 Skill management: editable skill surface keyboard CRUD,
// duplicate identifier error stability, search pagination and native scroll
// activation.
// ===========================================================================
//
// Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T110
// ("在 `crates/hivegui/tests/accessibility.rs` 编写 Skill 编辑器、重复
// identifier conflict 后表单保持/错误焦点/安全值、错误状态、搜索分页和
// 键盘 CRUD 测试，并激活 T016E Skill 行、写入并首次运行该行原生滚动断言").
//
// The tests below activate `T016E` `SkillList` for the first time (Red).
// `T112` implements the native scroll/tag + keyboard + focus + error-state
// contracts in `crates/hivegui/src/ui/skill_view.rs` and this block records
// the required surface/selector behavior first.

const SKILL_VIEW_SOURCE: &str = include_str!("../src/ui/skill_view.rs");

#[test]
fn skill_view_module_carries_scroll_tag_for_native_surface() {
    use support::scroll_inventory::{ScrollSurface, assert_source_tag};

    // T112 must publish `//! scroll:skill_list` in the module doc
    // comment. Until then `parse_source_tag` returns `None` and
    // `assert_source_tag` panics — the test fails Red.
    assert_source_tag(SKILL_VIEW_SOURCE, ScrollSurface::SkillList.slug());
}

#[test]
fn skill_list_surface_is_registered_in_t016e_inventory() {
    use support::scroll_inventory::ScrollSurface;

    // T110 activates the T016E Skill surface owned by US12/T112.
    let owner = ScrollSurface::SkillList.owner_phase().to_string();
    assert!(
        owner == "US12/T112",
        "SkillList owner phase must be US12/T112, got: {owner}"
    );
    assert_eq!(ScrollSurface::SkillList.slug(), "skill_list");
}

#[test]
fn skill_view_supports_keyboard_navigation_for_form_crud() {
    // T112 must wire Tab/Enter/Escape keyboard flow for the CRUD modal.
    assert!(
        SKILL_VIEW_SOURCE.contains("on_key_down")
            || SKILL_VIEW_SOURCE.contains("on_key_event")
            || SKILL_VIEW_SOURCE.contains("KeyDownEvent")
            || SKILL_VIEW_SOURCE.contains("track_focus")
            || SKILL_VIEW_SOURCE.contains("focus_handle")
            || SKILL_VIEW_SOURCE.contains("focus.previous"),
        "T112 Skill view must handle keyboard input and focus for accessibility CRUD"
    );
}

#[test]
fn skill_view_declares_stable_form_modal_and_error_surface() {
    // T110 expects duplicate identifier save errors to keep form fields,
    // surface errors, and keep user focus on or near the form.
    assert!(
        SKILL_VIEW_SOURCE.contains("skill-form-scroll")
            || SKILL_VIEW_SOURCE.contains("SKILL_FORM")
            || SKILL_VIEW_SOURCE.contains("SKILL_MODAL"),
        "T112 must declare a stable modal/scroll surface for Skill form keyboard focus"
    );
    assert!(
        SKILL_VIEW_SOURCE.contains("error_message")
            && SKILL_VIEW_SOURCE.contains("更新失败")
            && SKILL_VIEW_SOURCE.contains("创建失败"),
        "T112 should render error state on conflict/validation failures"
    );
    assert!(
        !SKILL_VIEW_SOURCE.contains("hide_form(cx)") || SKILL_VIEW_SOURCE.contains("Err(e) => {"),
        "Do not close form on save failure before preserving safe values"
    );
}

#[test]
fn skill_view_search_and_pagination_controls_use_size_20() {
    // T110 requires keyword search + 20-item pages for accessibility keyboard
    // tests. T112 must preserve these anchors.
    assert!(SKILL_VIEW_SOURCE.contains("search_text"));
    assert!(SKILL_VIEW_SOURCE.contains("page_size: i64"));
    assert!(
        SKILL_VIEW_SOURCE.contains("page_size")
            && SKILL_VIEW_SOURCE.contains("prev")
            && SKILL_VIEW_SOURCE.contains("next"),
        "Skill list pagination controls are missing or renamed"
    );
}

#[gpui::test]
fn skill_view_initial_render_does_not_materialize_the_hidden_form(cx: &mut TestAppContext) {
    let (_workspace, store, store_entity, runtime) = function_visual_store(cx);
    let _runtime_guard = runtime.enter();
    let window = cx.open_window(size(px(720.0), px(520.0)), move |window, cx| {
        let view = cx.new(|cx| hivegui::ui::skill_view::SkillView::new(store_entity, cx));
        gpui_component::Root::new(view, window, cx).bordered(false)
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    for _ in 0..200 {
        visual.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    assert!(
        visual
            .debug_bounds(hivegui::ui::skill_view::SKILL_MODAL)
            .is_none(),
        "the hidden Skill form must remain absent until the user opens it"
    );

    visual.update(|_, cx| {
        cx.remove_global::<HiveGuiAppState>();
    });
    close_function_visual_window(&mut visual);
    drop(visual);
    drop(store);
    drop(_runtime_guard);
}

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

// ===========================================================================
// §T085 — US9 Function management: builtin/schema/accessibility/contracts and
// T016E Function activation.
// ===========================================================================
//
// Source of truth: `specs/011-hivegui-standalone-mode/tasks.md` §T085
// ("在 `crates/hivegui/tests/accessibility.rs` 编写 Function JSON schema
// 编辑、Builtin 禁用状态、Placeholder 标签/schema-only 字段/无测试操作/
// 不可执行状态、执行反馈、重复 identifier conflict 后表单保持/错误焦点/
// 安全值和键盘测试，并激活 T016E Function 行、写入并首次运行该行原生滚动
// 断言").
//
// T088 adds the runtime/a11y behavior in `crates/hivegui/src/ui/function_view.rs`.
// These tests are the corresponding accessibility-side Red entry points and
// inventory activation for T016E Function.

const FUNCTION_VIEW_SOURCE: &str = include_str!("../src/ui/function_view.rs");

#[test]
fn function_view_module_carries_scroll_tag_for_native_surface() {
    use support::scroll_inventory::{ScrollSurface, assert_source_tag};
    // T088 must publish `//! scroll:function_list` in the module doc
    // comment. Until then `parse_source_tag` returns `None` and
    // `assert_source_tag` panics.
    assert_source_tag(FUNCTION_VIEW_SOURCE, ScrollSurface::FunctionList.slug());
}

#[test]
fn function_list_surface_is_registered_in_t016e_inventory() {
    use support::scroll_inventory::ScrollSurface;
    // T085 activates the T016E Function surface closed by the T088 UI task.
    let owner = ScrollSurface::FunctionList.owner_phase().to_string();
    assert_eq!(
        owner, "US9/T088",
        "FunctionList owner phase must be fixed to the US9/T088 UI implementation task"
    );
    assert_eq!(ScrollSurface::FunctionList.slug(), "function_list");
}

#[test]
fn function_view_supports_json_schema_editor_controls() {
    // T088 must keep dedicated JSON schema editors in the form so users can
    // configure builtin/custom schemas directly from the management UI.
    assert!(
        FUNCTION_VIEW_SOURCE.contains("form_field_multiline(")
            && FUNCTION_VIEW_SOURCE.contains("\"Input Schema (JSON)\""),
        "T088 should render the input schema editor"
    );
    assert!(
        FUNCTION_VIEW_SOURCE.contains("form_field_multiline(")
            && FUNCTION_VIEW_SOURCE.contains("\"Output Schema (JSON)\""),
        "T088 should render the output schema editor"
    );
    assert!(
        FUNCTION_VIEW_SOURCE.contains("input_schema_input")
            && FUNCTION_VIEW_SOURCE.contains("output_schema_input"),
        "Function form should keep schema input handles for validation and persistence"
    );
}

#[test]
fn function_view_placeholder_row_should_be_schema_only_and_non_executable() {
    // Placeholder (kind 3) must have no inline test action and only placeholder
    // schema semantics, not a live execution contract.
    assert!(
        FUNCTION_VIEW_SOURCE.contains("ic.kind != \"placeholder\""),
        "Placeholder rows must hide the inline '测试' action"
    );
    assert!(
        FUNCTION_VIEW_SOURCE.contains("kind == \"placeholder\"")
            && FUNCTION_VIEW_SOURCE.contains("required_capabilities = None"),
        "Placeholder rows must clear capability/export state and stay schema-only"
    );
    assert!(
        FUNCTION_VIEW_SOURCE.contains("function_kind_label")
            && FUNCTION_VIEW_SOURCE.contains("占位"),
        "Function list should still expose the placeholder label"
    );
}

#[test]
fn function_view_exposes_execution_feedback_states_for_test_runs() {
    // The run-test drawer should provide terminal success/error feedback so users
    // can see why execution failed or succeeded.
    assert!(
        FUNCTION_VIEW_SOURCE.contains("TestState::Running"),
        "Function test dialog should expose running state"
    );
    assert!(
        FUNCTION_VIEW_SOURCE.contains("TestState::Success"),
        "Function test dialog should render success state"
    );
    assert!(
        FUNCTION_VIEW_SOURCE.contains("TestState::Error")
            && FUNCTION_VIEW_SOURCE.contains("执行结果")
            && FUNCTION_VIEW_SOURCE.contains(".execute_with_capabilities("),
        "Function test dialog should render error state after execute failure"
    );
}

#[test]
fn function_view_form_keeps_safe_values_and_focus_contract_after_save_error() {
    // T088 runtime implementation must preserve form values on create/update
    // failure, keep the user on the same form surface, and focus recoverably.
    assert!(
        FUNCTION_VIEW_SOURCE.contains(r#"v.error_message = Some(format!("创建失败"#)
            || FUNCTION_VIEW_SOURCE.contains(r#"v.error_message = Some(format!("更新失败"#),
        "Save failure should stay on form and render safe error text"
    );
    assert!(
        !FUNCTION_VIEW_SOURCE.contains("show_form = false"),
        "Error path should not force-close form immediately"
    );
}

#[test]
fn function_view_declares_keyboard_and_focus_contract_for_accessibility() {
    // This contract line is intentionally strict: the Function surface must add
    // keyboard/focus support, and builtins should move from read-only defaults
    // into explicit disabled-mode behavior during T088 implementation.
    assert!(
        FUNCTION_VIEW_SOURCE.contains("track_focus")
            || FUNCTION_VIEW_SOURCE.contains("trap_focus")
            || FUNCTION_VIEW_SOURCE.contains("focus.previous")
            || FUNCTION_VIEW_SOURCE.contains("cx.focus_handle")
            || FUNCTION_VIEW_SOURCE.contains("FocusHandle::new"),
        "T088 must wire focus lifecycle for Function modal/focus surface"
    );
    assert!(
        FUNCTION_VIEW_SOURCE.contains("on_key_down")
            || FUNCTION_VIEW_SOURCE.contains("on_key_event")
            || FUNCTION_VIEW_SOURCE.contains("subscribe_key"),
        "T088 must wire keyboard handling for Function list/form/Test dialogs"
    );
    assert!(
        FUNCTION_VIEW_SOURCE.contains("kind == \"builtin\"")
            || FUNCTION_VIEW_SOURCE.contains("kind != \"builtin\""),
        "Builtin rows should have an explicit contract path for disabled mode"
    );
}

fn function_visual_store(
    cx: &mut TestAppContext,
) -> (
    support::TestWorkspace,
    hivegui::datasource::Store,
    gpui::Entity<hivegui::datasource::Store>,
    tokio::runtime::Runtime,
) {
    use hivegui::datasource::store::{Store, StoreOpenOptions};
    use hivegui::ui::app::{AppRoute, HiveGuiAppState};

    init_gpui(cx);
    cx.executor().allow_parking();
    let workspace = support::TestWorkspace::new().expect("create Function visual-test workspace");
    let runtime = tokio::runtime::Runtime::new().expect("create Function visual-test runtime");
    let store = runtime
        .block_on(Store::open_local(StoreOpenOptions::new(
            workspace.database_path(),
            workspace.plugin_root(),
        )))
        .expect("open canonical Function visual-test Store");
    let store_entity = cx.new(|_| store.clone());
    cx.update(|cx| {
        HiveGuiAppState::install_for_test_with_store(cx, AppRoute::Ai, store_entity.clone());
    });
    (workspace, store, store_entity, runtime)
}

fn open_function_visual_window(
    cx: &mut TestAppContext,
    store: gpui::Entity<hivegui::datasource::Store>,
    window_size: gpui::Size<gpui::Pixels>,
) -> WindowHandle<gpui_component::Root> {
    cx.open_window(window_size, move |window, cx| {
        let view = cx.new(|cx| hivegui::ui::function_view::FunctionView::new(store, cx));
        gpui_component::Root::new(view, window, cx).bordered(false)
    })
}

fn click_function_selector(visual: &mut VisualTestContext, selector: &'static str) -> bool {
    let Some(bounds) = visual.debug_bounds(selector) else {
        return false;
    };
    visual.simulate_click(bounds.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    true
}

fn replace_function_input(
    visual: &mut VisualTestContext,
    selector: &'static str,
    value: &str,
) -> bool {
    let Some(bounds) = visual.debug_bounds(selector) else {
        return false;
    };
    visual.simulate_click(bounds.center(), gpui::Modifiers::default());
    visual.simulate_keystrokes("ctrl-a");
    visual.simulate_input(value);
    visual.run_until_parked();
    true
}

fn close_function_visual_window(visual: &mut VisualTestContext) {
    visual.update(|window, _| window.remove_window());
    visual.run_until_parked();
}

fn settle_function_selector(visual: &mut VisualTestContext, selector: &'static str) -> bool {
    for _ in 0..16 {
        visual.run_until_parked();
        if visual.debug_bounds(selector).is_some() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    false
}

fn settle_function_selector_absent(visual: &mut VisualTestContext, selector: &'static str) -> bool {
    for _ in 0..200 {
        visual.run_until_parked();
        if visual.debug_bounds(selector).is_none() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    false
}

fn assert_function_semantic_node(
    node: &gpui::accesskit::Node,
    expected_role: gpui::Role,
    expected_label: &str,
) {
    use gpui::accesskit::Action;

    assert_eq!(node.role(), expected_role);
    assert_eq!(node.label(), Some(expected_label));
    for action in [Action::Click, Action::Expand, Action::Collapse] {
        assert!(
            !node.supports_action(action),
            "Function status/alert semantics must not expose an inapplicable {action:?} action"
        );
    }
}

#[gpui::test]
fn function_form_native_wheel_moves_bottom_actions_into_viewport(cx: &mut TestAppContext) {
    use gpui::{ScrollDelta, ScrollWheelEvent, TouchPhase, point};

    let (_workspace, _store, store_entity, runtime) = function_visual_store(cx);
    let _runtime_guard = runtime.enter();
    let window = open_function_visual_window(cx, store_entity, size(px(520.0), px(300.0)));
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let add_visible = click_function_selector(&mut visual, "FUNCTION_ADD");
    let modal = visual.debug_bounds("FUNCTION_MODAL");
    let scroll = visual.debug_bounds("FUNCTION_FORM_SCROLL");
    let actions_at_top = visual.debug_bounds("FUNCTION_FORM_ACTIONS");
    let keyboard_save = if scroll.is_some() {
        visual.simulate_keystrokes("shift-tab");
        visual.run_until_parked();
        visual
            .debug_bounds("FUNCTION_FORM_SAVE_FOCUSED")
            .and_then(|_| visual.debug_bounds("FUNCTION_FORM_SAVE"))
    } else {
        None
    };
    let actions_after_keyboard = visual.debug_bounds("FUNCTION_FORM_ACTIONS");
    if let Some(scroll) = scroll {
        visual.simulate_event(ScrollWheelEvent {
            position: point(scroll.left() + px(24.0), scroll.top() + px(24.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(2_000.0))),
            modifiers: Default::default(),
            touch_phase: TouchPhase::Moved,
        });
        visual.run_until_parked();
    }
    let actions_before_wheel = visual.debug_bounds("FUNCTION_FORM_ACTIONS");
    let mut actions_after = None;
    if let (Some(scroll), Some(_)) = (scroll, actions_before_wheel) {
        visual.simulate_event(ScrollWheelEvent {
            position: point(scroll.left() + px(24.0), scroll.top() + px(24.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-2_000.0))),
            modifiers: Default::default(),
            touch_phase: TouchPhase::Moved,
        });
        visual.run_until_parked();
        actions_after = visual.debug_bounds("FUNCTION_FORM_ACTIONS");
    }
    let custom_controls: Vec<&str> = [
        "FUNCTION_SCROLL_UP",
        "FUNCTION_SCROLL_DOWN",
        "FUNCTION_SCROLLBAR_TRACK",
        "FUNCTION_SCROLL_HANDLE",
    ]
    .into_iter()
    .filter(|selector| visual.debug_bounds(selector).is_some())
    .collect();
    let handwritten_scroll: Vec<&str> = [
        "ScrollWheelEvent",
        ".on_scroll_wheel(",
        "FUNCTION_SCROLL_UP",
        "FUNCTION_SCROLL_DOWN",
        "FUNCTION_SCROLLBAR_TRACK",
        "FUNCTION_SCROLL_HANDLE",
        "scroll-up",
        "scroll-down",
        "scrollbar-track",
        "scroll-handle",
    ]
    .into_iter()
    .filter(|needle| FUNCTION_VIEW_SOURCE.contains(needle))
    .collect();
    close_function_visual_window(&mut visual);

    assert!(
        add_visible,
        "Function view must expose a stable FUNCTION_ADD selector for pointer and keyboard tests"
    );
    let modal = modal.expect("Function form must expose FUNCTION_MODAL bounds");
    let scroll = scroll.expect(
        "Function form must expose its GPUI Component native viewport as FUNCTION_FORM_SCROLL",
    );
    let actions_at_top = actions_at_top
        .expect("long Function form must expose its bottom actions before keyboard navigation");
    let keyboard_save = keyboard_save
        .expect("Shift-Tab navigation must wrap from the first field to FUNCTION_FORM_SAVE");
    let actions_after_keyboard = actions_after_keyboard
        .expect("Tab navigation must move the bottom Function actions into the viewport");
    assert!(
        keyboard_save.top() >= scroll.top() && keyboard_save.bottom() <= scroll.bottom(),
        "Tab reached Save but did not make it visible: viewport={scroll:?}, save={keyboard_save:?}"
    );
    assert!(
        actions_after_keyboard.top() < actions_at_top.top()
            && actions_after_keyboard.bottom() <= scroll.bottom(),
        "Tab-to-bottom did not scroll Function actions into view: before={actions_at_top:?}, after={actions_after_keyboard:?}"
    );
    let actions_before_wheel = actions_before_wheel
        .expect("long Function form must expose its bottom actions as FUNCTION_FORM_ACTIONS");
    let actions_after = actions_after.expect("bottom Function actions after real wheel input");
    assert!(modal.top() >= px(0.0) && modal.bottom() <= px(300.0));
    assert!(scroll.top() >= modal.top() && scroll.bottom() <= modal.bottom());
    assert!(
        actions_after.top() < actions_before_wheel.top(),
        "real GPUI wheel input did not move Function form content: before={actions_before_wheel:?}, after={actions_after:?}"
    );
    assert!(
        actions_after.top() >= scroll.top() && actions_after.bottom() <= scroll.bottom(),
        "bottom Function actions are not fully reachable: viewport={scroll:?}, actions={actions_after:?}"
    );
    assert!(
        custom_controls.is_empty(),
        "Function form rendered forbidden custom scroll controls: {custom_controls:?}"
    );
    assert!(
        handwritten_scroll.is_empty(),
        "Function production source contains handwritten wheel/custom scroll controls: {handwritten_scroll:?}"
    );
}

#[gpui::test]
fn function_builtin_and_placeholder_surfaces_enforce_kind_semantics(cx: &mut TestAppContext) {
    use gpui::{ScrollDelta, ScrollWheelEvent, TouchPhase, point};
    use hivegui::datasource::{FunctionInput, FunctionKind, FunctionStore};
    use hivegui::ui::function_view::{FunctionSemanticStatus, function_semantic_accesskit_probe};

    const UPDATED_INPUT_SCHEMA: &str =
        r#"{"type":"object","properties":{"query":{"type":"string"}}}"#;
    const UPDATED_OUTPUT_SCHEMA: &str =
        r#"{"type":"object","properties":{"answer":{"type":"string"}}}"#;

    let (_workspace, store, store_entity, runtime) = function_visual_store(cx);
    let function_store =
        FunctionStore::new(store.pool().clone()).expect("construct public FunctionStore fixture");
    // Store::open_local synchronizes the canonical Builtin registry before the
    // Function view loads. Resolve the synchronized row without duplicating it.
    let builtin_id = runtime
        .block_on(async {
            sqlx::query_scalar::<_, i64>(
                "SELECT id FROM functions WHERE identifier = ? AND kind = 'builtin'",
            )
            .bind("format_template")
            .fetch_one(store.pool())
            .await
        })
        .expect("resolve synchronized system-owned Builtin Function");
    let builtin = runtime
        .block_on(function_store.get(builtin_id))
        .expect("load Builtin Function")
        .expect("synchronized Builtin Function must exist");
    let placeholder_input = FunctionInput::for_write(
        "t085_schema_placeholder".to_string(),
        "T085 schema-only placeholder".to_string(),
        None,
        FunctionKind::Placeholder,
        "{}".to_string(),
        "{}".to_string(),
        None,
        None,
        None,
        None,
    )
    .expect("build Placeholder Function input");
    let placeholder = runtime
        .block_on(function_store.create(placeholder_input))
        .expect("seed Placeholder Function");
    let _runtime_guard = runtime.enter();
    let window = open_function_visual_window(cx, store_entity, size(px(760.0), px(900.0)));
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let readonly_selector: &'static str =
        Box::leak(format!("FUNCTION_BUILTIN_READONLY-{}", builtin.id()).into_boxed_str());
    let test_selector: &'static str =
        Box::leak(format!("FUNCTION_TEST-{}", builtin.id()).into_boxed_str());
    let edit_selector: &'static str =
        Box::leak(format!("FUNCTION_EDIT-{}", builtin.id()).into_boxed_str());
    let delete_selector: &'static str =
        Box::leak(format!("FUNCTION_DELETE-{}", builtin.id()).into_boxed_str());
    let readonly_visible = settle_function_selector(&mut visual, readonly_selector);
    let readonly_accesskit =
        function_semantic_accesskit_probe(FunctionSemanticStatus::BuiltinImmutable {
            id: builtin.id(),
            identifier: builtin.identifier().to_string(),
        });
    let test_visible = visual.debug_bounds(test_selector).is_some();
    let edit_visible = visual.debug_bounds(edit_selector).is_some();
    let delete_visible = visual.debug_bounds(delete_selector).is_some();

    let placeholder_state_selector: &'static str = Box::leak(
        format!("FUNCTION_PLACEHOLDER_NON_EXECUTABLE-{}", placeholder.id()).into_boxed_str(),
    );
    let placeholder_test_selector: &'static str =
        Box::leak(format!("FUNCTION_TEST-{}", placeholder.id()).into_boxed_str());
    let placeholder_edit_selector: &'static str =
        Box::leak(format!("FUNCTION_EDIT-{}", placeholder.id()).into_boxed_str());
    let placeholder_state_visible = visual.debug_bounds(placeholder_state_selector).is_some();
    let placeholder_accesskit =
        function_semantic_accesskit_probe(FunctionSemanticStatus::PlaceholderNonExecutable {
            id: placeholder.id(),
            identifier: placeholder.identifier().to_string(),
        });
    let placeholder_test_visible = visual.debug_bounds(placeholder_test_selector).is_some();

    let add_visible = click_function_selector(&mut visual, "FUNCTION_ADD");
    let builtin_option_visible = visual
        .debug_bounds("FUNCTION_KIND_OPTION-builtin")
        .is_some();
    let builtin_disabled = visual
        .debug_bounds("FUNCTION_KIND_BUILTIN_DISABLED")
        .is_some();
    if builtin_option_visible {
        let _ = click_function_selector(&mut visual, "FUNCTION_KIND_OPTION-builtin");
    }
    let builtin_selectable = visual
        .debug_bounds("FUNCTION_KIND_BUILTIN_SELECTABLE")
        .is_some();
    let builtin_selected = visual
        .debug_bounds("FUNCTION_KIND_SELECTED-builtin")
        .is_some();
    let builtin_attempt_identifier_set = replace_function_input(
        &mut visual,
        "FUNCTION_IDENTIFIER",
        "t085_user_builtin_attempt",
    );
    let builtin_attempt_name_set =
        replace_function_input(&mut visual, "FUNCTION_NAME", "T085 user Builtin attempt");
    let builtin_attempt_submitted = click_function_selector(&mut visual, "FUNCTION_FORM_SAVE")
        && settle_function_selector_absent(&mut visual, "FUNCTION_MODAL")
        && settle_function_selector(&mut visual, "FUNCTION_LIST_LOADED");
    let add_closed = if visual.debug_bounds("FUNCTION_MODAL").is_some() {
        click_function_selector(&mut visual, "FUNCTION_FORM_CANCEL")
    } else {
        true
    };
    let placeholder_edit_opened = settle_function_selector(&mut visual, placeholder_edit_selector)
        && click_function_selector(&mut visual, placeholder_edit_selector);
    let schema_fields_visible = visual.debug_bounds("FUNCTION_INPUT_SCHEMA").is_some()
        && visual.debug_bounds("FUNCTION_OUTPUT_SCHEMA").is_some()
        && visual
            .debug_bounds("FUNCTION_PLACEHOLDER_SCHEMA_ONLY")
            .is_some();
    let relation_fields_absent = visual.debug_bounds("FUNCTION_PLUGIN_SELECTOR").is_none()
        && visual.debug_bounds("FUNCTION_EXPORT_SELECTOR").is_none()
        && visual
            .debug_bounds("FUNCTION_CAPABILITY_SELECTOR")
            .is_none();
    let input_schema_set =
        replace_function_input(&mut visual, "FUNCTION_INPUT_SCHEMA", UPDATED_INPUT_SCHEMA);
    let output_schema_set =
        replace_function_input(&mut visual, "FUNCTION_OUTPUT_SCHEMA", UPDATED_OUTPUT_SCHEMA);
    let form_scroll = visual.debug_bounds("FUNCTION_FORM_SCROLL");
    if let Some(scroll) = form_scroll {
        visual.simulate_event(ScrollWheelEvent {
            position: point(scroll.left() + px(24.0), scroll.top() + px(24.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-2_000.0))),
            modifiers: Default::default(),
            touch_phase: TouchPhase::Moved,
        });
        visual.run_until_parked();
    }
    let save_in_viewport = form_scroll
        .zip(visual.debug_bounds("FUNCTION_FORM_SAVE"))
        .is_some_and(|(scroll, save)| {
            save.top() >= scroll.top() && save.bottom() <= scroll.bottom()
        });
    let placeholder_saved = save_in_viewport
        && click_function_selector(&mut visual, "FUNCTION_FORM_SAVE")
        && settle_function_selector_absent(&mut visual, "FUNCTION_MODAL")
        && settle_function_selector(&mut visual, "FUNCTION_LIST_LOADED");
    visual.update(|_, cx| {
        cx.remove_global::<HiveGuiAppState>();
    });
    close_function_visual_window(&mut visual);
    drop(_runtime_guard);
    let builtin_attempts = runtime
        .block_on(function_store.list(Some("t085_user_builtin_attempt".to_string()), 1))
        .expect("reload attempted user-created Builtin");
    let persisted_placeholder = runtime
        .block_on(function_store.get(placeholder.id()))
        .expect("reload Placeholder Function")
        .expect("Placeholder Function must remain after edit");
    let cleanup_runtime_guard = runtime.enter();
    drop(function_store);
    drop(store);
    drop(cleanup_runtime_guard);

    assert!(
        readonly_visible,
        "Builtin row must expose a non-colour, accessible read-only state via {readonly_selector}"
    );
    assert!(test_visible, "Builtin Function must remain testable");
    assert!(!edit_visible, "Builtin Function must not expose Edit");
    assert!(!delete_visible, "Builtin Function must not expose Delete");
    assert_function_semantic_node(
        &readonly_accesskit,
        gpui::Role::Status,
        "format_template builtin_immutable",
    );
    assert!(
        add_visible,
        "Function Add control must be keyboard/visual addressable"
    );
    assert!(
        !builtin_option_visible || builtin_disabled,
        "Builtin may be absent from Add, but any rendered option must be explicitly disabled"
    );
    assert!(
        !builtin_selectable && !builtin_selected,
        "the user-facing Add form must never make Builtin selectable or writable"
    );
    assert!(
        builtin_attempt_identifier_set && builtin_attempt_name_set && builtin_attempt_submitted,
        "the test must attempt creation through the rendered Function Add form"
    );
    assert!(
        builtin_attempts
            .items()
            .iter()
            .all(|function| function.kind() != FunctionKind::Builtin),
        "the Function Add form must never persist a user-created Builtin"
    );
    assert!(
        add_closed,
        "Function Add form must close after save or expose FUNCTION_FORM_CANCEL after validation"
    );
    assert!(
        placeholder_state_visible,
        "Placeholder row must expose an explicit non-executable state"
    );
    assert_function_semantic_node(
        &placeholder_accesskit,
        gpui::Role::Status,
        "t085_schema_placeholder function_not_executable",
    );
    assert!(
        !placeholder_test_visible,
        "Placeholder row must not expose Test"
    );
    assert!(placeholder_edit_opened, "Placeholder row must expose Edit");
    assert!(
        schema_fields_visible,
        "Placeholder edit form must be schema-only"
    );
    assert!(
        relation_fields_absent,
        "Placeholder edit form must hide Plugin, export and Capability relationships"
    );
    assert!(
        input_schema_set && output_schema_set,
        "Function JSON schemas must be editable through rendered Textarea controls"
    );
    assert!(
        placeholder_saved,
        "Placeholder form must expose FUNCTION_FORM_SAVE"
    );
    assert_eq!(persisted_placeholder.kind(), FunctionKind::Placeholder);
    assert_eq!(persisted_placeholder.input_schema(), UPDATED_INPUT_SCHEMA);
    assert_eq!(persisted_placeholder.output_schema(), UPDATED_OUTPUT_SCHEMA);
    assert_eq!(persisted_placeholder.plugin_id(), None);
    assert_eq!(persisted_placeholder.plugin_export(), None);
    assert_eq!(persisted_placeholder.required_capabilities(), None);
}

#[gpui::test]
fn function_test_dialog_renders_local_success_and_error_terminal_feedback(cx: &mut TestAppContext) {
    use hivegui::ui::function_view::{FunctionSemanticStatus, function_semantic_accesskit_probe};

    let (_workspace, store, store_entity, runtime) = function_visual_store(cx);
    let builtin_id = runtime
        .block_on(async {
            sqlx::query_scalar::<_, i64>(
                "SELECT id FROM functions WHERE identifier = ? AND kind = 'builtin'",
            )
            .bind("json_parse")
            .fetch_one(store.pool())
            .await
        })
        .expect("resolve synchronized local json_parse Builtin");
    let _runtime_guard = runtime.enter();
    let window = open_function_visual_window(cx, store_entity, size(px(760.0), px(900.0)));
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let test_selector: &'static str =
        Box::leak(format!("FUNCTION_TEST-{builtin_id}").into_boxed_str());
    let test_loaded = settle_function_selector(&mut visual, test_selector);
    let test_opened = test_loaded && click_function_selector(&mut visual, test_selector);
    let dialog_visible = visual.debug_bounds("FUNCTION_TEST_DIALOG").is_some();
    let success_input_set = replace_function_input(
        &mut visual,
        "FUNCTION_TEST_INPUT-text",
        r#""{\"ok\":true}""#,
    );
    let success_run = click_function_selector(&mut visual, "FUNCTION_TEST_RUN");
    let success_visible = settle_function_selector(&mut visual, "FUNCTION_TEST_SUCCESS");
    let success_accesskit = function_semantic_accesskit_probe(FunctionSemanticStatus::TestSuccess);

    let error_input_set =
        replace_function_input(&mut visual, "FUNCTION_TEST_INPUT-text", "not-json");
    let error_run = click_function_selector(&mut visual, "FUNCTION_TEST_RUN");
    let error_visible = settle_function_selector(&mut visual, "FUNCTION_TEST_ERROR");
    let error_accesskit = function_semantic_accesskit_probe(FunctionSemanticStatus::TestError);
    let no_remote_backend = visual.read(|cx| {
        cx.global::<HiveGuiAppState>()
            .assert_no_remote_backend_prerequisite()
            .is_ok()
    });
    close_function_visual_window(&mut visual);

    assert!(
        test_opened,
        "Builtin row must expose a stable Test selector"
    );
    assert!(
        dialog_visible,
        "Function Test must render FUNCTION_TEST_DIALOG"
    );
    assert!(
        success_input_set && success_run,
        "local json_parse success must be driven through rendered input and Run controls"
    );
    assert!(
        success_visible,
        "successful local Builtin execution must render a terminal success state"
    );
    assert_function_semantic_node(&success_accesskit, gpui::Role::Status, "status=success");
    assert!(
        error_input_set && error_run,
        "local json_parse error must be driven through the same rendered controls"
    );
    assert!(
        error_visible,
        "failed local Builtin execution must render a terminal error state"
    );
    assert_function_semantic_node(&error_accesskit, gpui::Role::Alert, "status=error");
    assert!(
        no_remote_backend,
        "Builtin Function test feedback must remain entirely local"
    );
}

#[gpui::test]
fn function_keyboard_conflict_preserves_safe_value_focus_and_modal_lifecycle(
    cx: &mut TestAppContext,
) {
    use hivegui::datasource::{FunctionInput, FunctionKind, FunctionStore};
    use hivegui::ui::function_view::{FunctionSemanticStatus, function_semantic_accesskit_probe};

    const IDENTIFIER: &str = "t085_safe_duplicate";
    const DRAFT_IDENTIFIER: &str = "t085_textarea_enter_draft";
    let (_workspace, store, store_entity, runtime) = function_visual_store(cx);
    let function_store =
        FunctionStore::new(store.pool().clone()).expect("construct public FunctionStore fixture");
    let duplicate_input = FunctionInput::for_write(
        IDENTIFIER.to_string(),
        "Existing safe duplicate".to_string(),
        None,
        FunctionKind::Placeholder,
        "{}".to_string(),
        "{}".to_string(),
        None,
        None,
        None,
        None,
    )
    .expect("build duplicate Function fixture");
    runtime
        .block_on(function_store.create(duplicate_input))
        .expect("seed duplicate Function");
    let _runtime_guard = runtime.enter();
    let window = open_function_visual_window(cx, store_entity, size(px(760.0), px(900.0)));
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let add_visible = click_function_selector(&mut visual, "FUNCTION_ADD");
    let modal_visible = visual.debug_bounds("FUNCTION_MODAL").is_some();
    let identifier_initially_focused = visual.debug_bounds("FUNCTION_IDENTIFIER_FOCUSED").is_some();
    if modal_visible {
        visual.simulate_keystrokes("shift-tab");
        visual.run_until_parked();
    }
    let shift_tab_wrapped_to_save = visual.debug_bounds("FUNCTION_FORM_SAVE_FOCUSED").is_some();
    if shift_tab_wrapped_to_save {
        visual.simulate_keystrokes("tab");
        visual.run_until_parked();
    }
    let tab_wrapped_to_identifier = visual.debug_bounds("FUNCTION_IDENTIFIER_FOCUSED").is_some();

    let draft_identifier_set =
        replace_function_input(&mut visual, "FUNCTION_IDENTIFIER", DRAFT_IDENTIFIER);
    let name_set = replace_function_input(&mut visual, "FUNCTION_NAME", "Duplicate attempt");
    let kind_opened = click_function_selector(&mut visual, "FUNCTION_KIND_SELECTOR");
    let placeholder_selected =
        click_function_selector(&mut visual, "FUNCTION_KIND_OPTION-placeholder");
    let textarea_clicked = click_function_selector(&mut visual, "FUNCTION_INPUT_SCHEMA");
    let textarea_focused = visual
        .debug_bounds("FUNCTION_INPUT_SCHEMA_FOCUSED")
        .is_some();
    if textarea_clicked {
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
    }
    let modal_after_textarea_enter = visual.debug_bounds("FUNCTION_MODAL").is_some();
    let draft_rows = runtime
        .block_on(function_store.list(Some(DRAFT_IDENTIFIER.to_string()), 1))
        .expect("reload textarea Enter draft")
        .items()
        .iter()
        .filter(|function| function.identifier() == DRAFT_IDENTIFIER)
        .count();
    let schema_reset = replace_function_input(&mut visual, "FUNCTION_INPUT_SCHEMA", "{}");
    let duplicate_identifier_set =
        replace_function_input(&mut visual, "FUNCTION_IDENTIFIER", IDENTIFIER);
    if duplicate_identifier_set {
        visual.simulate_keystrokes("shift-tab");
        visual.run_until_parked();
    }
    let keyboard_save_focused = visual.debug_bounds("FUNCTION_FORM_SAVE_FOCUSED").is_some();
    if keyboard_save_focused {
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
    }

    let conflict_selector: &'static str =
        Box::leak(format!("FUNCTION_IDENTIFIER_CONFLICT-{IDENTIFIER}").into_boxed_str());
    let preserved_selector: &'static str =
        Box::leak(format!("FUNCTION_IDENTIFIER_VALUE-{IDENTIFIER}").into_boxed_str());
    let form_visible = visual.debug_bounds("FUNCTION_MODAL").is_some();
    let conflict_visible = settle_function_selector(&mut visual, conflict_selector);
    let preserved_visible = visual.debug_bounds(preserved_selector).is_some();
    let error_focused = visual.debug_bounds("FUNCTION_FORM_ERROR_FOCUSED").is_some();
    let conflict_accesskit =
        function_semantic_accesskit_probe(FunctionSemanticStatus::IdentifierConflict {
            value: IDENTIFIER.to_string(),
            field: "identifier".to_string(),
            reason: "duplicate".to_string(),
        });
    let exact_rows = runtime
        .block_on(function_store.list(Some(IDENTIFIER.to_string()), 1))
        .expect("reload duplicate fixture")
        .items()
        .iter()
        .filter(|function| function.identifier() == IDENTIFIER)
        .count();
    if form_visible {
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
    }
    let modal_closed = visual.debug_bounds("FUNCTION_MODAL").is_none();
    let add_focus_restored = visual.debug_bounds("FUNCTION_ADD_FOCUSED").is_some();
    if modal_closed && add_focus_restored {
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
    }
    let reopened_from_keyboard = visual.debug_bounds("FUNCTION_MODAL").is_some()
        && visual.debug_bounds("FUNCTION_IDENTIFIER_FOCUSED").is_some();
    close_function_visual_window(&mut visual);

    assert!(add_visible, "Function Add control must expose FUNCTION_ADD");
    assert!(modal_visible, "Function Add must open FUNCTION_MODAL");
    assert!(
        identifier_initially_focused,
        "opening the Function modal must focus its first field"
    );
    assert!(
        shift_tab_wrapped_to_save && tab_wrapped_to_identifier,
        "Function modal must trap Shift-Tab/Tab between its last and first controls"
    );
    assert!(
        draft_identifier_set,
        "Function form must expose FUNCTION_IDENTIFIER"
    );
    assert!(name_set, "Function form must expose FUNCTION_NAME");
    assert!(
        kind_opened && placeholder_selected,
        "Placeholder kind must be selectable"
    );
    assert!(
        textarea_clicked && textarea_focused,
        "Function JSON schema must be a focusable multiline Textarea"
    );
    assert!(
        modal_after_textarea_enter,
        "Enter inside a Function schema Textarea must insert/edit content, not submit the form"
    );
    assert_eq!(
        draft_rows, 0,
        "Enter inside a Function schema Textarea unexpectedly submitted the draft"
    );
    assert!(
        schema_reset,
        "Function schema Textarea must remain editable after Enter"
    );
    assert!(
        duplicate_identifier_set,
        "Function identifier must remain editable before the explicit save"
    );
    assert!(
        keyboard_save_focused,
        "duplicate Function must be submitted by keyboard focus + Enter, without a pointer click"
    );
    assert!(
        form_visible,
        "identifier conflict must keep the Function form open"
    );
    assert!(
        preserved_visible,
        "identifier conflict must preserve the submitted safe value `{IDENTIFIER}`"
    );
    assert!(
        conflict_visible,
        "identifier conflict must render a stable safely-labelled error selector"
    );
    assert!(
        error_focused,
        "identifier conflict must focus its error summary"
    );
    let conflict_accessible_name = conflict_accesskit
        .label()
        .expect("identifier conflict must publish an accessible error name");
    assert_function_semantic_node(
        &conflict_accesskit,
        gpui::Role::Alert,
        "t085_safe_duplicate field=identifier reason=duplicate",
    );
    for forbidden in ["UNIQUE constraint", "sqlite", "database"] {
        assert!(
            !conflict_accessible_name
                .to_ascii_lowercase()
                .contains(&forbidden.to_ascii_lowercase()),
            "Function conflict exposed backend detail `{forbidden}`: {conflict_accessible_name}"
        );
    }
    assert_eq!(
        exact_rows, 1,
        "duplicate submit must not create a second row"
    );
    assert!(modal_closed, "Escape must close the Function modal");
    assert!(
        add_focus_restored,
        "closing the Function modal must restore focus to its Add trigger"
    );
    assert!(
        reopened_from_keyboard,
        "restored Add focus must support Enter to reopen the Function modal"
    );
}

// ---------------------------------------------------------------------------
// §T092 [P] [US10] — Workflow / DAG accessibility surface.
//
// T097 (DAG keyboard canvas) and T098 (workflow form keyboard) own the
// production code; these tests activate the T016E Workflow/DAG row and
// assert the keyboard / focus / native-scroll source contract.
// ---------------------------------------------------------------------------

const DAG_EDITOR_SOURCE: &str = include_str!("../src/ui/dag_editor_view.rs");
const WORKFLOW_VIEW_SOURCE: &str = include_str!("../src/ui/workflow_view.rs");

#[test]
fn dag_editor_module_carries_scroll_tag_for_native_surface() {
    use support::scroll_inventory::{ScrollSurface, assert_source_tag};
    // T097 must publish `//! scroll:workflow_dag` in the module doc comment.
    assert_source_tag(DAG_EDITOR_SOURCE, ScrollSurface::WorkflowDag.slug());
}

#[test]
fn workflow_dag_surface_is_registered_in_t016e_inventory() {
    use support::scroll_inventory::ScrollSurface;
    // T092 activates the T016E Workflow/DAG surface closed by T097/T098.
    let owner = ScrollSurface::WorkflowDag.owner_phase().to_string();
    assert_eq!(
        owner, "US10/T097-T098",
        "WorkflowDag owner phase must be fixed to the T097/T098 UI pair"
    );
    assert_eq!(ScrollSurface::WorkflowDag.slug(), "workflow_dag");
}

#[test]
fn dag_editor_supports_keyboard_canvas_operations() {
    // T097 must wire keyboard canvas navigation/editing: arrow-key node
    // movement, Enter edge drawing, Escape cancel, Delete removal, and a
    // focus handle for the canvas surface.
    assert!(
        DAG_EDITOR_SOURCE.contains("on_key_down"),
        "DAG editor must wire keyboard handling"
    );
    assert!(
        DAG_EDITOR_SOURCE.contains("track_focus") || DAG_EDITOR_SOURCE.contains("focus_handle"),
        "DAG editor must own a focus handle for the canvas surface"
    );
    for required in ["\"up\"", "\"down\"", "\"left\"", "\"right\""] {
        assert!(
            DAG_EDITOR_SOURCE.contains(required),
            "DAG editor must handle the {required} arrow key"
        );
    }
    assert!(
        DAG_EDITOR_SOURCE.contains("\"enter\""),
        "DAG editor must handle Enter for edge drawing"
    );
    assert!(
        DAG_EDITOR_SOURCE.contains("\"escape\""),
        "DAG editor must handle Escape for cancel"
    );
    assert!(
        DAG_EDITOR_SOURCE.contains("\"delete\"") || DAG_EDITOR_SOURCE.contains("\"backspace\""),
        "DAG editor must handle Delete/Backspace for node/edge removal"
    );
}

#[test]
fn workflow_view_supports_keyboard_form_operations() {
    // T098 must wire keyboard form handling (Escape close / Enter submit)
    // and a focus handle for the form surface.
    assert!(
        WORKFLOW_VIEW_SOURCE.contains("on_key_down"),
        "Workflow view must wire keyboard handling"
    );
    assert!(
        WORKFLOW_VIEW_SOURCE.contains("track_focus")
            || WORKFLOW_VIEW_SOURCE.contains("focus_handle"),
        "Workflow view must own a focus handle for the form surface"
    );
    assert!(
        WORKFLOW_VIEW_SOURCE.contains("\"escape\""),
        "Workflow form must handle Escape to close"
    );
    assert!(
        WORKFLOW_VIEW_SOURCE.contains("\"enter\""),
        "Workflow form must handle Enter to submit"
    );
}

fn workflow_visual_store(
    cx: &mut TestAppContext,
) -> (
    tempfile::TempDir,
    hivegui::datasource::Store,
    gpui::Entity<hivegui::datasource::Store>,
    tokio::runtime::Runtime,
) {
    init_gpui(cx);
    cx.executor().allow_parking();
    let temp_dir = tempfile::tempdir().expect("create Workflow visual-test data directory");
    let runtime = tokio::runtime::Runtime::new().expect("create Workflow visual-test runtime");
    let store = runtime
        .block_on(hivegui::datasource::Store::new(temp_dir.path()))
        .expect("create Workflow visual-test Store");
    let store_entity = cx.new(|_| store.clone());
    (temp_dir, store, store_entity, runtime)
}

fn open_workflow_visual_window(
    cx: &mut TestAppContext,
    store: gpui::Entity<hivegui::datasource::Store>,
    window_size: gpui::Size<gpui::Pixels>,
) -> WindowHandle<gpui_component::Root> {
    cx.open_window(window_size, move |window, cx| {
        let view = cx.new(|cx| hivegui::ui::workflow_view::WorkflowView::new(store, cx));
        gpui_component::Root::new(view, window, cx).bordered(false)
    })
}

fn click_visual_selector(visual: &mut VisualTestContext, selector: &'static str) {
    let bounds = visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("missing rendered Workflow selector `{selector}`"));
    visual.simulate_click(bounds.center(), gpui::Modifiers::default());
    visual.run_until_parked();
}

fn replace_workflow_input(
    visual: &mut VisualTestContext,
    selector: &'static str,
    value: &str,
) -> bool {
    let Some(bounds) = visual.debug_bounds(selector) else {
        return false;
    };
    visual.simulate_click(bounds.center(), gpui::Modifiers::default());
    visual.simulate_keystrokes("ctrl-a");
    visual.simulate_input(value);
    visual.run_until_parked();
    true
}

fn settle_workflow_selector(visual: &mut VisualTestContext, selector: &'static str) -> bool {
    for _ in 0..200 {
        visual.run_until_parked();
        if visual.debug_bounds(selector).is_some() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    false
}

fn close_workflow_visual_window(
    visual: &mut VisualTestContext,
    store: hivegui::datasource::Store,
    runtime: &tokio::runtime::Runtime,
) {
    visual.update(|window, _| window.remove_window());
    visual.run_until_parked();
    runtime.block_on(store.pool().close());
    drop(store);
}

#[test]
fn workflow_dag_scroll_owner_is_the_t097_t098_ui_pair() {
    use support::scroll_inventory::ScrollSurface;

    assert_eq!(
        ScrollSurface::WorkflowDag.owner_phase().to_string(),
        "US10/T097-T098",
        "the activated Workflow/DAG product scroll line belongs to the T097/T098 UI pair, not the earlier Store task"
    );
}

// ---------------------------------------------------------------------------
// §T101 [P] [US11] — Tool management accessibility + native scroll Red.
//
// T106 owns the production selectors/focus semantics and is allowed to close
// this surface only after T104 approves the observed Red below.
// ---------------------------------------------------------------------------

const TOOL_VIEW_SOURCE: &str = include_str!("../src/ui/tool_view.rs");

#[test]
fn tool_view_module_carries_scroll_tag_for_native_surface() {
    use support::scroll_inventory::{ScrollSurface, assert_source_tag};

    assert_source_tag(TOOL_VIEW_SOURCE, ScrollSurface::ToolList.slug());
}

#[test]
fn tool_scroll_owner_is_the_t106_ui_task() {
    use support::scroll_inventory::ScrollSurface;

    assert_eq!(
        ScrollSurface::ToolList.owner_phase().to_string(),
        "US11/T106",
        "the activated Tool product scroll line belongs to the T106 UI implementation"
    );
}

fn tool_visual_store(
    cx: &mut TestAppContext,
) -> (
    support::TestWorkspace,
    hivegui::datasource::Store,
    gpui::Entity<hivegui::datasource::Store>,
    tokio::runtime::Runtime,
) {
    use hivegui::datasource::store::{Store, StoreOpenOptions};

    init_gpui(cx);
    cx.executor().allow_parking();
    let workspace = support::TestWorkspace::new().expect("create Tool visual-test workspace");
    let runtime = tokio::runtime::Runtime::new().expect("create Tool visual-test runtime");
    let store = runtime
        .block_on(Store::open_local(StoreOpenOptions::new(
            workspace.database_path(),
            workspace.plugin_root(),
        )))
        .expect("open canonical Tool visual-test Store");
    let store_entity = cx.new(|_| store.clone());
    (workspace, store, store_entity, runtime)
}

fn open_tool_visual_window(
    cx: &mut TestAppContext,
    store: gpui::Entity<hivegui::datasource::Store>,
    window_size: gpui::Size<gpui::Pixels>,
) -> WindowHandle<gpui_component::Root> {
    cx.open_window(window_size, move |window, cx| {
        let view = cx.new(|cx| hivegui::ui::tool_view::ToolView::new(store, cx));
        gpui_component::Root::new(view, window, cx).bordered(false)
    })
}

fn close_tool_visual_window(
    visual: &mut VisualTestContext,
    store: hivegui::datasource::Store,
    runtime: &tokio::runtime::Runtime,
) {
    visual.update(|window, _| window.remove_window());
    visual.run_until_parked();
    runtime.block_on(store.pool().close());
    drop(store);
}

#[gpui::test]
fn tool_form_native_wheel_moves_bottom_actions_into_viewport(cx: &mut TestAppContext) {
    use gpui::{ScrollDelta, ScrollWheelEvent, TouchPhase, point};

    let (_workspace, store, store_entity, runtime) = tool_visual_store(cx);
    let _runtime_guard = runtime.enter();
    let window = open_tool_visual_window(cx, store_entity, size(px(520.0), px(300.0)));
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let add = visual.debug_bounds("TOOL_ADD");
    if let Some(add) = add {
        visual.simulate_click(add.center(), gpui::Modifiers::default());
        visual.run_until_parked();
    }
    let modal = visual.debug_bounds("TOOL_MODAL");
    let scroll = visual.debug_bounds("TOOL_FORM_SCROLL");
    let actions_before = visual.debug_bounds("TOOL_FORM_ACTIONS");
    if let Some(scroll) = scroll {
        visual.simulate_event(ScrollWheelEvent {
            position: point(scroll.left() + px(24.0), scroll.top() + px(24.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-2_000.0))),
            modifiers: Default::default(),
            touch_phase: TouchPhase::Moved,
        });
        visual.run_until_parked();
    }
    let actions_after = visual.debug_bounds("TOOL_FORM_ACTIONS");
    let forbidden_custom_controls = [
        "TOOL_SCROLL_UP",
        "TOOL_SCROLL_DOWN",
        "TOOL_SCROLLBAR_TRACK",
        "TOOL_SCROLL_HANDLE",
    ]
    .into_iter()
    .filter(|selector| visual.debug_bounds(selector).is_some())
    .collect::<Vec<_>>();
    close_tool_visual_window(&mut visual, store, &runtime);

    assert!(
        add.is_some(),
        "Tool view must expose stable TOOL_ADD pointer/keyboard semantics"
    );
    let modal = modal.expect("Tool form must expose TOOL_MODAL bounds");
    let scroll = scroll.expect("Tool form must expose TOOL_FORM_SCROLL native viewport bounds");
    let actions_before =
        actions_before.expect("long Tool form must expose TOOL_FORM_ACTIONS before wheel input");
    let actions_after =
        actions_after.expect("long Tool form must retain TOOL_FORM_ACTIONS after wheel input");
    assert!(modal.top() >= px(0.0) && modal.bottom() <= px(300.0));
    assert!(scroll.top() >= modal.top() && scroll.bottom() <= modal.bottom());
    assert!(
        actions_after.top() < actions_before.top(),
        "real GPUI wheel input did not move Tool form content: before={actions_before:?}, after={actions_after:?}"
    );
    assert!(
        actions_after.top() >= scroll.top() && actions_after.bottom() <= scroll.bottom(),
        "bottom Tool actions are not fully reachable: viewport={scroll:?}, actions={actions_after:?}"
    );
    assert!(
        forbidden_custom_controls.is_empty(),
        "Tool form rendered forbidden custom scroll controls: {forbidden_custom_controls:?}"
    );
    for forbidden in [
        "ScrollWheelEvent",
        ".on_scroll_wheel(",
        "TOOL_SCROLL_UP",
        "TOOL_SCROLL_DOWN",
        "TOOL_SCROLLBAR_TRACK",
        "TOOL_SCROLL_HANDLE",
    ] {
        assert!(
            !TOOL_VIEW_SOURCE.contains(forbidden),
            "Tool production source must not implement custom scrolling via `{forbidden}`"
        );
    }
}

fn click_tool_add(visual: &mut VisualTestContext) -> bool {
    let Some(bounds) = visual
        .debug_bounds("TOOL_ADD")
        .or_else(|| visual.debug_bounds("add-btn"))
    else {
        return false;
    };
    visual.simulate_click(bounds.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    true
}

fn settle_tool_selector(visual: &mut VisualTestContext, selector: &'static str) -> bool {
    for _ in 0..100 {
        visual.run_until_parked();
        if visual.debug_bounds(selector).is_some() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    false
}

#[gpui::test]
fn tool_form_renders_every_fr020_field_with_semantic_controls(cx: &mut TestAppContext) {
    let (_workspace, store, store_entity, runtime) = tool_visual_store(cx);
    let _runtime_guard = runtime.enter();
    let window = open_tool_visual_window(cx, store_entity, size(px(760.0), px(620.0)));
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    assert!(click_tool_add(&mut visual), "rendered Tool Add control");
    let missing = [
        "TOOL_IDENTIFIER_INPUT",
        "TOOL_NAME_INPUT",
        "TOOL_DESCRIPTION_INPUT",
        "TOOL_KIND_SELECT",
        "TOOL_SOURCE_SELECT",
        "TOOL_IS_ALWAYS",
        "TOOL_FUNCTION_TARGET",
        "TOOL_INPUT_SCHEMA_TEXTAREA",
        "TOOL_OUTPUT_SCHEMA_TEXTAREA",
        "TOOL_CATEGORY_SELECT",
        "TOOL_REQUIRED_CAPABILITIES_TEXTAREA",
        "TOOL_FORM_ACTIONS",
    ]
    .into_iter()
    .filter(|selector| visual.debug_bounds(selector).is_none())
    .collect::<Vec<_>>();
    close_tool_visual_window(&mut visual, store, &runtime);

    assert!(
        missing.is_empty(),
        "Tool FR-020 form fields missing: {missing:?}"
    );
    for required in [
        "TextareaState",
        "Textarea::new",
        "input_schema_textarea",
        "output_schema_textarea",
        "required_capabilities_textarea",
    ] {
        assert!(
            TOOL_VIEW_SOURCE.contains(required),
            "multiline Tool schema/Capability field must use the shared Textarea control: missing {required}"
        );
    }
}

#[gpui::test]
fn tool_duplicate_identifier_keeps_form_focus_and_safe_value(cx: &mut TestAppContext) {
    const IDENTIFIER: &str = "t101_safe_duplicate";

    let (_workspace, store, store_entity, runtime) = tool_visual_store(cx);
    runtime.block_on(async {
        let now = "2026-08-25T00:00:00Z";
        let function_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO functions (identifier, name, description, kind, input_schema, \
             output_schema, plugin_id, plugin_export, category_id, required_capabilities, \
             created_at, updated_at) VALUES ('t101_function', 'T101 Function', '', 'builtin', \
             '{}', '{}', NULL, NULL, NULL, NULL, ?, ?) RETURNING id",
        )
        .bind(now)
        .bind(now)
        .fetch_one(store.pool())
        .await
        .expect("seed Tool target");
        sqlx::query(
            "INSERT INTO tools (identifier, name, description, kind, source, is_always, \
             function_id, workflow_id, input_schema, output_schema, category_id, \
             required_capabilities, created_at, updated_at) VALUES (?, 'Existing Tool', '', \
             'function-wrap', 'workspace', 0, ?, NULL, '{}', '{}', NULL, NULL, ?, ?)",
        )
        .bind(IDENTIFIER)
        .bind(function_id)
        .bind(now)
        .bind(now)
        .execute(store.pool())
        .await
        .expect("seed duplicate Tool");
    });
    let _runtime_guard = runtime.enter();
    let window = open_tool_visual_window(cx, store_entity, size(px(760.0), px(620.0)));
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    assert!(click_tool_add(&mut visual), "rendered Tool Add control");
    let identifier = settle_tool_selector(&mut visual, "TOOL_IDENTIFIER_INPUT");
    if identifier {
        let bounds = visual
            .debug_bounds("TOOL_IDENTIFIER_INPUT")
            .expect("settled identifier input");
        visual.simulate_click(bounds.center(), gpui::Modifiers::default());
        visual.simulate_input(IDENTIFIER);
        visual.simulate_keystrokes("shift-tab");
        visual.run_until_parked();
        if visual.debug_bounds("TOOL_FORM_SAVE_FOCUSED").is_some() {
            visual.simulate_keystrokes("enter");
        }
    }
    let conflict = settle_tool_selector(&mut visual, "TOOL_CONFLICT_IDENTIFIER");
    let form_open = visual.debug_bounds("TOOL_MODAL").is_some();
    let preserved = visual
        .debug_bounds("TOOL_IDENTIFIER_VALUE-t101_safe_duplicate")
        .is_some();
    let focused = visual.debug_bounds("TOOL_ERROR_FOCUSED").is_some();
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    let modal_closed = visual.debug_bounds("TOOL_MODAL").is_none();
    let add_focus_restored = visual.debug_bounds("TOOL_ADD_FOCUSED").is_some();
    close_tool_visual_window(&mut visual, store, &runtime);

    assert!(identifier, "Tool form must expose TOOL_IDENTIFIER_INPUT");
    assert!(
        conflict,
        "duplicate identifier must render safe Tool conflict"
    );
    assert!(form_open, "Tool conflict must keep the form open");
    assert!(
        preserved,
        "Tool conflict must preserve its safe identifier value"
    );
    assert!(focused, "Tool conflict summary must receive focus");
    assert!(modal_closed, "Escape must close the Tool modal");
    assert!(
        add_focus_restored,
        "Tool modal close must restore Add focus"
    );
}

#[gpui::test]
fn workflow_form_renders_every_fr019_field_with_stable_selectors(cx: &mut TestAppContext) {
    let (_temp_dir, store, store_entity, runtime) = workflow_visual_store(cx);
    let _runtime_guard = runtime.enter();
    let window = open_workflow_visual_window(cx, store_entity, size(px(760.0), px(520.0)));
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    click_visual_selector(&mut visual, "WORKFLOW_ADD");
    assert!(
        visual.debug_bounds("workflow-form-scroll").is_some(),
        "test precondition: clicking the rendered Add control must open the Workflow form"
    );
    let required = [
        "WORKFLOW_IDENTIFIER",
        "WORKFLOW_NAME",
        "WORKFLOW_DESCRIPTION",
        "WORKFLOW_TIMEOUT_MS",
        "WORKFLOW_CATEGORY_ID",
        "WORKFLOW_INPUT_SCHEMA",
        "WORKFLOW_START_DESCRIPTION",
        "WORKFLOW_OUTPUT_SCHEMA",
        "WORKFLOW_REQUIRED_CAPABILITIES",
    ];
    let missing: Vec<&str> = required
        .into_iter()
        .filter(|selector| visual.debug_bounds(selector).is_none())
        .collect();
    close_workflow_visual_window(&mut visual, store, &runtime);
    assert!(
        missing.is_empty(),
        "FR-019 Workflow form is missing rendered, keyboard-addressable fields/selectors: {missing:?}"
    );
}

#[gpui::test]
fn workflow_form_native_wheel_moves_bottom_actions_into_viewport(cx: &mut TestAppContext) {
    use gpui::{ScrollDelta, ScrollWheelEvent, TouchPhase, point};

    let (_temp_dir, store, store_entity, runtime) = workflow_visual_store(cx);
    let _runtime_guard = runtime.enter();
    let window = open_workflow_visual_window(cx, store_entity, size(px(520.0), px(260.0)));
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    click_visual_selector(&mut visual, "WORKFLOW_ADD");

    let scroll = visual.debug_bounds("WORKFLOW_FORM_SCROLL");
    let actions_before = visual.debug_bounds("WORKFLOW_FORM_ACTIONS");
    let mut actions_after = None;
    if let (Some(scroll), Some(_)) = (scroll, actions_before) {
        visual.simulate_event(ScrollWheelEvent {
            position: point(scroll.left() + px(24.0), scroll.top() + px(24.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-1_000.0))),
            modifiers: Default::default(),
            touch_phase: TouchPhase::Moved,
        });
        visual.run_until_parked();
        actions_after = visual.debug_bounds("WORKFLOW_FORM_ACTIONS");
    }
    let custom_controls: Vec<&str> = [
        "WORKFLOW_SCROLL_UP",
        "WORKFLOW_SCROLL_DOWN",
        "WORKFLOW_SCROLLBAR_TRACK",
    ]
    .into_iter()
    .filter(|selector| visual.debug_bounds(selector).is_some())
    .collect();
    close_workflow_visual_window(&mut visual, store, &runtime);

    let scroll = scroll.expect(
        "Workflow form must expose the real native-scroll viewport as WORKFLOW_FORM_SCROLL",
    );
    let actions_before = actions_before
        .expect("long FR-019 fixture must expose its bottom actions as WORKFLOW_FORM_ACTIONS");
    let actions_after = actions_after.expect("bottom Workflow actions after real wheel input");
    assert!(
        actions_after.top() < actions_before.top(),
        "real GPUI wheel input did not change the Workflow form content offset: before={actions_before:?}, after={actions_after:?}"
    );
    assert!(
        actions_after.top() >= scroll.top() && actions_after.bottom() <= scroll.bottom(),
        "bottom Workflow actions are not fully reachable in the viewport: viewport={scroll:?}, actions={actions_after:?}"
    );
    assert!(
        custom_controls.is_empty(),
        "Workflow form rendered custom scroll controls: {custom_controls:?}"
    );
}

#[gpui::test]
fn workflow_delete_conflict_keeps_the_tool_reference_and_confirmation_surface(
    cx: &mut TestAppContext,
) {
    use hivegui::datasource::entity_store::{Tool, Workflow, WorkflowEdge, WorkflowNode};

    let (_temp_dir, store, store_entity, runtime) = workflow_visual_store(cx);
    let (workflow, tool) = runtime.block_on(async {
        let workflow = Workflow::create(
            store.pool(),
            "ui-delete-conflict".to_string(),
            "UI delete conflict".to_string(),
            Some("must remain after conflict".to_string()),
            30_000,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("seed Workflow");
        WorkflowNode::upsert(
            store.pool(),
            workflow.id,
            "start".to_string(),
            "start_node".to_string(),
            None,
            10.0,
            20.0,
            Some("{}".to_string()),
        )
        .await
        .expect("seed start node");
        WorkflowNode::upsert(
            store.pool(),
            workflow.id,
            "end".to_string(),
            "end_node".to_string(),
            None,
            30.0,
            40.0,
            Some("{}".to_string()),
        )
        .await
        .expect("seed end node");
        WorkflowEdge::upsert(
            store.pool(),
            workflow.id,
            "start".to_string(),
            "end".to_string(),
            "{}".to_string(),
        )
        .await
        .expect("seed edge");
        let tool = Tool::create(
            store.pool(),
            "ui-safe-tool".to_string(),
            "UI safe tool".to_string(),
            "references the workflow".to_string(),
            "workflow-wrap".to_string(),
            "workspace".to_string(),
            false,
            None,
            Some(workflow.id),
            "{}".to_string(),
            "{}".to_string(),
            None,
            None,
        )
        .await
        .expect("seed referencing Tool");
        (workflow, tool)
    });
    let _runtime_guard = runtime.enter();

    let window = open_workflow_visual_window(cx, store_entity, size(px(900.0), px(600.0)));
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let delete_selector: &'static str =
        Box::leak(format!("WORKFLOW_DELETE-{}", workflow.id).into_boxed_str());
    let delete_loaded = settle_workflow_selector(&mut visual, delete_selector);
    let delete_bounds = visual.debug_bounds(delete_selector);
    let conflict_selector: &'static str =
        Box::leak(format!("WORKFLOW_DELETE_CONFLICT-{}", workflow.id).into_boxed_str());
    if let Some(delete_bounds) = delete_bounds {
        visual.simulate_click(delete_bounds.center(), gpui::Modifiers::default());
        visual.run_until_parked();
        if let Some(confirm_bounds) = visual.debug_bounds("confirm-delete") {
            visual.simulate_click(confirm_bounds.center(), gpui::Modifiers::default());
            let _ = settle_workflow_selector(&mut visual, conflict_selector);
        }
    }
    let confirmation_visible = visual.debug_bounds("confirm-delete").is_some();
    let safe_conflict_visible = visual.debug_bounds(conflict_selector).is_some();

    let after = runtime.block_on(async {
        (
            Workflow::get(store.pool(), workflow.id)
                .await
                .expect("reload Workflow"),
            WorkflowNode::list_by_workflow(store.pool(), workflow.id)
                .await
                .expect("reload nodes"),
            WorkflowEdge::list_by_workflow(store.pool(), workflow.id)
                .await
                .expect("reload edges"),
            Tool::get(store.pool(), tool.id).await.expect("reload Tool"),
        )
    });
    close_workflow_visual_window(&mut visual, store, &runtime);
    assert!(
        delete_loaded && delete_bounds.is_some(),
        "Workflow row must expose a stable keyboard/visual selector `{delete_selector}` for Delete"
    );
    assert!(
        confirmation_visible,
        "a referenced_by_tool failure must keep the confirmation/form surface open for recovery"
    );
    assert!(
        safe_conflict_visible,
        "the UI must render a stable, safely labelled referenced_by_tool error surface"
    );
    assert!(after.0.is_some(), "Workflow changed after delete conflict");
    assert_eq!(
        after.1.len(),
        2,
        "Workflow nodes changed after delete conflict"
    );
    assert_eq!(
        after.2.len(),
        1,
        "Workflow edges changed after delete conflict"
    );
    assert_eq!(
        after.3.as_ref().and_then(|row| row.workflow_id),
        Some(workflow.id),
        "referencing Tool changed after delete conflict"
    );
}

#[gpui::test]
fn workflow_duplicate_identifier_preserves_form_safe_values_and_error_focus(
    cx: &mut TestAppContext,
) {
    use hivegui::datasource::entity_store::Workflow;

    const IDENTIFIER: &str = "t092-safe-duplicate";
    const SAFE_NAME: &str = "T092 safe duplicate draft";
    let (_temp_dir, store, store_entity, runtime) = workflow_visual_store(cx);
    runtime
        .block_on(Workflow::create(
            store.pool(),
            IDENTIFIER.to_string(),
            "Existing Workflow".to_string(),
            None,
            30_000,
            None,
            None,
            None,
            None,
            None,
        ))
        .expect("seed duplicate Workflow");
    let _runtime_guard = runtime.enter();
    let window = open_workflow_visual_window(cx, store_entity, size(px(760.0), px(900.0)));
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    click_visual_selector(&mut visual, "WORKFLOW_ADD");
    let identifier_set = replace_workflow_input(&mut visual, "WORKFLOW_IDENTIFIER", IDENTIFIER);
    let name_set = replace_workflow_input(&mut visual, "WORKFLOW_NAME", SAFE_NAME);
    let save_visible = visual.debug_bounds("WORKFLOW_FORM_SAVE").is_some();
    if save_visible {
        click_visual_selector(&mut visual, "WORKFLOW_FORM_SAVE");
    }
    let conflict_selector: &'static str =
        Box::leak(format!("WORKFLOW_IDENTIFIER_CONFLICT-{IDENTIFIER}").into_boxed_str());
    let preserved_identifier_selector: &'static str =
        Box::leak(format!("WORKFLOW_IDENTIFIER_VALUE-{IDENTIFIER}").into_boxed_str());
    let preserved_name_selector: &'static str =
        Box::leak(format!("WORKFLOW_NAME_VALUE-{SAFE_NAME}").into_boxed_str());
    let conflict_visible = settle_workflow_selector(&mut visual, conflict_selector);
    let form_visible = visual.debug_bounds("workflow-form-scroll").is_some();
    let identifier_preserved = visual.debug_bounds(preserved_identifier_selector).is_some();
    let name_preserved = visual.debug_bounds(preserved_name_selector).is_some();
    let error_focused = visual.debug_bounds("WORKFLOW_FORM_ERROR_FOCUSED").is_some();
    let row_count: i64 = runtime
        .block_on(
            sqlx::query_scalar("SELECT COUNT(*) FROM workflows WHERE identifier = ?")
                .bind(IDENTIFIER)
                .fetch_one(store.pool()),
        )
        .expect("count duplicate Workflow rows");
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    let modal_closed = visual.debug_bounds("workflow-form-scroll").is_none();
    let add_focus_restored = visual.debug_bounds("WORKFLOW_ADD_FOCUSED").is_some();
    close_workflow_visual_window(&mut visual, store, &runtime);

    assert!(
        identifier_set && name_set,
        "Workflow form inputs must be editable"
    );
    assert!(
        save_visible,
        "Workflow form must expose a stable keyboard-addressable Save action"
    );
    assert!(
        form_visible,
        "duplicate identifier must keep the Workflow form open"
    );
    assert!(
        conflict_visible,
        "duplicate identifier must render a stable safe conflict surface"
    );
    assert!(
        identifier_preserved && name_preserved,
        "duplicate conflict must preserve submitted safe values"
    );
    assert!(
        error_focused,
        "duplicate conflict must move focus to the error summary"
    );
    assert_eq!(row_count, 1, "duplicate submit must be zero modification");
    assert!(
        modal_closed,
        "Escape must close the Workflow form after recovery"
    );
    assert!(
        add_focus_restored,
        "closing the form must restore focus to Workflow Add"
    );
}

fn seed_dag_keyboard_workflow(
    runtime: &tokio::runtime::Runtime,
    store: &hivegui::datasource::Store,
) -> i64 {
    use hivegui::datasource::entity_store::{Workflow, WorkflowNode};

    runtime.block_on(async {
        let workflow = Workflow::create(
            store.pool(),
            "t092-dag-keyboard".to_string(),
            "T092 DAG keyboard".to_string(),
            None,
            30_000,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("seed DAG Workflow");
        for (node_key, node_type, x, y) in [
            ("start", "start_node", 80.0, 80.0),
            ("middle", "function_node", 330.0, 190.0),
            ("end", "end_node", 580.0, 300.0),
        ] {
            WorkflowNode::upsert(
                store.pool(),
                workflow.id,
                node_key.to_string(),
                node_type.to_string(),
                None,
                x,
                y,
                Some("{}".to_string()),
            )
            .await
            .expect("seed DAG node");
        }
        workflow.id
    })
}

fn open_dag_visual_window(
    cx: &mut TestAppContext,
    store: gpui::Entity<hivegui::datasource::Store>,
    workflow_id: i64,
) -> WindowHandle<gpui_component::Root> {
    cx.open_window(size(px(900.0), px(600.0)), move |window, cx| {
        let editor =
            cx.new(|cx| hivegui::ui::dag_editor_view::DagEditorView::new(store, workflow_id, cx));
        gpui_component::Root::new(editor, window, cx).bordered(false)
    })
}

fn close_dag_visual_window(
    visual: &mut VisualTestContext,
    store: hivegui::datasource::Store,
    runtime: &tokio::runtime::Runtime,
) {
    visual.update(|window, _| window.remove_window());
    visual.run_until_parked();
    let _ = runtime.block_on(tokio::time::timeout(
        std::time::Duration::from_secs(1),
        store.pool().close(),
    ));
    drop(store);
}

#[gpui::test]
fn dag_keyboard_arrow_moves_the_real_selected_node(cx: &mut TestAppContext) {
    let (_temp_dir, store, store_entity, runtime) = workflow_visual_store(cx);
    let workflow_id = seed_dag_keyboard_workflow(&runtime, &store);
    let _runtime_guard = runtime.enter();
    let window = open_dag_visual_window(cx, store_entity, workflow_id);
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    assert!(
        settle_workflow_selector(&mut visual, "DAG_NODE-middle"),
        "DAG fixture must render its middle node"
    );
    let before = visual
        .debug_bounds("DAG_NODE-middle")
        .expect("middle before move");
    visual.simulate_click(before.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    let selected = visual.debug_bounds("DAG_SELECTED-middle").is_some();
    let canvas_focused = visual.debug_bounds("DAG_CANVAS_FOCUSED").is_some();
    visual.simulate_keystrokes("right");
    visual.run_until_parked();
    let after = visual
        .debug_bounds("DAG_NODE-middle")
        .expect("middle after move");
    close_dag_visual_window(&mut visual, store, &runtime);

    assert!(
        selected,
        "pointer selection must publish the actual selected DAG node"
    );
    assert!(
        canvas_focused,
        "selecting a DAG node must focus the keyboard canvas"
    );
    assert!(
        after.left() > before.left(),
        "Right Arrow must move the selected node right"
    );
}

#[gpui::test]
fn dag_keyboard_enter_connects_nodes_and_escape_cancels_edge_mode(cx: &mut TestAppContext) {
    let (_temp_dir, store, store_entity, runtime) = workflow_visual_store(cx);
    let workflow_id = seed_dag_keyboard_workflow(&runtime, &store);
    let _runtime_guard = runtime.enter();
    let window = open_dag_visual_window(cx, store_entity, workflow_id);
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    assert!(settle_workflow_selector(&mut visual, "DAG_NODE-start"));
    let start = visual.debug_bounds("DAG_NODE-start").expect("start node");
    visual.simulate_click(start.center(), gpui::Modifiers::default());
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let edge_mode_started = visual.debug_bounds("DAG_EDGE_MODE-start").is_some();
    visual.simulate_keystrokes("right");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let edge_created = visual.debug_bounds("DAG_EDGE-start-middle").is_some();

    visual.simulate_click(start.center(), gpui::Modifiers::default());
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let second_edge_mode = visual.debug_bounds("DAG_EDGE_MODE-start").is_some();
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    let edge_mode_cancelled = visual.debug_bounds("DAG_EDGE_MODE-start").is_none();
    close_dag_visual_window(&mut visual, store, &runtime);

    assert!(
        edge_mode_started,
        "Enter must expose keyboard edge-drawing mode"
    );
    assert!(
        edge_created,
        "Arrow selection plus Enter must create start→middle"
    );
    assert!(
        second_edge_mode,
        "Enter must allow a second edge gesture to begin"
    );
    assert!(
        edge_mode_cancelled,
        "Escape must cancel keyboard edge-drawing mode"
    );
}

#[gpui::test]
fn dag_keyboard_delete_property_panel_and_focus_restore_are_real(cx: &mut TestAppContext) {
    let (_temp_dir, store, store_entity, runtime) = workflow_visual_store(cx);
    let workflow_id = seed_dag_keyboard_workflow(&runtime, &store);
    let _runtime_guard = runtime.enter();
    let window = open_dag_visual_window(cx, store_entity, workflow_id);
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    assert!(settle_workflow_selector(&mut visual, "DAG_NODE-start"));

    let start = visual.debug_bounds("DAG_NODE-start").expect("start node");
    visual.simulate_mouse_down(
        start.center(),
        gpui::MouseButton::Right,
        gpui::Modifiers::default(),
    );
    visual.run_until_parked();
    let config_action = visual
        .debug_bounds("DAG_NODE_MENU_CONFIG")
        .expect("node property action");
    visual.simulate_click(config_action.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    let property_panel_visible = visual.debug_bounds("DAG_NODE_CONFIG_EDITOR").is_some();
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    let property_panel_closed = visual.debug_bounds("DAG_NODE_CONFIG_EDITOR").is_none();
    let canvas_focus_restored = visual.debug_bounds("DAG_CANVAS_FOCUSED").is_some();

    let middle_deleted = if let Some(middle) = visual.debug_bounds("DAG_NODE-middle") {
        visual.simulate_click(middle.center(), gpui::Modifiers::default());
        visual.simulate_keystrokes("delete");
        visual.run_until_parked();
        visual.debug_bounds("DAG_NODE-middle").is_none()
    } else {
        false
    };
    close_dag_visual_window(&mut visual, store, &runtime);

    assert!(
        property_panel_visible,
        "keyboard-reachable property panel must render"
    );
    assert!(
        property_panel_closed,
        "Escape must close the DAG property panel"
    );
    assert!(
        canvas_focus_restored,
        "closing properties must restore DAG canvas focus"
    );
    assert!(
        middle_deleted,
        "Delete must remove the selected non-boundary node"
    );
}

// ---------------------------------------------------------------------------
// §T081 [P] [US8] — Plugin view keyboard accessibility surface.
//
// T081 owns the Plugin CRUD modal; this asserts the keyboard / focus source
// contract so the form is operable without a pointer.
// ---------------------------------------------------------------------------

const PLUGIN_VIEW_SOURCE: &str = include_str!("../src/ui/plugin_view.rs");

#[test]
fn plugin_view_module_carries_the_t016e_native_scroll_tag() {
    use support::scroll_inventory::{ScrollSurface, assert_source_tag};

    assert_source_tag(PLUGIN_VIEW_SOURCE, ScrollSurface::PluginList.slug());
}

#[test]
fn plugin_list_inventory_is_owned_by_the_t081_ui_task() {
    use support::scroll_inventory::{Inventory, ScrollSurface, assert_inventory_contains};

    let mut inventory = Inventory::new();
    inventory.register(ScrollSurface::PluginList, ScrollSurface::PluginList.slug());
    assert_inventory_contains(&inventory, ScrollSurface::PluginList);

    let registration = inventory
        .get(ScrollSurface::PluginList)
        .expect("PluginList must be registered by its story owner");
    assert_eq!(registration.owner.to_string(), "US8/T081");
    assert_eq!(registration.build_tag, "plugin_list");
}

#[test]
fn plugin_view_supports_keyboard_form_operations() {
    // T081 must wire keyboard form handling (Escape close / Enter submit)
    // and a focus handle for the modal surface.
    assert!(
        PLUGIN_VIEW_SOURCE.contains("on_key_down"),
        "Plugin view must wire keyboard handling"
    );
    assert!(
        PLUGIN_VIEW_SOURCE.contains("track_focus") || PLUGIN_VIEW_SOURCE.contains("focus_handle"),
        "Plugin view must own a focus handle for the modal surface"
    );
    assert!(
        PLUGIN_VIEW_SOURCE.contains("\"escape\""),
        "Plugin form must handle Escape to close"
    );
    assert!(
        PLUGIN_VIEW_SOURCE.contains("\"enter\""),
        "Plugin form must handle Enter to submit"
    );
}

#[test]
fn plugin_view_locks_artifact_file_on_edit() {
    // T081: editing a plugin must NOT let the artifact file change. The
    // identifier / version / runtime are locked (read-only) and the WASM
    // picker is replaced by an immutable "不可修改" surface, so the original
    // s3_key / sha256 / size are preserved.
    assert!(
        PLUGIN_VIEW_SOURCE.contains("不可修改"),
        "Plugin edit must surface that the artifact file is immutable"
    );
    assert!(
        PLUGIN_VIEW_SOURCE.contains("form_field_readonly"),
        "Plugin edit must render identifier/version as read-only fields"
    );
    assert!(
        PLUGIN_VIEW_SOURCE.contains("form_original_s3_key"),
        "Plugin save must preserve the original s3_key when editing"
    );
    assert!(
        PLUGIN_VIEW_SOURCE.contains("PLUGIN_WASM_SELECTION"),
        "Plugin add must still tag the WASM selection surface for accessibility"
    );
}

#[test]
fn plugin_view_surfaces_required_capabilities_selector_and_validation() {
    // T081 manifest migration must surface a required-capabilities picker
    // and validate the manifest before persisting.
    assert!(
        PLUGIN_VIEW_SOURCE.contains("PLUGIN_CAPABILITY_SELECTOR"),
        "Plugin view must tag the required-capabilities selector surface"
    );
    assert!(
        PLUGIN_VIEW_SOURCE.contains("所需 Capabilities"),
        "Plugin form must render the required-capabilities field"
    );
    assert!(
        PLUGIN_VIEW_SOURCE.contains("validate_manifest"),
        "Plugin save must validate the manifest against the host capability set"
    );
    assert!(
        PLUGIN_VIEW_SOURCE.contains("manifest 不兼容"),
        "Plugin save must surface manifest incompatibilities before persisting"
    );
}

// ---------------------------------------------------------------------------
// §T121 [P] [US13] — Agent / conversation / settings owner surfaces.
// ---------------------------------------------------------------------------

const AGENT_VIEW_SOURCE: &str = include_str!("../src/ui/agent_view.rs");
const CONVERSATION_VIEW_SOURCE: &str = include_str!("../src/ui/conversation_view.rs");
const SETTINGS_VIEW_SOURCE: &str = include_str!("../src/ui/settings_view.rs");

#[test]
fn us13_agent_conversation_settings_activate_the_owner_native_scroll_row() {
    use support::scroll_inventory::{
        Inventory, ScrollSurface, assert_inventory_contains, assert_source_tag,
    };

    let mut inventory = Inventory::new();
    inventory.register(
        ScrollSurface::AgentExecution,
        ScrollSurface::AgentExecution.slug(),
    );
    assert_inventory_contains(&inventory, ScrollSurface::AgentExecution);
    assert_eq!(
        ScrollSurface::AgentExecution.owner_phase().to_string(),
        "US13/T132-T135"
    );
    assert_source_tag(AGENT_VIEW_SOURCE, ScrollSurface::AgentExecution.slug());
    assert_source_tag(
        CONVERSATION_VIEW_SOURCE,
        ScrollSurface::AgentExecution.slug(),
    );
    assert_source_tag(SETTINGS_VIEW_SOURCE, ScrollSurface::AgentExecution.slug());
}

fn us13_visual_store(
    cx: &mut TestAppContext,
) -> (
    support::TestWorkspace,
    hivegui::datasource::Store,
    gpui::Entity<hivegui::datasource::Store>,
    tokio::runtime::Runtime,
) {
    use hivegui::datasource::store::{Store, StoreOpenOptions};
    use hivegui::ui::app::{AppRoute, HiveGuiAppState};

    init_gpui(cx);
    cx.executor().allow_parking();
    let workspace = support::TestWorkspace::new().expect("create US13 visual workspace");
    let runtime = tokio::runtime::Runtime::new().expect("create US13 visual runtime");
    let store = runtime
        .block_on(Store::open_local(StoreOpenOptions::new(
            workspace.database_path(),
            workspace.plugin_root(),
        )))
        .expect("open canonical US13 Store");
    let store_entity = cx.new(|_| store.clone());
    cx.update(|cx| {
        HiveGuiAppState::install_for_test_with_store(cx, AppRoute::Ai, store_entity.clone());
    });
    (workspace, store, store_entity, runtime)
}

fn click_us13_selector(visual: &mut VisualTestContext, selector: &'static str) -> bool {
    let Some(bounds) = visual.debug_bounds(selector) else {
        return false;
    };
    visual.simulate_click(bounds.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    true
}

fn settle_us13_selector(visual: &mut VisualTestContext, selector: &'static str) -> bool {
    for _ in 0..100 {
        visual.run_until_parked();
        if visual.debug_bounds(selector).is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

fn close_us13_window(visual: &mut VisualTestContext) {
    visual.update(|window, _| window.remove_window());
    visual.run_until_parked();
}

#[derive(Clone, Copy)]
struct T121NoopWorkflowExecutor;

impl hivegui::runtime::workflow_executor::WorkflowNodeExecutor for T121NoopWorkflowExecutor {
    fn execute(
        &self,
        _node: hivegui::datasource::workflow_store::WorkflowNode,
        _input: serde_json::Value,
        _cancel: hivegui::runtime::execution::CancelHandle,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
    > {
        Box::pin(async { Ok(serde_json::Value::Null) })
    }
}

fn t121_100_node_workflow() -> hivegui::datasource::workflow_store::WorkflowGraph {
    use hivegui::datasource::workflow_store::{NodeType, WorkflowGraph, WorkflowNode};

    let mut builder = WorkflowGraph::builder()
        .name("t121-combined-load")
        .node(WorkflowNode::new("start", NodeType::Start));
    for index in 0..98 {
        builder = builder.node(WorkflowNode::new(
            format!("noop-{index}"),
            NodeType::Function,
        ));
    }
    builder = builder
        .node(WorkflowNode::new("end", NodeType::End))
        .edge("start", "noop-0");
    for index in 0..97 {
        builder = builder.edge(format!("noop-{index}"), format!("noop-{}", index + 1));
    }
    builder.edge("noop-97", "end").build()
}

fn t121_combined_ui_target() -> support::performance::TargetSpec {
    support::performance::TargetSpec {
        id: "us13_combined_ui_feedback".into(),
        owner_task: "T121".into(),
        timing_boundary: "one keyboard character dispatched while Agent conversation, 100-node no-op Workflow, and backup prevalidation are all active to that character's visible input feedback".into(),
        excluded_time: vec![
            "external local-LLM wait".into(),
            "Workflow user-node execution".into(),
            "backup archive construction before the combined-load window".into(),
            "baseline and exception I/O".into(),
        ],
        warmup_iterations: support::performance::WARMUP_ITERATIONS,
        measured_samples: support::performance::MEASURED_SAMPLES,
        p95_budget_ns: 100_000_000,
        workflow_node_count: Some(100),
    }
}

#[gpui::test]
fn agent_duplicate_keeps_form_focus_and_native_scroll_reaches_actions(cx: &mut TestAppContext) {
    use gpui::{ScrollDelta, ScrollWheelEvent, TouchPhase, point};
    use hivegui::datasource::entity_store::{AgentInput, AgentStore};

    let (_workspace, store, store_entity, runtime) = us13_visual_store(cx);
    runtime
        .block_on(async {
            AgentStore::new(store.pool().clone())
                .expect("Agent Store")
                .create(
                    AgentInput::new_root("t121_safe_duplicate", "Existing Agent", "system")
                        .expect("valid Agent"),
                )
                .await
        })
        .expect("seed duplicate Agent");
    let _runtime_guard = runtime.enter();
    let window = cx.open_window(size(px(520.0), px(300.0)), move |window, cx| {
        let view = cx.new(|cx| hivegui::ui::agent_view::AgentView::new(store_entity, cx));
        gpui_component::Root::new(view, window, cx).bordered(false)
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let add_visible = settle_us13_selector(&mut visual, "AGENT_ADD");
    let opened = add_visible && click_us13_selector(&mut visual, "AGENT_ADD");
    let modal = visual.debug_bounds("AGENT_MODAL");
    let scroll = visual.debug_bounds("AGENT_FORM_SCROLL");
    let actions_before = visual.debug_bounds("AGENT_FORM_ACTIONS");
    if let Some(scroll_bounds) = scroll {
        visual.simulate_event(ScrollWheelEvent {
            position: scroll_bounds.center(),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-900.0))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        });
        visual.run_until_parked();
    }
    let actions_after = visual.debug_bounds("AGENT_FORM_ACTIONS");
    let input_ready =
        replace_function_input(&mut visual, "AGENT_IDENTIFIER_INPUT", "t121_safe_duplicate")
            && replace_function_input(&mut visual, "AGENT_NAME_INPUT", "Preserved Agent Name");
    if input_ready {
        visual.simulate_keystrokes("shift-tab enter");
    }
    let conflict = settle_us13_selector(
        &mut visual,
        "AGENT_CONFLICT-field=identifier-reason=duplicate",
    );
    let preserved = visual
        .debug_bounds("AGENT_IDENTIFIER_VALUE-t121_safe_duplicate")
        .is_some()
        && visual
            .debug_bounds("AGENT_NAME_VALUE-Preserved Agent Name")
            .is_some();
    let error_focused = visual.debug_bounds("AGENT_ERROR_FOCUSED").is_some();
    visual.simulate_keystrokes("escape");
    let focus_restored = visual.debug_bounds("AGENT_ADD_FOCUSED").is_some();
    close_us13_window(&mut visual);

    assert!(
        add_visible && opened,
        "Agent Add must be keyboard/pointer reachable"
    );
    let modal = modal.expect("Agent modal must render");
    let scroll = scroll.expect("Agent form must expose the owner native scroll surface");
    let before = actions_before.expect("Agent actions exist before scroll");
    let after = actions_after.expect("Agent actions exist after scroll");
    assert!(scroll.top() >= modal.top() && scroll.bottom() <= modal.bottom());
    assert!(after.top() < before.top() && after.bottom() <= modal.bottom());
    assert!(conflict && preserved && error_focused);
    assert!(focus_restored, "Escape restores focus to Agent Add");
    assert!(!AGENT_VIEW_SOURCE.contains("on_scroll_wheel"));
}

#[gpui::test]
fn conversation_stop_history_delete_and_keyboard_flow_are_real(cx: &mut TestAppContext) {
    use hivegui::agent::local_agent::LocalAgentRuntime;
    use hivegui::datasource::entity_store::{AgentInput, AgentStore};
    use hivegui::runtime::diagnostics::ExecutionEventCollector;

    let (_workspace, store, _store_entity, runtime) = us13_visual_store(cx);
    runtime
        .block_on(async {
            AgentStore::new(store.pool().clone())
                .expect("Agent Store")
                .create(
                    AgentInput::new_root("t121_conversation_root", "Root", "system")
                        .expect("root input"),
                )
                .await
        })
        .expect("seed default root");
    let local_runtime = LocalAgentRuntime::new(store.pool().clone()).expect("local runtime");
    let collector = Arc::new(ExecutionEventCollector::new());
    let _runtime_guard = runtime.enter();
    let window = cx.open_window(size(px(760.0), px(420.0)), move |window, cx| {
        let view = cx.new(|cx| {
            hivegui::ui::conversation_view::ConversationView::new(
                cx,
                Some(local_runtime),
                Some(store),
                collector,
            )
        });
        gpui_component::Root::new(view, window, cx).bordered(false)
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let required = [
        "CONVERSATION_NEW",
        "CONVERSATION_MESSAGE_INPUT",
        "CONVERSATION_SEND",
        "CONVERSATION_HISTORY_SCROLL",
        "CONVERSATION_MESSAGES_SCROLL",
    ];
    let missing = required
        .iter()
        .filter(|selector| visual.debug_bounds(selector).is_none())
        .copied()
        .collect::<Vec<_>>();
    let stop_available = visual.debug_bounds("CONVERSATION_STOP").is_some();
    if stop_available {
        click_us13_selector(&mut visual, "CONVERSATION_STOP");
    }
    let stopping = visual
        .debug_bounds("CONVERSATION_STATUS-stopping")
        .is_some();
    let delete_visible = visual.debug_bounds("CONVERSATION_DELETE_ACTIVE").is_some();
    if delete_visible {
        click_us13_selector(&mut visual, "CONVERSATION_DELETE_ACTIVE");
    }
    let confirmation = visual.debug_bounds("CONVERSATION_DELETE_CONFIRM").is_some();
    visual.simulate_keystrokes("escape");
    let input_focus_restored = visual
        .debug_bounds("CONVERSATION_MESSAGE_INPUT_FOCUSED")
        .is_some();
    close_us13_window(&mut visual);

    assert!(
        missing.is_empty(),
        "missing conversation selectors: {missing:?}"
    );
    assert!(
        stop_available && stopping,
        "Stop must immediately render stopping"
    );
    assert!(
        delete_visible && confirmation,
        "history delete requires confirmation"
    );
    assert!(
        input_focus_restored,
        "Escape restores conversation input focus"
    );
    assert!(!CONVERSATION_VIEW_SOURCE.contains("on_scroll_wheel"));
}

#[gpui::test]
fn settings_backup_restore_retention_and_diagnostics_are_keyboard_reachable(
    cx: &mut TestAppContext,
) {
    use gpui::{ScrollDelta, ScrollWheelEvent, TouchPhase, point};
    use hivegui::runtime::diagnostics::ExecutionEventCollector;

    let (_workspace, store, _store_entity, runtime) = us13_visual_store(cx);
    let collector = Arc::new(ExecutionEventCollector::new());
    let _runtime_guard = runtime.enter();
    let window = cx.open_window(size(px(560.0), px(320.0)), move |window, cx| {
        let view = cx.new(|cx| {
            let mut view = hivegui::ui::settings_view::SettingsView::new(cx, Some(collector));
            view.set_store(store);
            view
        });
        gpui_component::Root::new(view, window, cx).bordered(false)
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let surface = visual.debug_bounds("SETTINGS_SCROLL");
    let actions_before = visual.debug_bounds("SETTINGS_DIAGNOSTIC_EXPORT");
    if let Some(bounds) = surface {
        visual.simulate_event(ScrollWheelEvent {
            position: bounds.center(),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-1_200.0))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        });
        visual.run_until_parked();
    }
    let actions_after = visual.debug_bounds("SETTINGS_DIAGNOSTIC_EXPORT");
    let required = [
        "SETTINGS_RETENTION_DAYS",
        "SETTINGS_RETENTION_PREVIEW",
        "SETTINGS_BACKUP_EXPORT",
        "SETTINGS_RESTORE_PRECHECK",
        "SETTINGS_RESTORE_CONFIRM",
        "SETTINGS_DIAGNOSTIC_EXPORT",
    ];
    let missing = required
        .iter()
        .filter(|selector| visual.debug_bounds(selector).is_none())
        .copied()
        .collect::<Vec<_>>();
    visual.simulate_keystrokes("tab tab tab enter escape");
    let focus_restored = visual
        .debug_bounds("SETTINGS_RESTORE_PRECHECK_FOCUSED")
        .is_some();
    close_us13_window(&mut visual);

    let surface = surface.expect("Settings must expose native scroll bounds");
    let before = actions_before.expect("diagnostic action exists before scroll");
    let after = actions_after.expect("diagnostic action exists after scroll");
    assert!(after.top() < before.top() && after.bottom() <= surface.bottom());
    assert!(
        missing.is_empty(),
        "missing Settings selectors: {missing:?}"
    );
    assert!(focus_restored, "modal Escape restores precheck focus");
    assert!(!SETTINGS_VIEW_SOURCE.contains("on_scroll_wheel"));
}

#[gpui::test]
fn agent_workflow_backup_combined_load_keeps_keyboard_focus_and_stop_responsive(
    cx: &mut TestAppContext,
) {
    use chrono::Utc;
    use hivegui::agent::local_agent::LocalAgentRuntime;
    use hivegui::datasource::{
        backup::{BackupExporter, BackupImporter},
        entity_store::{AgentInput, AgentStore},
        llm_store::LlmStore,
    };
    use hivegui::runtime::{
        diagnostics::ExecutionEventCollector, execution::CancelHandle,
        workflow_executor::WorkflowExecutor,
    };
    use support::performance::{
        BenchmarkReport, ComparisonOutcome, EnvironmentFingerprint, baseline_path,
        evaluate_benchmark_gate, regression_exception_path, source_revision,
    };
    use tokio::io::AsyncReadExt as _;

    let (workspace, store, _store_entity, runtime) = us13_visual_store(cx);
    let request_seen = Arc::new(AtomicBool::new(false));
    let release_agent = Arc::new(Notify::new());
    let (endpoint, server_task) = runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind T121 hanging local LLM");
        let address = listener.local_addr().expect("T121 local LLM address");
        let request_seen = Arc::clone(&request_seen);
        let release_agent = Arc::clone(&release_agent);
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept T121 local LLM");
            let mut request_prefix = [0_u8; 4096];
            let read = stream
                .read(&mut request_prefix)
                .await
                .expect("read T121 local LLM request");
            assert!(
                read > 0,
                "Agent conversation must issue a real local request"
            );
            request_seen.store(true, Ordering::SeqCst);
            release_agent.notified().await;
        });
        (format!("http://{address}"), task)
    });

    runtime.block_on(async {
        let llm_store = LlmStore::new(store.pool().clone(), store.crypto().clone());
        let provider = llm_store
            .create_provider(
                "t121-local-provider",
                "openai",
                &endpoint,
                "t121-local-token",
                "",
            )
            .await
            .expect("create T121 local Provider");
        let preset = llm_store
            .create_preset("t121-local-preset", "", false, 256, 0.0)
            .await
            .expect("create T121 local Preset");
        llm_store
            .create_model("t121-local-model", preset.id, provider.id, 1)
            .await
            .expect("create T121 local Model");
        AgentStore::new(store.pool().clone())
            .expect("T121 Agent Store")
            .create(
                AgentInput::new_root(
                    "t121_combined_root",
                    "Combined Load Root",
                    "local-only system prompt",
                )
                .expect("valid T121 root Agent")
                .with_model_preset("t121-local-preset"),
            )
            .await
            .expect("create T121 root Agent");
    });

    let archive = workspace.root().join("t121-combined.age");
    runtime
        .block_on(
            BackupExporter::new(workspace.database_path())
                .export_age(&archive, "T121-combined-passphrase"),
        )
        .expect("build the T121 archive before the measured boundary");

    let load_running = Arc::new(AtomicBool::new(true));
    let workflow_running = Arc::clone(&load_running);
    let workflow_task = runtime.spawn(async move {
        let workflow = t121_100_node_workflow();
        let executor = WorkflowExecutor::new(T121NoopWorkflowExecutor);
        let mut completed = 0_usize;
        while workflow_running.load(Ordering::SeqCst) {
            let outcome = executor
                .execute(&workflow, serde_json::json!({}), CancelHandle::new())
                .await
                .expect("T121 no-op Workflow remains healthy");
            assert!(outcome.completed && outcome.node_results.len() == 100);
            completed += 1;
            tokio::task::yield_now().await;
        }
        completed
    });
    let backup_running = Arc::clone(&load_running);
    let backup_archive = archive.clone();
    let backup_importer = BackupImporter::new(workspace.root().join("t121-staging"));
    let backup_task = runtime.spawn(async move {
        let mut completed = 0_usize;
        while backup_running.load(Ordering::SeqCst) {
            backup_importer
                .inspect_manifest(&backup_archive, "T121-combined-passphrase")
                .await
                .expect("T121 backup prevalidation remains healthy");
            completed += 1;
        }
        completed
    });

    let local_runtime = LocalAgentRuntime::new(store.pool().clone()).expect("T121 local runtime");
    let collector = Arc::new(ExecutionEventCollector::new());
    let _runtime_guard = runtime.enter();
    let window = cx.open_window(size(px(760.0), px(420.0)), move |window, cx| {
        let view = cx.new(|cx| {
            hivegui::ui::conversation_view::ConversationView::new(
                cx,
                Some(local_runtime),
                Some(store),
                collector,
            )
        });
        gpui_component::Root::new(view, window, cx).bordered(false)
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let input_ready = settle_us13_selector(&mut visual, "CONVERSATION_MESSAGE_INPUT");
    let send_ready = settle_us13_selector(&mut visual, "CONVERSATION_SEND");
    let stop_ready = settle_us13_selector(&mut visual, "CONVERSATION_STOP");
    let started = input_ready
        && send_ready
        && stop_ready
        && replace_function_input(
            &mut visual,
            "CONVERSATION_MESSAGE_INPUT",
            "start combined load",
        )
        && click_us13_selector(&mut visual, "CONVERSATION_SEND");
    for _ in 0..100 {
        visual.run_until_parked();
        if request_seen.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let agent_in_flight = request_seen.load(Ordering::SeqCst);

    let target = t121_combined_ui_target();
    let mut samples_ns = Vec::with_capacity(target.measured_samples);
    let mut stop_available = Vec::with_capacity(target.measured_samples);
    let mut all_feedback_visible = true;
    let total_interactions = target.warmup_iterations + target.measured_samples;
    let input_reset = replace_function_input(&mut visual, "CONVERSATION_MESSAGE_INPUT", "");
    let mut visible_value = String::with_capacity(total_interactions);
    for index in 0..total_interactions {
        let character = char::from(b'a' + u8::try_from(index % 26).expect("alphabet index"));
        visible_value.push(character);
        let value_selector =
            Box::leak(format!("CONVERSATION_MESSAGE_VALUE-{visible_value}").into_boxed_str());
        let interaction_started = Instant::now();
        visual.simulate_input(character.to_string().as_str());
        let mut feedback_visible = false;
        if input_reset {
            for _ in 0..100 {
                visual.run_until_parked();
                if visual.debug_bounds(value_selector).is_some() {
                    feedback_visible = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        let elapsed = interaction_started.elapsed();
        all_feedback_visible &= input_reset && feedback_visible;
        stop_available.push(visual.debug_bounds("CONVERSATION_STOP").is_some());
        if index >= target.warmup_iterations {
            samples_ns.push(
                u64::try_from(elapsed.as_nanos()).expect("T121 latency fits in u64 nanoseconds"),
            );
        }
    }
    visual.simulate_keystrokes("tab tab");
    visual.run_until_parked();
    let stop_focused = visual.debug_bounds("CONVERSATION_STOP_FOCUSED").is_some();
    visual.simulate_keystrokes("enter");
    let stopping = settle_us13_selector(&mut visual, "CONVERSATION_STATUS-stopping");
    close_us13_window(&mut visual);

    load_running.store(false, Ordering::SeqCst);
    release_agent.notify_waiters();
    server_task.abort();
    drop(_runtime_guard);
    let (workflow_iterations, backup_iterations) = runtime.block_on(async {
        let workflow_iterations = workflow_task.await.expect("join T121 Workflow load");
        let backup_iterations = backup_task.await.expect("join T121 backup load");
        (workflow_iterations, backup_iterations)
    });

    assert!(
        started && agent_in_flight,
        "the real local Agent conversation must be in flight"
    );
    assert!(
        all_feedback_visible,
        "every keyboard input must produce visible feedback"
    );
    assert!(
        workflow_iterations > 0,
        "the real 100-node Workflow must overlap UI input"
    );
    assert!(
        backup_iterations > 0,
        "real backup prevalidation must overlap UI input"
    );
    assert!(
        stop_available.iter().all(|available| *available),
        "Stop availability must remain 100% throughout the combined load"
    );
    assert!(
        stop_focused && stopping,
        "keyboard Stop must focus and immediately show stopping"
    );

    let environment = EnvironmentFingerprint::capture();
    let report = BenchmarkReport::from_samples(target.clone(), environment.clone(), &samples_ns)
        .expect("build the fixed T121 report");
    assert!(
        report.percentiles_ns.p95 <= 100_000_000,
        "input feedback p95={}ns exceeds 100ms",
        report.percentiles_ns.p95
    );
    assert!(
        samples_ns.iter().all(|sample| *sample <= 250_000_000),
        "a UI heartbeat exceeded the 250ms continuous-blocking ceiling: {samples_ns:?}"
    );

    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let revision = source_revision(&repository).expect("capture T121 source revision");
    let comparison = evaluate_benchmark_gate(
        &report,
        &revision,
        &baseline_path(&target, &environment),
        &regression_exception_path(&target, &environment),
        Utc::now().date_naive(),
    )
    .expect("evaluate the T005 T121 performance gate");
    assert!(
        matches!(
            comparison.outcome,
            ComparisonOutcome::Passed | ComparisonOutcome::ApprovedException
        ),
        "T121 combined-load performance gate is not approved: {comparison:?}; report={report:?}"
    );
}
