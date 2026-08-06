//! T-AUTH-2 Red contract for the local master-password unlock flow
//! (FR-049 + SC-033).
//!
//! These tests intentionally target the public `auth` API planned by T-AUTH-5
//! (`hivegui::auth::{keystore, lock, policy, ui}`). They must remain
//! compile-Red until that module exists; do not replace them with fixture
//! or private validation tests.
//!
//! Coverage:
//! - FR-049: `wrapped_device_key.v1` 存在时应用必须先显示解锁界面；
//!   密码字段无回显、不可被剪贴板复制。
//! - SC-033: 正确密码解包 KEK 与设备密钥成功并进入主 UI；连续 5 次错误
//!   后整应用 5 分钟 backoff，UI 仅显示"从备份恢复" 入口、不得再进入
//!   解锁界面（注入时钟验证）；错误密码不得修改、删除或重新生成设备
//!   密钥或 `datasources.db` 任何行；解锁后内存中已派生 KEK 与设备密钥
//!   材料在再次锁定时经 `zeroize`+编译器屏障清零。
//! - Constitution II.3 (TDD): 本文件必须先观察到 Red，再批准 T-AUTH-5
//!   实现。
//!
//! Owner phase: `Foundation-auth-Red`. Reviewer: 未指派前不得进入
//! T-AUTH-5。

#![cfg(test)]

#[path = "support/mod.rs"]
mod support;

use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, time::Duration};

use hivegui::auth::{
    keystore::{AuthError, AuthKeystore, UnlockOutcome, UnlockedKeystore},
    lock::{AuthLockReason, AuthLockState},
    policy::PasswordPolicy,
    ui::{AuthUnlockView, PasswordFieldEcho, RecoveryPath, UnlockScreen},
};
use support::TestWorkspace;

const STRONG_PASSWORD: &str = "CorrectHorseBattery-2026!Staple";
const WRONG_PASSWORD: &str = "WrongPasswordNotStrong!";
const KEYSTORE_RELATIVE: &str = "keystore/wrapped_device_key.v1";
const DATASTORE_RELATIVE: &str = "datasources.db";
const T025_DEVICE_KEY_RELATIVE: &str = "datasource/key_store.bin";

fn new_workspace() -> TestWorkspace {
    TestWorkspace::new().expect("allocate isolated test workspace")
}

/// 构造已设置过主密码的 workspace：调用 T-AUTH-5 的 `submit_setup`
/// 公共 API（虽然尚未实现，但被本 Red 合同作为前置 fixture 引入）。
fn seeded_workspace() -> TestWorkspace {
    let workspace = new_workspace();
    let view = AuthUnlockView::open(workspace.root())
        .expect("first launch must enter the unlock view before any wrap file exists");
    let proposal = hivegui::auth::keystore::PasswordProposal::new(STRONG_PASSWORD, STRONG_PASSWORD)
        .expect("strong password must survive construction");
    match view
        .submit_setup(proposal)
        .expect("strong password must be accepted in the setup flow")
    {
        hivegui::auth::keystore::AuthOutcome::SetupCompleted { .. } => workspace,
        other => panic!("expected SetupCompleted, got {other:?}"),
    }
}

fn current_screen(view: &AuthUnlockView) -> UnlockScreen {
    view.current_screen()
        .expect("unlock view must present a screen")
}

#[test]
fn wrapped_keystore_present_shows_unlock_screen_first() {
    let workspace = seeded_workspace();
    let path = workspace.root().join(KEYSTORE_RELATIVE);
    assert!(
        path.exists(),
        "fixture invariant: seeded workspace must contain {KEYSTORE_RELATIVE}"
    );

    let view = AuthUnlockView::open(workspace.root())
        .expect("opening a workspace that already wrapped its key must enter the unlock view");

    match current_screen(&view) {
        UnlockScreen::EnterMasterPassword { .. } => {}
        other => panic!("expected EnterMasterPassword, got {other:?}"),
    }

    let lock_state: AuthLockState = view.lock_state();
    assert_eq!(
        lock_state.reason(),
        AuthLockReason::Startup,
        "lock state on a sealed workspace must report Startup"
    );
    assert!(
        !view.primary_store_open(),
        "primary Store must not open while the wrapped keystore is sealed"
    );
}

#[test]
fn password_field_echo_is_blank_and_blocks_clipboard() {
    let workspace = seeded_workspace();
    let view = AuthUnlockView::open(workspace.root()).expect("unlock view");
    let field = view.password_field();
    assert_eq!(
        field.echo(),
        PasswordFieldEcho::Blank,
        "password field must not echo any glyph while typing"
    );
    let clipboard = view.attempt_copy_password_field_to_clipboard();
    assert!(
        clipboard.is_err(),
        "password field must reject any clipboard read; got {clipboard:?}"
    );
    if let Err(err) = clipboard {
        assert!(
            matches!(err, AuthError::PasswordFieldHiddenFromClipboard),
            "clipboard rejection must collapse to PasswordFieldHiddenFromClipboard: {err:?}"
        );
    }
}

#[test]
fn correct_password_unwraps_keystore_and_enters_main_ui() {
    let workspace = seeded_workspace();
    let view = AuthUnlockView::open(workspace.root()).expect("unlock view");
    let outcome: UnlockOutcome = view
        .submit_password(STRONG_PASSWORD)
        .expect("correct password must unlock the wrapped keystore");

    match outcome {
        UnlockOutcome::Unlocked(UnlockedKeystore { .. }) => {
            assert!(
                view.primary_store_open(),
                "primary Store must be open after a successful unlock"
            );
            match current_screen(&view) {
                UnlockScreen::MainUi { .. } => {}
                other => panic!("expected MainUi after unlock, got {other:?}"),
            }
        }
        other => panic!("expected Unlocked, got {other:?}"),
    }
}

#[test]
fn five_wrong_passwords_lock_for_five_minutes_with_recovery_entry() {
    let workspace = seeded_workspace();
    let view = AuthUnlockView::open(workspace.root()).expect("unlock view");

    for attempt in 1..=5 {
        let result = view.submit_password(WRONG_PASSWORD);
        assert!(
            result.is_err(),
            "wrong password attempt #{attempt} must be rejected"
        );
        if let Err(err) = &result {
            assert!(
                matches!(err, AuthError::InvalidPassword),
                "wrong password must collapse to InvalidPassword (no side channel): {err:?}"
            );
        }
    }

    // 第 5 次错误之后，必须进入 5 分钟 backoff。
    assert_eq!(
        view.lock_state().reason(),
        AuthLockReason::TooManyAttempts,
        "5 wrong attempts must transition lock state to TooManyAttempts"
    );
    let backoff = view.lock_state().backoff_remaining();
    assert!(
        backoff >= Duration::from_secs(4 * 60),
        "backoff must be at least 5 minutes - 1 elapsed minute: {backoff:?}"
    );
    assert!(
        backoff <= Duration::from_secs(5 * 60 + 1),
        "backoff must not exceed 5 minutes: {backoff:?}"
    );

    // 在 backoff 期间必须无法再进入 EnterMasterPassword 屏幕。
    match current_screen(&view) {
        UnlockScreen::RecoveryOnly {
            entry: RecoveryPath::RestoreFromBackup,
        } => {}
        other => panic!("expected RecoveryOnly with RestoreFromBackup, got {other:?}"),
    }

    // 注入式时钟把 backoff 推过 5 分钟；之后必须能再次进入 EnterMasterPassword。
    view.advance_clock_for_test(Duration::from_secs(5 * 60 + 1));
    match current_screen(&view) {
        UnlockScreen::EnterMasterPassword { .. } => {}
        other => panic!("after backoff elapses the unlock screen must return, got {other:?}"),
    }
}

#[test]
fn wrong_passwords_must_not_modify_any_persistent_state() {
    let workspace = seeded_workspace();
    let wrapped_path = workspace.root().join(KEYSTORE_RELATIVE);
    let datastore_path = workspace.root().join(DATASTORE_RELATIVE);
    let t025_path = workspace.root().join(T025_DEVICE_KEY_RELATIVE);

    let wrapped_before = fs::read(&wrapped_path).expect("read wrapped_device_key");
    let wrapped_mtime_before = fs::metadata(&wrapped_path)
        .and_then(|m| m.modified())
        .expect("mtime wrapped_device_key");
    let wrapped_mode_before = fs::metadata(&wrapped_path)
        .expect("stat wrapped_device_key")
        .permissions()
        .mode()
        & 0o777;

    let datastore_before = fs::read(&datastore_path).ok();
    let t025_before = fs::read(&t025_path).ok();

    let view = AuthUnlockView::open(workspace.root()).expect("unlock view");
    for _ in 0..3 {
        let _ = view.submit_password(WRONG_PASSWORD);
    }

    let wrapped_after = fs::read(&wrapped_path).expect("read wrapped_device_key");
    let wrapped_mtime_after = fs::metadata(&wrapped_path)
        .and_then(|m| m.modified())
        .expect("mtime wrapped_device_key");
    let wrapped_mode_after = fs::metadata(&wrapped_path)
        .expect("stat wrapped_device_key")
        .permissions()
        .mode()
        & 0o777;

    assert_eq!(
        wrapped_before, wrapped_after,
        "wrong passwords must not rewrite the wrapped_device_key bytes"
    );
    assert_eq!(
        wrapped_mtime_before, wrapped_mtime_after,
        "wrong passwords must not touch wrapped_device_key mtime"
    );
    assert_eq!(
        wrapped_mode_before, wrapped_mode_after,
        "wrong passwords must not relax wrapped_device_key permissions"
    );

    let datastore_after = fs::read(&datastore_path).ok();
    assert_eq!(
        datastore_before, datastore_after,
        "wrong passwords must not modify datasources.db"
    );
    let t025_after = fs::read(&t025_path).ok();
    assert_eq!(
        t025_before, t025_after,
        "wrong passwords must not modify the T025 device-only key"
    );
}

#[test]
fn successful_unlock_exposes_decrypted_device_key_bytes() {
    let workspace = seeded_workspace();
    let view = AuthUnlockView::open(workspace.root()).expect("unlock view");
    let outcome: UnlockOutcome = view
        .submit_password(STRONG_PASSWORD)
        .expect("correct password must unlock");
    let UnlockedKeystore { device_key, kek } = match outcome {
        UnlockOutcome::Unlocked(unlocked) => unlocked,
        other => panic!("expected Unlocked, got {other:?}"),
    };
    assert_eq!(
        device_key.as_bytes().len(),
        32,
        "device key must be 32 bytes"
    );
    assert_eq!(kek.as_bytes().len(), 32, "KEK must be 32 bytes");
    assert_ne!(
        device_key.as_bytes(),
        kek.as_bytes(),
        "device key and KEK must not be the same 32-byte secret"
    );
}

#[test]
fn relock_zeros_decrypted_secrets_from_memory() {
    let workspace = seeded_workspace();
    let view = AuthUnlockView::open(workspace.root()).expect("unlock view");
    let _ = view
        .submit_password(STRONG_PASSWORD)
        .expect("unlock with correct password");

    // 触发 relock（idle 15min 或显式 lock_now）；T-AUTH-5 必须把内存中
    // 的 KEK + 设备密钥材料经 `zeroize`+编译器屏障清零。
    view.lock_now_for_test(AuthLockReason::UserRequested);

    let state = view.lock_state();
    assert_eq!(
        state.reason(),
        AuthLockReason::UserRequested,
        "lock_now must transition state to UserRequested"
    );
    assert!(
        !view.primary_store_open(),
        "primary Store must close after lock_now"
    );
    assert!(
        state.unlocked_secrets_zeroed(),
        "KEK + device key must be zeroized in memory after lock_now"
    );
}

#[test]
fn reload_after_lock_re_presents_unlock_screen() {
    let workspace = seeded_workspace();
    let view = AuthUnlockView::open(workspace.root()).expect("unlock view");
    let _ = view
        .submit_password(STRONG_PASSWORD)
        .expect("unlock with correct password");
    view.lock_now_for_test(AuthLockReason::UserRequested);
    let _ = view.close();
    let view = AuthUnlockView::open(workspace.root()).expect("reopen after lock");
    match current_screen(&view) {
        UnlockScreen::EnterMasterPassword { .. } => {}
        other => panic!("expected EnterMasterPassword after re-open, got {other:?}"),
    }
    assert_eq!(
        view.lock_state().reason(),
        AuthLockReason::Startup,
        "fresh open after lock must reset reason to Startup"
    );
}
