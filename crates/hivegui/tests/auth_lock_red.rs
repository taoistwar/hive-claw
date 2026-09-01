//! T-AUTH-3 Red contract for the auto-lock + OS screen-lock-event flow
//! (FR-050 + SC-034).
//!
//! These tests intentionally target the public `auth` API planned by T-AUTH-5
//! (`hivegui::auth::{keystore, lock, policy, ui}`). They must remain
//! compile-Red until that module exists; do not replace them with fixture
//! or private validation tests.
//!
//! Coverage:
//! - FR-050: 默认 15 分钟空闲（keypress / 主窗口 mousedown / 焦点变化
//!   重置，mousemove 不重置）自动锁定；`auto_lock_minutes` 范围
//!   `1..=1440`；越界值返回 `invalid_input`。
//! - SC-034: 内存中已派生设备密钥材料与未持久化敏感明文经
//!   `zeroize`+编译器屏障清零；UI 回到解锁界面；
//!   `AgentExecution.status=cancelled`、`ChatSession.status=locked`；
//!   OS 屏幕锁事件（Linux `zbus` `ScreenSaver ActiveChanged`、
//!   macOS `NSWorkspaceScreensDidSleep`、Windows
//!   `WM_WTSSESSION_CHANGE=0x7`）立即触发同样行为；事件被禁用 / DBus
//!   不可用 / 通知 API 不可用时 **fail-closed = 整应用立即锁定 + 提示
//!   "无法验证屏幕锁事件" + 阻断主 UI**（非静默忽略、也非"禁用自动
//!   锁定"）；锁定后 SQLite 主文件、WAL/SHM、备份 staging、诊断包明文
//!   canary 扫描命中数必须为 0。
//! - Constitution II.3 (TDD): 本文件必须先观察到 Red，再批准 T-AUTH-5
//!   实现。
//!
//! Owner phase: `Foundation-auth-Red`. Reviewer: 未指派前不得进入
//! T-AUTH-5。

#![cfg(test)]

#[path = "support/mod.rs"]
mod support;

use std::time::Duration;

use hivegui::auth::{
    config::LockErrorMode,
    keystore::{AuthError, UnlockedKeystore},
    lock::{
        AuthLockReason, DisabledMonitor, IdleActivity, OsScreenLockEvent, ScreenLockMonitorError,
    },
    monitor::{AgentExecutionStatus, ChatSessionStatus, SensitiveCanaryScanner, SensitiveFileKind},
    ui::AuthUnlockView,
};
use support::TestWorkspace;

const STRONG_PASSWORD: &str = "CorrectHorseBattery-2026!Staple";

fn new_workspace() -> TestWorkspace {
    TestWorkspace::new().expect("allocate isolated test workspace")
}

fn unlocked_view(workspace: &TestWorkspace) -> AuthUnlockView {
    let view = AuthUnlockView::open(workspace.root()).expect("unlock view");
    // T-AUTH-5: 在已存在的 wrapped_device_key.v1 上解锁；如果不存在则
    // 先建立 keystore，让所有 T-AUTH-3 测试共享同一 fixture。
    let proposal = hivegui::auth::keystore::PasswordProposal::new(STRONG_PASSWORD, STRONG_PASSWORD)
        .expect("strong password must survive construction");
    let _ = view
        .submit_setup(proposal)
        .expect("setup must accept the seeded password");
    let _ = view
        .submit_password(STRONG_PASSWORD)
        .expect("unlock with the seeded password");
    view
}

fn now() -> std::time::Instant {
    std::time::Instant::now()
}

#[test]
fn default_auto_lock_minutes_is_fifteen() {
    let view = unlocked_view(&new_workspace());
    let cfg = view.auto_lock_config();
    assert_eq!(cfg.minutes(), 15u16, "default auto_lock_minutes must be 15");
    assert_eq!(cfg.minutes(), 15);
}

#[test]
fn auto_lock_minutes_out_of_range_returns_invalid_input() {
    let view = unlocked_view(&new_workspace());
    let result = view.set_auto_lock_minutes_for_test(0);
    assert!(
        matches!(
            result,
            Err(AuthError::InvalidInput {
                field: "auth.auto_lock_minutes",
                reason: "out_of_range",
            })
        ),
        "auto_lock_minutes=0 must return invalid_input; got {result:?}"
    );
    let result = view.set_auto_lock_minutes_for_test(1441);
    assert!(
        matches!(
            result,
            Err(AuthError::InvalidInput {
                field: "auth.auto_lock_minutes",
                reason: "out_of_range",
            })
        ),
        "auto_lock_minutes=1441 must return invalid_input; got {result:?}"
    );
    // 主 UI 未解锁前 auth.auto_lock_minutes 字段不得出现。
    let locked_view = AuthUnlockView::open(new_workspace().root()).expect("locked view");
    assert!(
        locked_view
            .config_field_visibility("auth.auto_lock_minutes")
            .is_none(),
        "auth.auto_lock_minutes must not be visible before unlock"
    );
}

#[test]
fn idle_15_minutes_triggers_lock_without_mousemove_reset() {
    let workspace = new_workspace();
    let view = unlocked_view(&workspace);
    let clock = view.idle_clock_for_test();
    clock.advance(Duration::from_secs(14 * 60));
    assert_eq!(
        view.lock_state().reason(),
        AuthLockReason::None,
        "before the 15-min mark the app must still be unlocked"
    );
    // mousemove 不能重置 idle 计时。
    view.record_idle_for_test(IdleActivity::MouseMove);
    clock.advance(Duration::from_secs(2 * 60));
    assert_eq!(
        view.lock_state().reason(),
        AuthLockReason::IdleTimeout,
        "mousemove must not reset the idle clock; idle >= 15min must trigger lock"
    );
    assert!(
        !view.primary_store_open(),
        "primary Store must close on idle lock"
    );
}

#[test]
fn keypress_resets_idle_clock() {
    let view = unlocked_view(&new_workspace());
    let clock = view.idle_clock_for_test();
    clock.advance(Duration::from_secs(14 * 60));
    view.record_idle_for_test(IdleActivity::KeyPress);
    clock.advance(Duration::from_secs(14 * 60));
    assert_eq!(
        view.lock_state().reason(),
        AuthLockReason::None,
        "keypress must reset the idle clock"
    );
    clock.advance(Duration::from_secs(2 * 60));
    assert_eq!(
        view.lock_state().reason(),
        AuthLockReason::IdleTimeout,
        "after 16min since the last keypress, idle lock must fire"
    );
}

#[test]
fn main_window_mousedown_and_focus_change_reset_idle_clock() {
    let view = unlocked_view(&new_workspace());
    let clock = view.idle_clock_for_test();
    clock.advance(Duration::from_secs(14 * 60));
    view.record_idle_for_test(IdleActivity::MainWindowMouseDown);
    clock.advance(Duration::from_secs(14 * 60));
    view.record_idle_for_test(IdleActivity::FocusChange);
    clock.advance(Duration::from_secs(14 * 60));
    assert_eq!(
        view.lock_state().reason(),
        AuthLockReason::None,
        "main window mousedown and focus change must each reset idle"
    );
}

#[test]
fn auto_lock_cancels_running_agent_execution_and_locks_chat_session() {
    let workspace = new_workspace();
    let view = unlocked_view(&workspace);
    let execution = view.start_agent_execution_for_test("test");
    let chat = view.start_chat_session_for_test("session-1");
    assert_eq!(execution.status(), AgentExecutionStatus::Running);
    assert_eq!(chat.status(), ChatSessionStatus::Active);

    let clock = view.idle_clock_for_test();
    clock.advance(Duration::from_secs(15 * 60 + 1));
    assert_eq!(
        view.lock_state().reason(),
        AuthLockReason::IdleTimeout,
        "idle >= 15min must fire the lock"
    );
    assert_eq!(
        execution.status(),
        AgentExecutionStatus::Cancelled,
        "idle lock must cancel the running AgentExecution"
    );
    assert_eq!(
        chat.status(),
        ChatSessionStatus::Locked,
        "idle lock must transition the ChatSession to Locked"
    );
}

#[test]
fn os_screen_lock_event_linux_triggers_immediate_lock() {
    let view = unlocked_view(&new_workspace());
    view.emit_os_screen_lock_event_for_test(OsScreenLockEvent::LinuxScreenSaverActiveChanged);
    assert_eq!(
        view.lock_state().reason(),
        AuthLockReason::OsScreenLock,
        "Linux ScreenSaver ActiveChanged must transition lock reason to OsScreenLock"
    );
    assert!(
        !view.primary_store_open(),
        "primary Store must close on OS screen lock"
    );
}

#[test]
fn os_screen_lock_event_macos_triggers_immediate_lock() {
    let view = unlocked_view(&new_workspace());
    view.emit_os_screen_lock_event_for_test(OsScreenLockEvent::MacOsScreensDidSleep);
    assert_eq!(view.lock_state().reason(), AuthLockReason::OsScreenLock);
}

#[test]
fn os_screen_lock_event_windows_triggers_immediate_lock() {
    let view = unlocked_view(&new_workspace());
    view.emit_os_screen_lock_event_for_test(OsScreenLockEvent::WindowsWtSessionChange);
    assert_eq!(view.lock_state().reason(), AuthLockReason::OsScreenLock);
}

#[test]
fn screen_lock_monitor_disabled_fails_closed_globally() {
    let view = unlocked_view(&new_workspace());
    // 注入：ScreenLockMonitor 报告自身 disabled（zbus 不可用、NSWorkspace 不可用、
    // WTS 不可用等所有情形都收敛到 disabled）。
    let monitor = DisabledMonitor::disabled_for_test(LockErrorMode::FailClosed);
    view.attach_screen_lock_monitor_for_test(monitor);
    match view.startup_screen_lock_check() {
        Ok(_) => panic!("disabled monitor must not silently allow auto-lock to run"),
        Err(ScreenLockMonitorError::Unavailable {
            mode: LockErrorMode::FailClosed,
        }) => {}
        Err(other) => panic!("fail-closed monitor must report Unavailable: {other:?}"),
    }
    // fail-closed = 整应用立即锁定。
    assert_eq!(
        view.lock_state().reason(),
        AuthLockReason::OsScreenLockUnavailable,
        "OS screen lock monitor disabled/DBus unavailable/API unavailable must fail-closed"
    );
    assert!(
        !view.primary_store_open(),
        "fail-closed must close the primary Store"
    );
    let banner = view.active_banner();
    assert_eq!(
        banner.message(),
        "无法验证屏幕锁事件：应用已进入锁定状态",
        "fail-closed must surface the exact user-visible banner"
    );
    assert!(
        banner.blocks_main_ui(),
        "fail-closed banner must block the main UI; never silent, never disable auto-lock"
    );
}

#[test]
fn locked_state_yields_zero_plaintext_canary_hits_across_persistent_storage() {
    let workspace = new_workspace();
    let view = unlocked_view(&workspace);
    // 在内存中临时塞入一段明文 canary，模拟"未持久化敏感明文" 必须 zeroize。
    view.inject_unpersisted_canary_for_test("HIVEGUI_CANARY_LOCAL_AUTH=plaintext");

    let clock = view.idle_clock_for_test();
    clock.advance(Duration::from_secs(15 * 60 + 1));
    assert_eq!(
        view.lock_state().reason(),
        AuthLockReason::IdleTimeout,
        "idle >= 15min must fire the lock"
    );
    let unlocked: UnlockedKeystore = view
        .unlocked_keystore_snapshot()
        .expect("snapshot before lock must be available");
    drop(unlocked);

    let scanner = SensitiveCanaryScanner::new(workspace.root());
    for kind in [
        SensitiveFileKind::SqliteMain,
        SensitiveFileKind::SqliteWal,
        SensitiveFileKind::SqliteShm,
        SensitiveFileKind::BackupStaging,
        SensitiveFileKind::DiagnosticsBundle,
    ] {
        let hits = scanner.scan_known_canary(kind, "HIVEGUI_CANARY_LOCAL_AUTH=plaintext");
        assert_eq!(
            hits, 0,
            "after idle lock, {kind:?} must contain zero plaintext canary hits"
        );
    }
    // 内存 zeroize 通过公共 API 断言（不依赖私有字段）。
    assert!(
        view.lock_state().unlocked_secrets_zeroed(),
        "idle lock must zeroize the KEK + device key + canary in memory"
    );
}

#[test]
fn idle_minutes_change_takes_effect_without_relogin() {
    let view = unlocked_view(&new_workspace());
    view.set_auto_lock_minutes_for_test(30)
        .expect("30 is within 1..=1440");
    let clock = view.idle_clock_for_test();
    clock.advance(Duration::from_secs(15 * 60 + 1));
    assert_eq!(
        view.lock_state().reason(),
        AuthLockReason::None,
        "after raising auto_lock_minutes to 30, 15min idle must not lock"
    );
    clock.advance(Duration::from_secs(15 * 60));
    assert_eq!(
        view.lock_state().reason(),
        AuthLockReason::IdleTimeout,
        "30min after the last activity, lock must fire"
    );
}
