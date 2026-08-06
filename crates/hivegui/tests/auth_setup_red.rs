//! T-AUTH-1 Red contract for the local master-password authentication setup
//! flow (FR-049 + SC-033).
//!
//! These tests intentionally target the public `auth` API planned by T-AUTH-5
//! (`hivegui::auth::{keystore, policy, ui}`). They must remain compile-Red
//! until that module exists; do not replace them with fixture or private
//! validation tests.
//!
//! Coverage:
//! - FR-049: 首次启动必须进入密码设置界面；弱密码、长度 < 12 或仅一类
//!   字符的密码、与 `PasswordPolicy::os_pw_passwd_for_test` 注入的 OS
//!   passwd 字符集同形且同长的密码，必须 100% 拒绝。
//! - SC-033: 接受密码后仅写 `wrapped_device_key`，不重复 T025 已 Red 的
//!   设备密钥生命周期；Argon2id 派生耗时 < 5000ms（通过
//!   `AuthKeystore::set_clock_for_test` 注入式慢时钟可测）；
//!   超过 5000ms 必须 fail-closed，阻断主 UI。
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
    crypto::SlowClock,
    keystore::{AuthError, AuthKeystore, AuthOutcome, PasswordProposal},
    policy::{PasswordPolicy, PasswordRejection},
    ui::{AuthSetupView, SetupScreen},
};
use support::TestWorkspace;

const STRONG_PASSWORD: &str = "CorrectHorseBattery-2026!Staple";
const SHORT_PASSWORD: &str = "abc123";
const ALPHABETIC_ONLY: &str = "abcdefghijkl";
const DIGITS_ONLY: &str = "123456789012";
const KEYSTORE_RELATIVE: &str = "keystore/wrapped_device_key.v1";

fn new_workspace() -> TestWorkspace {
    TestWorkspace::new().expect("allocate isolated test workspace")
}

fn proposed_screen(view: &AuthSetupView) -> SetupScreen {
    view.current_screen()
        .expect("setup view must present a screen")
}

#[test]
fn first_launch_enters_setup_screen() {
    let workspace = new_workspace();
    let view =
        AuthSetupView::open(workspace.root()).expect("first launch must enter the setup view");

    match proposed_screen(&view) {
        SetupScreen::SetMasterPassword { .. } => {}
        other => panic!("expected SetMasterPassword, got {other:?}"),
    }
}

#[test]
fn weak_passwords_are_uniformly_rejected() {
    let policy = PasswordPolicy::load_default();
    let samples = [
        SHORT_PASSWORD,
        ALPHABETIC_ONLY,
        DIGITS_ONLY,
        "P@ssw0rd",
        "             ",
    ];
    for candidate in samples {
        match policy.evaluate(candidate) {
            PasswordRejection::Accepted { .. } => {
                panic!("weak password `{candidate}` must be rejected");
            }
            rejection => {
                assert!(
                    rejection.is_blocking(),
                    "weak password `{candidate}` rejection must block setup: {rejection:?}"
                );
            }
        }
    }
}

#[test]
fn password_matching_injected_os_pw_passwd_is_rejected() {
    let policy = PasswordPolicy::load_default();
    // 注入一个 fake OS password；T-AUTH-5 必须在桌面进程中将
    // `getpwuid(getuid()).pw_passwd` 映射到此 setter。
    let fake_os_pw = "local-os-pass-2026";
    policy.set_os_pw_passwd_for_test(fake_os_pw);
    match policy.evaluate(fake_os_pw) {
        PasswordRejection::Accepted { .. } => {
            panic!("password equal to OS pw_passwd must be rejected")
        }
        rejection => assert!(
            rejection.is_blocking(),
            "OS overlap must be blocked: {rejection:?}"
        ),
    }
}

#[test]
fn accepted_password_persists_only_wrapped_device_key() {
    let workspace = new_workspace();
    let view =
        AuthSetupView::open(workspace.root()).expect("first launch must enter the setup view");
    let proposal = PasswordProposal::new(STRONG_PASSWORD, STRONG_PASSWORD)
        .expect("strong password must survive construction");

    let outcome = view
        .submit_setup(proposal)
        .expect("strong password must be accepted");

    match outcome {
        AuthOutcome::SetupCompleted { keystore } => {
            let path: PathBuf = workspace.root().join(KEYSTORE_RELATIVE);
            assert!(
                path.exists(),
                "wrapped_device_key file must exist at {}",
                path.display()
            );
            let metadata = fs::metadata(&path).expect("stat wrapped_device_key");
            let mode = metadata.permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "wrapped_device_key must be 0600");
            let contents = fs::read(&path).expect("read wrapped_device_key");
            assert!(
                contents.starts_with(b"AUTHV1"),
                "wrapped_device_key must carry the AUTHV1 magic prefix; \
                 ensures this is the password-wrapped keystore, not the T025 device-only key"
            );
            let keystore: AuthKeystore = keystore;
            assert_eq!(
                keystore.version(),
                1,
                "wrapped_device_key must be version 1; T025 device key is not used"
            );
        }
        other => panic!("expected SetupCompleted, got {other:?}"),
    }
}

#[test]
fn accepted_password_must_not_touch_t025_device_key() {
    let workspace = new_workspace();
    let t025_path = workspace.root().join("datasource/key_store.bin");
    let existed_before = t025_path.exists();
    let mtime_before = existed_before
        .then(|| {
            fs::metadata(&t025_path)
                .expect("stat T025 key")
                .modified()
                .ok()
        })
        .flatten();

    let view =
        AuthSetupView::open(workspace.root()).expect("first launch must enter the setup view");
    let proposal = PasswordProposal::new(STRONG_PASSWORD, STRONG_PASSWORD)
        .expect("strong password must survive construction");
    let _ = view
        .submit_setup(proposal)
        .expect("strong password must be accepted");

    if let (Some(before), true) = (mtime_before, t025_path.exists()) {
        let after = fs::metadata(&t025_path)
            .expect("stat T025 key after auth setup")
            .modified()
            .expect("mtime available");
        assert_eq!(
            before, after,
            "T-AUTH setup must not modify the T025 device-only key file"
        );
    }
}

#[test]
fn argon2id_derivation_under_5_seconds_otherwise_fail_closed() {
    let workspace = new_workspace();
    // T-AUTH-5: 先用强密码建立 wrapped_device_key.v1，再用 6s 慢时钟触发
    // 5s deadline fail-closed。
    let view =
        AuthSetupView::open(workspace.root()).expect("first launch must enter the setup view");
    let proposal = PasswordProposal::new(STRONG_PASSWORD, STRONG_PASSWORD)
        .expect("strong password must survive construction");
    let _ = view
        .submit_setup(proposal)
        .expect("strong password must be accepted");
    let mut keystore = AuthKeystore::open(workspace.root(), STRONG_PASSWORD)
        .expect("strong password must unlock the wrapped keystore");

    // 注入式慢时钟把派生时间调到 6 秒；T-AUTH-5 必须按 5s deadline fail-closed。
    keystore.set_clock_for_test(SlowClock::after_6s());
    match keystore.derive_kek_with_5s_deadline() {
        Ok(_) => panic!("slow clock must force the 5s deadline to fire"),
        Err(AuthError::DerivationTimeout { elapsed }) => {
            assert!(
                elapsed >= Duration::from_secs(5),
                "fail-closed must only fire after the 5s deadline: {elapsed:?}"
            );
            assert!(
                !keystore.primary_store_open(),
                "primary Store must not open when derivation times out"
            );
        }
        Err(other) => panic!("unexpected AuthError: {other:?}"),
    }
}

#[test]
fn kek_verifier_round_trip_does_not_reveal_kek() {
    let workspace = new_workspace();
    // T-AUTH-5: 先建立 wrapped_device_key.v1，再验证 kek_verifier 往返。
    let view =
        AuthSetupView::open(workspace.root()).expect("first launch must enter the setup view");
    let proposal = PasswordProposal::new(STRONG_PASSWORD, STRONG_PASSWORD)
        .expect("strong password must survive construction");
    let _ = view
        .submit_setup(proposal)
        .expect("strong password must be accepted");
    let keystore = AuthKeystore::open(workspace.root(), STRONG_PASSWORD)
        .expect("strong password must unlock the wrapped keystore");
    let first = keystore
        .verify_kek()
        .expect("first verify_kek must succeed for the correct password");
    let second = keystore
        .verify_kek()
        .expect("second verify_kek must succeed for the correct password");
    assert_eq!(
        first, second,
        "kek_verifier must be deterministic across calls"
    );
    let wrong = AuthKeystore::open(workspace.root(), "WrongPassword123!!")
        .expect("open must succeed for the wrapper; the verify call decides")
        .verify_kek();
    assert!(
        wrong.is_err(),
        "kek_verifier must reject wrong password without distinguishing \
         kek_verifier mismatch from wrapped_device_key unwrap failure"
    );
    if let Err(err) = wrong {
        assert!(
            matches!(err, AuthError::InvalidPassword),
            "kek_verifier failure must collapse to InvalidPassword (no side channel): {err:?}"
        );
    }
}

/// T-AUTH-5 实现提供了 `auth::crypto::{Clock, SlowClock}`；本测试通过
/// 导入 lib 的 `SlowClock` 注入式慢时钟，使 KEK 派生 > 5s deadline。
pub use hivegui::auth::crypto::SlowClock as _SlowClockReExport;
