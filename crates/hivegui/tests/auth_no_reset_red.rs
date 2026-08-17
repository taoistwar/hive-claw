//! T-AUTH-4 Red contract for the no-password-reset + backup-mandatory flow
//! (FR-051 + SC-035).
//!
//! These tests intentionally target the public `auth` API planned by T-AUTH-5
//! (`hivegui::auth::{keystore, policy, ui}`). They must remain compile-Red
//! until that module exists; do not replace them with fixture or private
//! validation tests.
//!
//! Coverage:
//! - FR-051: 首次设置主密码成功后、进入主 UI 之前，UI 必须强制展示
//!   "忘记主密码 = 只能从备份恢复" 风险说明并要求勾选确认；不存在
//!   T129 备份文件时 UI 立即跳到 T129 备份向导并完成首次导出与
//!   SHA-256 校验后，才允许回到主 UI；恢复后**必须重新生成设备密钥**
//!   并以新主密码重新包装（旧 `wrapped_device_key` 物理字节保留仅
//!   用于审计）；`auth::keystore` 不得暴露任何 `reset_password` /
//!   `recover_from_questions` / `recovery_key` API。
//! - SC-035: **注入式篡改备份** fault-injection — 把 T129 备份内
//!   `wrapped_device_key` 替换为旧设备 wrapped blob，断言新设备使用
//!   新主密码仍 fail-closed。
//! - Constitution II.3 (TDD): 本文件必须先观察到 Red，再批准 T-AUTH-5
//!   实现。
//!
//! Owner phase: `Foundation-auth-Red`. Reviewer: 未指派前不得进入
//! T-AUTH-5。

#![cfg(test)]

#[path = "support/mod.rs"]
mod support;

use std::{fs, path::PathBuf, process::Command};

use hivegui::auth::{
    backup::{BackupBundle, BackupExportOutcome, BackupRequiredNotice},
    keystore::{AuthError, AuthKeystore, PasswordProposal, RestoreOutcome},
    policy::PasswordPolicy,
    recovery::{
        AcknowledgeOutcome, RecoveryAcceptance, RecoveryConfirmView, RecoveryEntryKind,
        RecoveryRiskNotice,
    },
    ui::{AuthSetupView, SetupScreen},
};
use support::TestWorkspace;

const STRONG_PASSWORD: &str = "CorrectHorseBattery-2026!Staple";
const STRONG_PASSWORD_2: &str = "DifferentStaple-2026!Battery";
const KEYSTORE_RELATIVE: &str = "keystore/wrapped_device_key.v1";
const BACKUP_RELATIVE: &str = "backups/first_export.tgz.age";

fn new_workspace() -> TestWorkspace {
    TestWorkspace::new().expect("allocate isolated test workspace")
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve repository root")
}

/// 公共 API 表面 grep：T-AUTH-5 实现后必须保证 `auth::keystore`（及
/// 整个 `hivegui::auth`）公共 API 中**不**暴露任何 password reset /
/// recovery_key / recover_from_questions 符号。Constitution Code Quality
/// 原则：禁止在内部/未导出模块夹塞绕过路径。
fn assert_no_reset_or_recovery_api_symbols() {
    let root = repository_root();
    let auth_src = root.join("crates/hivegui/src/auth");
    if !auth_src.exists() {
        // T-AUTH-5 还未创建 `auth/` 模块；grep 视为不适用。
        return;
    }
    for forbidden in [
        "reset_password",
        "recover_from_questions",
        "recovery_key",
        "reset_master_password",
        "forgot_password",
        "emergency_access",
    ] {
        let output = Command::new("grep")
            .args([
                "-rn",
                "--include=*.rs",
                "-E",
                &format!(r"pub\s+(fn|struct|enum|trait)\s+{forbidden}"),
            ])
            .arg(&auth_src)
            .output()
            .expect("grep public surface for forbidden reset API");
        assert!(
            !output.status.success() || String::from_utf8_lossy(&output.stdout).trim().is_empty(),
            "auth::keystore must not expose `{forbidden}`; found:\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

fn assert_api_surface_clean() {
    assert_no_reset_or_recovery_api_symbols();
}

#[test]
fn post_setup_displays_recovery_risk_notice_before_main_ui() {
    let workspace = new_workspace();
    let view =
        AuthSetupView::open(workspace.root()).expect("first launch must enter the setup view");
    let proposal = PasswordProposal::new(STRONG_PASSWORD, STRONG_PASSWORD)
        .expect("strong password must survive construction");
    let _ = view
        .submit_setup(proposal)
        .expect("strong password must be accepted");

    // 设置成功后、进入 MainUi 之前必须展示 recovery confirm 视图。
    let notice = view
        .take_recovery_confirm_view()
        .expect("post-setup must present RecoveryConfirmView");
    match notice {
        RecoveryConfirmView::RiskNotice => {}
    }
    let risk: RecoveryRiskNotice = view.recovery_risk_notice();
    assert_eq!(
        risk.title(),
        "忘记主密码 = 只能从备份恢复",
        "recovery confirm view must surface the exact FR-051 risk title"
    );
    assert!(
        risk.requires_explicit_acknowledgement(),
        "user must explicitly check the acknowledgement before MainUi opens"
    );
    assert!(
        !view.primary_ui_open(),
        "MainUi must not open before acknowledgement"
    );
}

#[test]
fn recovery_view_without_backup_forces_first_export_wizard() {
    let workspace = new_workspace();
    let view = AuthSetupView::open(workspace.root()).expect("setup view");
    let proposal = PasswordProposal::new(STRONG_PASSWORD, STRONG_PASSWORD)
        .expect("strong password must survive construction");
    let _ = view.submit_setup(proposal).expect("setup accepted");
    let _ = view.take_recovery_confirm_view().expect("risk notice");
    assert!(!workspace.root().join(BACKUP_RELATIVE).exists());

    // 用户在未备份状态下尝试直接勾选"已了解风险" → 必须先跳到 T129 备份向导。
    let attempted = view
        .acknowledge_risk_for_test(RecoveryAcceptance::Checked)
        .expect("ack should be evaluated synchronously");
    match attempted {
        AcknowledgeOutcome::BackupRequired { notice } => {
            assert_eq!(
                notice.reason(),
                BackupRequiredNotice::NoPriorBackup,
                "ack-without-backup must be redirected to the backup wizard"
            );
        }
        other => panic!("expected BackupRequired redirect, got {other:?}"),
    }

    // 完成 T129 备份向导：导出 + SHA-256 校验。
    let export = view
        .run_backup_wizard_for_test(STRONG_PASSWORD)
        .expect("export");
    let BackupExportOutcome::Exported { bundle } = export;
    let bundle: BackupBundle = bundle;
    let manifest_sha = bundle.manifest_sha256();
    assert_eq!(
        manifest_sha.len(),
        64,
        "manifest SHA-256 must be 64 hex chars; SHA-256 verification is a hard gate"
    );
    assert!(
        bundle.export_path().exists(),
        "exported bundle must exist on disk"
    );
    let contents = fs::read(bundle.export_path()).expect("read bundle");
    assert!(
        bundle.verify_sha256(&contents),
        "bundle verify_sha256 must succeed; backup integrity is mandatory"
    );
    assert!(
        workspace.root().join(BACKUP_RELATIVE).exists()
            || bundle.export_path().starts_with(workspace.root()),
        "first-export bundle must live under the workspace root"
    );

    // 备份完成后才能再勾选"已了解风险" → 进入主 UI。
    let accepted = view
        .acknowledge_risk_for_test(RecoveryAcceptance::Checked)
        .expect("after backup export the acknowledgement must be accepted");
    assert!(matches!(accepted, AcknowledgeOutcome::Accepted));
    assert!(
        view.primary_ui_open(),
        "MainUi must open after backup + acknowledgement"
    );
}

#[test]
fn no_password_reset_or_recovery_key_symbol_in_public_api() {
    assert_api_surface_clean();
}

#[test]
fn restore_regenerates_device_key_and_re_wraps_with_new_password() {
    let workspace = new_workspace();
    let view = AuthSetupView::open(workspace.root()).expect("setup view");
    let proposal = PasswordProposal::new(STRONG_PASSWORD, STRONG_PASSWORD)
        .expect("strong password must survive construction");
    let _ = view.submit_setup(proposal).expect("setup accepted");
    let _ = view.take_recovery_confirm_view().expect("risk notice");
    let _ = view
        .run_backup_wizard_for_test(STRONG_PASSWORD)
        .expect("export");
    let _ = view
        .acknowledge_risk_for_test(RecoveryAcceptance::Checked)
        .expect("ack");
    let old_keystore = view
        .unlocked_keystore_snapshot()
        .expect("snapshot pre-restore");
    let old_wrapped_bytes =
        fs::read(workspace.root().join(KEYSTORE_RELATIVE)).expect("read old wrapped_device_key");
    let old_device_fingerprint = old_keystore.device_key_fingerprint();

    // 走恢复流程：使用新主密码恢复（设备密钥必须重新生成并重新包装）。
    let restore = view
        .restore_from_backup_for_test(STRONG_PASSWORD_2, /*backup_path*/ None)
        .expect("restore request accepted");
    let RestoreOutcome::Restored { new_keystore } = restore else {
        panic!("expected Restored, got {restore:?}");
    };
    let new_keystore: AuthKeystore = new_keystore;
    assert_ne!(
        new_keystore.device_key_fingerprint(),
        old_device_fingerprint,
        "restore must regenerate the device key; reuse of the old key is a hard fail"
    );
    assert_eq!(
        new_keystore.version(),
        1,
        "post-restore keystore must remain v1"
    );

    // 旧 wrapped_device_key 物理字节必须保留（仅供审计）。
    let after = fs::read(workspace.root().join(KEYSTORE_RELATIVE))
        .expect("read wrapped_device_key after restore");
    let preserved = after
        .windows(old_wrapped_bytes.len())
        .any(|w| w == old_wrapped_bytes.as_slice())
        || audit_log_records_old_wrapped_bytes(&workspace, &old_wrapped_bytes);
    assert!(
        preserved,
        "old wrapped_device_key bytes must be preserved for audit (file or audit log)"
    );
}

fn audit_log_records_old_wrapped_bytes(workspace: &TestWorkspace, bytes: &[u8]) -> bool {
    let audit_log = workspace.root().join("logs/audit.log");
    if !audit_log.exists() {
        return false;
    }
    let text = fs::read_to_string(&audit_log).expect("read audit log");
    let hex = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    text.contains(&hex)
}

#[test]
fn tampered_backup_wrapped_key_yields_fail_closed_on_new_device() {
    let workspace = new_workspace();
    let view = AuthSetupView::open(workspace.root()).expect("setup view");
    let proposal = PasswordProposal::new(STRONG_PASSWORD, STRONG_PASSWORD)
        .expect("strong password must survive construction");
    let _ = view.submit_setup(proposal).expect("setup accepted");
    let _ = view.take_recovery_confirm_view().expect("risk notice");
    let export = view
        .run_backup_wizard_for_test(STRONG_PASSWORD)
        .expect("export");
    let bundle = match export {
        BackupExportOutcome::Exported { bundle } => bundle,
    };
    let _ = view
        .acknowledge_risk_for_test(RecoveryAcceptance::Checked)
        .expect("ack");

    // 模拟攻击者把 T129 备份内 `wrapped_device_key` 替换为旧设备 wrapped blob；
    // 重新打开新设备、注入相同主密码。
    let mut tampered = bundle.clone();
    let bogus_wrapped = b"AUTHV1\x00BOGUS_OLD_DEVICE_WRAPPED_BLOB_FOR_FAULT_INJECTION\x00\x00";
    tampered.overwrite_wrapped_device_key_for_test(bogus_wrapped.to_vec());

    let new_view = AuthSetupView::open(workspace.root()).expect("re-open post-restore-attempt");
    let restore = new_view
        .restore_from_backup_for_test(STRONG_PASSWORD, Some(tampered.export_path().to_path_buf()));
    match restore {
        Ok(RestoreOutcome::Restored { .. }) => {
            panic!("tampered wrapped_device_key must NOT unlock the new device")
        }
        Ok(RestoreOutcome::TamperDetected { .. }) => {}
        Err(AuthError::BackupTamperDetected) => {}
        Err(AuthError::InvalidPassword) => {
            // 也可以 collapse 到 InvalidPassword 避免侧信道。
        }
        Err(other) => panic!(
            "tampered backup must fail-closed; expected BackupTamperDetected or \
             InvalidPassword, got {other:?}"
        ),
    }
    // fail-closed 后必须回到 RecoveryOnly 屏幕，不得让新设备解锁。
    let screen = new_view.current_screen().expect("screen present");
    assert!(
        matches!(screen, SetupScreen::SetMasterPassword { .. }),
        "tampered restore must leave the new device on the SetMasterPassword setup screen; got {screen:?}"
    );
}

#[test]
fn recovery_entry_kinds_are_only_restore_from_backup() {
    // FR-051: 唯一合法 recovery 入口 = 从备份恢复；不得存在
    // reset_password / recover_from_questions / recovery_key / forgot_password
    // 任何"密码旁路" 入口。
    let kinds = RecoveryEntryKind::all();
    assert_eq!(kinds, vec![RecoveryEntryKind::RestoreFromBackup]);
    assert!(
        !kinds.contains(&RecoveryEntryKind::ResetPassword),
        "ResetPassword entry must NOT exist"
    );
    assert!(
        !kinds.contains(&RecoveryEntryKind::RecoverFromQuestions),
        "RecoverFromQuestions entry must NOT exist"
    );
    assert!(
        !kinds.contains(&RecoveryEntryKind::RecoveryKey),
        "RecoveryKey entry must NOT exist"
    );
}

#[test]
fn policy_rejects_weak_acknowledgement_skip_via_no_checkbox() {
    // 用户在 risk notice 视图上**不**勾选复选框就尝试 ack → 必须拒绝。
    let workspace = new_workspace();
    let view = AuthSetupView::open(workspace.root()).expect("setup view");
    let proposal = PasswordProposal::new(STRONG_PASSWORD, STRONG_PASSWORD)
        .expect("strong password must survive construction");
    let _ = view.submit_setup(proposal).expect("setup accepted");
    let _ = view.take_recovery_confirm_view().expect("risk notice");
    let attempted = view
        .acknowledge_risk_for_test(RecoveryAcceptance::Unchecked)
        .expect("unchecked ack must be evaluated synchronously");
    assert!(
        matches!(attempted, AcknowledgeOutcome::AcknowledgementRequired),
        "unchecked acknowledgement must be rejected; got {attempted:?}"
    );
    assert!(
        !view.primary_ui_open(),
        "MainUi must remain closed while acknowledgement is unchecked"
    );
    // 同时不影响其他验证。
    let _ = PasswordPolicy::load_default();
}
