//! Local master-password authentication for HiveGUI (Phase 1A).
//!
//! Architecture (T-AUTH-5):
//! - `keystore` — public surface: `AuthKeystore`, `PasswordProposal`,
//!   `UnlockOutcome`, `UnlockedKeystore`, `AuthError`,
//!   `AuthOutcome`, `RestoreOutcome`, `AuthKeystore` re-wrap.
//! - `crypto` — Argon2id KEK derivation (m=64MiB, t=3, p=1) +
//!   ChaCha20Poly1305 (RFC 8439) wrap/unwrap. 5s deadline (injected clock) →
//!   `DerivationTimeout` + fail-closed.
//! - `lock` — `AuthLockState` with typed `AuthLockReason`
//!   (`None` / `Startup` / `IdleTimeout` / `OsScreenLock` /
//!   `OsScreenLockUnavailable` / `TooManyAttempts` / `UserRequested`),
//!   `IdleActivity` enum, `IdleClock` (injectable), `OsScreenLockEvent`,
//!   `ScreenLockMonitor` (Fail-closed when disabled).
//! - `policy` — `PasswordPolicy`: minimum length, character classes,
//!   rejection of system `pw_passwd`-shaped values, `set_os_pw_passwd_for_test`.
//! - `ui` — `AuthUnlockView` + `AuthSetupView` + `UnlockScreen` enum
//!   + `SetupScreen` enum + `PasswordFieldEcho` enum
//!   + `RecoveryEntryKind` (only `RestoreFromBackup`).
//! - `config` — `AutoLockMinutes` (1..=1440).
//! - `monitor` — `AgentExecution` / `ChatSession` status enums +
//!   `SensitiveCanaryScanner` for plain-text canary scans across
//!   `SqliteMain` / `SqliteWal` / `SqliteShm` / `BackupStaging` /
//!   `DiagnosticsBundle`.
//! - `backup` — `BackupBundle` + `BackupRequiredNotice` (typed).
//! - `recovery` — `RecoveryConfirmView` + `RecoveryRiskNotice` +
//!   `RecoveryAcceptance` + `RecoveryEntryKind`.
//!
//! **Security gates (T025R ⑤ 签字前不得合并；签字机制见 Constitution v1.5.0 §Security Requirements *Single-developer repository clause*，2026-07-30 增补)**:
//! - No `reset_password` / `recover_from_questions` / `recovery_key` /
//!   `forgot_password` / `reset_master_password` / `emergency_access`
//!   public symbol (asserted by `auth_no_reset_red.rs` grep test).
//! - `wrapped_device_key.v1` magic = `AUTHV1`; file mode = 0o600;
//!   T025 `datasource/key_store.bin` must not be touched.
//!
//! **T025R ⑤ 签字状态 (2026-07-30)**：本仓库仅 1 名 active maintainer，由该 maintainer
//! 同时承担 dedicated security review + second approver 角色；self-attestation
//! 见 `specs/011-hivegui-standalone-mode/checklists/security.md` §⑤.11。T-AUTH-5
//! 在 §⑤.11 完成后解除"T025R ⑤ 签字前不得合并" 阻断；其余 5 边界（① 设备密钥、
//! ② sidecar cleanup、③ Plugin sandbox、④ 备份 age 加密、⑥ 远程 MySQL 公开边界）
//! 仍待签字，约束适用未来实现任务。

// `doc 硬门槛` (T025R ⑤ 签字前, Constitution v1.5.0 §Security Requirements
// *Single-developer repository clause* 延续): any new `pub fn` must have a
// `///` doc comment before this module is merged. The 116/116 baseline is
// verified by a Python scan in `checklists/security.md` §⑤.8.
#![warn(missing_docs)]

pub mod backup;
pub mod config;
pub mod crypto;
pub mod keystore;
pub mod lock;
pub mod monitor;
pub mod policy;
pub mod recovery;
pub mod ui;
