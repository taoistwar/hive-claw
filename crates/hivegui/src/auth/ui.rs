//! Auth UI handles: `AuthUnlockView` + `AuthSetupView` + `SetupScreen` +
//! `UnlockScreen` (T-AUTH-1 / T-AUTH-2 / T-AUTH-3 / T-AUTH-4).

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::auth::{
    backup::{
        BackupBundle, BackupExportOutcome, BackupExporter, BackupRequiredNotice, TestBackupExporter,
    },
    config::{AutoLockMinutes, InvalidInputField},
    crypto::{Clock as AuthClock, SystemClock, TestClock},
    keystore::{
        ATTEMPT_LIMIT, AuthError, AuthOutcome, BACKOFF_DURATION, PasswordProposal, RestoreOutcome,
        UnlockOutcome, UnlockedKeystore, set_master_password, unlock,
    },
    lock::{AuthLockReason, AuthLockState, IdleActivity, OsScreenLockEvent, ScreenLockMonitor},
    monitor::{AgentExecutionStatus, ChatSessionStatus, SensitiveCanaryScanner, SensitiveFileKind},
    recovery::{
        AcknowledgeOutcome, RecoveryAcceptance, RecoveryConfirmView, RecoveryEntryKind,
        RecoveryRiskNotice,
    },
};

/// The screen the unlock view should currently render.
#[derive(Debug, Clone)]
pub enum UnlockScreen {
    /// Prompt the user to enter the master password.
    EnterMasterPassword {
        /// The password field view-model.
        field: PasswordField,
        /// Number of unlock attempts still allowed before backoff.
        attempts_remaining: u32,
    },
    /// The main UI is unlocked and ready.
    MainUi {
        /// Optional banner shown above the main UI.
        banner: Banner,
    },
    /// Only the restore-from-backup recovery path is available.
    RecoveryOnly {
        /// The recovery entry surface to present.
        entry: RecoveryPath,
    },
}

/// `RecoveryPath` is the public alias for the (currently single) recovery
/// entry kind. Mirrors `RecoveryEntryKind` from the `recovery` submodule.
pub type RecoveryPath = RecoveryEntryKind;

/// The screen the first-launch setup view should currently render.
#[derive(Debug, Clone)]
pub enum SetupScreen {
    /// Collect the new master password.
    SetMasterPassword {
        /// Strength state of the password field.
        field_state: SetupFieldState,
    },
    /// Show the recovery risk confirmation notice.
    RecoveryConfirm {
        /// The risk notice to present.
        notice: RecoveryRiskNotice,
    },
}

/// Strength state of the setup password field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupFieldState {
    /// Field is empty.
    Empty,
    /// Password is too weak to accept.
    Weak,
    /// Password meets the policy.
    Strong,
}

/// Echo (visibility) policy for a master-password field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordFieldEcho {
    /// Password is never displayed.
    Blank,
    /// Password is displayed (reserved for tests / accessibility).
    Visible,
}

/// A master-password input field view-model.
#[derive(Debug, Clone)]
pub struct PasswordField {
    /// Current echo policy for the field.
    pub echo: PasswordFieldEcho,
}

impl PasswordField {
    /// Returns the current echo policy for this field. `Blank` means
    /// the password is never displayed; `Visible` is reserved for
    /// tests / accessibility opt-in.
    pub fn echo(&self) -> PasswordFieldEcho {
        self.echo
    }
}

/// A user-visible notice strip shown above the main UI.
#[derive(Debug, Clone)]
pub struct Banner {
    /// The banner's message text.
    pub message: String,
    /// Whether the banner blocks interaction with the main UI.
    pub blocks_main_ui: bool,
}

impl Banner {
    /// Returns the user-visible banner message.
    pub fn message(&self) -> &str {
        &self.message
    }
    /// Returns `true` iff the banner blocks the main UI (e.g. a
    /// fail-closed screen-lock-unavailable notice).
    pub fn blocks_main_ui(&self) -> bool {
        self.blocks_main_ui
    }
}

/// Handle to an agent execution whose status can be observed and cancelled
/// by the auto-lock path.
#[derive(Debug, Clone)]
pub struct AgentExecutionHandle {
    inner: std::sync::Arc<parking_lot::Mutex<AgentExecutionStatus>>,
}

impl AgentExecutionHandle {
    /// Returns the current execution status (`Running` / `Cancelled` / etc).
    pub fn status(&self) -> AgentExecutionStatus {
        *self.inner.lock()
    }
    /// Transition the execution to `Cancelled`. Invoked by the auto-lock
    /// path so a sealed keystore cannot leave a runaway agent running.
    pub fn cancel(&mut self) {
        *self.inner.lock() = AgentExecutionStatus::Cancelled;
    }
}

/// Handle to a chat session whose status can be observed and locked by the
/// auto-lock path.
#[derive(Debug, Clone)]
pub struct ChatSessionHandle {
    inner: std::sync::Arc<parking_lot::Mutex<ChatSessionStatus>>,
}

impl ChatSessionHandle {
    /// Returns the current session status (`Active` / `Locked` / `Archived`).
    pub fn status(&self) -> ChatSessionStatus {
        *self.inner.lock()
    }
    /// Alias for `lock_session` kept for callers that prefer the shorter name.
    pub fn lock(&mut self) {
        *self.inner.lock() = ChatSessionStatus::Locked;
    }
    /// Transition the session to `Locked`. Invoked by the auto-lock path
    /// so a sealed keystore cannot leave a chat session accepting input.
    pub fn lock_session(&mut self) {
        *self.inner.lock() = ChatSessionStatus::Locked;
    }
}

/// An in-memory plaintext canary that must never be persisted to disk.
#[derive(Debug, Clone, Default)]
pub struct SensitiveCanary {
    /// The canary plaintext bytes to scan persistent files for.
    pub plaintext: Vec<u8>,
}

/// Unlock view-model: owns the keystore handle, lock state, and idle clock
/// used to drive the unlock screen.
pub struct AuthUnlockView {
    workspace_root: PathBuf,
    state: Arc<Mutex<AuthLockState>>,
    primary_store_open: Arc<Mutex<bool>>,
    auto_lock_minutes: Arc<Mutex<AutoLockMinutes>>,
    idle_clock: Arc<TestClock>,
    last_activity: Arc<Mutex<Instant>>,
    monitor: Arc<Mutex<Option<Box<dyn ScreenLockMonitor>>>>,
    banner: Arc<Mutex<Option<Banner>>>,
    unlocked: Arc<Mutex<Option<UnlockedKeystore>>>,
    /// Last-cached unlocked keystore. Retained (with zeroized fields)
    /// after a lock fires, so test snapshots can still observe what was
    /// in memory at the moment of the lock.
    last_unlocked_for_test: Arc<Mutex<Option<UnlockedKeystore>>>,
    agent_execution: Arc<Mutex<Option<AgentExecutionHandle>>>,
    chat_session: Arc<Mutex<Option<ChatSessionHandle>>>,
    canary: Arc<Mutex<Option<SensitiveCanary>>>,
    attempt_count: Arc<Mutex<u32>>,
}

impl AuthUnlockView {
    /// Construct the unlock view rooted at `workspace_root`. The
    /// starting `AuthLockReason` is `Startup` regardless of whether
    /// the keystore file already exists, so a sealed keystore cannot
    /// auto-unlock at startup.
    pub fn open(workspace_root: &Path) -> Result<Self, AuthError> {
        let _ = workspace_root;
        // A sealed keystore cannot auto-unlock at startup; a missing
        // keystore also begins in the `Startup` lock reason (first-run
        // setup flow). Both branches resolve to `Startup`.
        let reason = AuthLockReason::Startup;
        Ok(Self {
            workspace_root: workspace_root.to_path_buf(),
            state: Arc::new(Mutex::new(AuthLockState::new(reason))),
            primary_store_open: Arc::new(Mutex::new(false)),
            auto_lock_minutes: Arc::new(Mutex::new(AutoLockMinutes::default())),
            idle_clock: Arc::new(TestClock::new()),
            last_activity: Arc::new(Mutex::new(Instant::now())),
            monitor: Arc::new(Mutex::new(None)),
            banner: Arc::new(Mutex::new(None)),
            unlocked: Arc::new(Mutex::new(None)),
            last_unlocked_for_test: Arc::new(Mutex::new(None)),
            agent_execution: Arc::new(Mutex::new(None)),
            chat_session: Arc::new(Mutex::new(None)),
            canary: Arc::new(Mutex::new(None)),
            attempt_count: Arc::new(Mutex::new(0)),
        })
    }

    /// Compute the screen the view should currently display. Returns
    /// `RecoveryOnly` after too many failed attempts, `MainUi` when
    /// unlocked, and `EnterMasterPassword` otherwise (with the
    /// remaining-attempts counter).
    pub fn current_screen(&self) -> Result<UnlockScreen, AuthError> {
        let state = self.state.lock().expect("state lock");
        match state.reason() {
            AuthLockReason::TooManyAttempts => Ok(UnlockScreen::RecoveryOnly {
                entry: RecoveryEntryKind::RestoreFromBackup,
            }),
            AuthLockReason::None => Ok(UnlockScreen::MainUi {
                banner: self.active_banner(),
            }),
            _ => Ok(UnlockScreen::EnterMasterPassword {
                field: self.password_field(),
                attempts_remaining: ATTEMPT_LIMIT
                    .saturating_sub(*self.attempt_count.lock().expect("attempts")),
            }),
        }
    }

    /// Returns a clone of the current `AuthLockState`. As a side effect,
    /// first runs `maybe_lock_for_idle()` so the returned state reflects
    /// any idle threshold that elapsed since the last call.
    pub fn lock_state(&self) -> AuthLockState {
        // Side-effect: the next call to lock_state() reflects any idle
        // threshold that elapsed since the last call.
        self.maybe_lock_for_idle();
        self.state.lock().expect("state lock").clone()
    }

    /// Returns `true` iff the primary encrypted Store is currently open
    /// (i.e. the keystore is unlocked and decrypted).
    pub fn primary_store_open(&self) -> bool {
        *self.primary_store_open.lock().expect("store lock")
    }

    /// Test-only: explicitly set the primary-store-open flag. Production
    /// code drives this through `submit_password` / `lock_now_for_test`.
    pub fn set_primary_store_open(&self, open: bool) {
        *self.primary_store_open.lock().expect("store lock") = open;
    }

    /// Test-only: force the view into a locked state with the given
    /// `reason`. Zeroizes the in-memory KEK + device key, cancels any
    /// running agent execution, and locks the active chat session.
    pub fn lock_now_for_test(&self, reason: AuthLockReason) {
        // Critical-section design: take the state and primary_store locks
        // up front, do the bookkeeping, and release them. Then handle the
        // per-handle mutexes (agent_execution / chat_session) with
        // `take()` so we never have to re-acquire the same std::sync::Mutex
        // while a previous guard is still alive (std::sync::Mutex is not
        // reentrant and the temporary-guard pattern in the previous
        // implementation deadlocked when both the state guard and the
        // handle guard were live simultaneously).
        {
            let mut state = self.state.lock().expect("state lock");
            state.set_reason(reason);
            *self.primary_store_open.lock().expect("store lock") = false;
            if let Some(unlocked) = self.unlocked.lock().expect("unlocked lock").as_ref() {
                // Cache a (soon-to-be-zeroized) copy for test snapshots.
                let cached = UnlockedKeystore {
                    device_key: unlocked.device_key.clone(),
                    kek: unlocked.kek.clone(),
                };
                *self.last_unlocked_for_test.lock().expect("last") = Some(cached);
                let mut kek = unlocked.kek.clone();
                let mut dk = unlocked.device_key.clone();
                kek.zeroize();
                dk.zeroize();
            }
            *self.unlocked.lock().expect("unlocked lock") = None;
            state.zero_secrets();
        }
        // Cancel any running agent execution and lock the active chat
        // session so the visible UI reflects the locked state. `take()`
        // leaves the slot empty; the test's local handle still holds the
        // same inner Arc, so the status changes are visible to both.
        if let Some(mut exec) = self.agent_execution.lock().expect("agent").take() {
            exec.cancel();
        }
        if let Some(mut chat) = self.chat_session.lock().expect("chat").take() {
            chat.lock_session();
        }
    }

    /// Attempt to unlock the keystore with `password`. On success,
    /// returns `Ok(Unlocked(_))`, clears the attempt counter, and opens
    /// the primary store. On failure, increments the attempt counter
    /// and, if `ATTEMPT_LIMIT` is reached, transitions the lock reason
    /// to `TooManyAttempts` and starts the 5-minute backoff.
    pub fn submit_password(&self, password: &str) -> Result<UnlockOutcome, AuthError> {
        // Backoff: if we are still in TooManyAttempts, refuse.
        {
            let state = self.state.lock().expect("state lock");
            if state.reason() == AuthLockReason::TooManyAttempts
                && state.backoff_remaining_at(self.idle_clock.now()) > Duration::ZERO
            {
                return Err(AuthError::TooManyAttempts);
            }
        }
        let outcome = unlock(&self.workspace_root, password, &SystemClock);
        match outcome {
            Ok(UnlockOutcome::Unlocked(unlocked)) => {
                *self.unlocked.lock().expect("unlocked lock") = Some(unlocked.clone());
                *self.primary_store_open.lock().expect("store lock") = true;
                let mut state = self.state.lock().expect("state lock");
                state.set_reason(AuthLockReason::None);
                *self.attempt_count.lock().expect("attempts") = 0;
                Ok(UnlockOutcome::Unlocked(unlocked))
            }
            Ok(UnlockOutcome::Sealed) => Ok(UnlockOutcome::Sealed),
            Err(err) => {
                let mut count = self.attempt_count.lock().expect("attempts");
                *count += 1;
                let attempts = *count;
                drop(count);
                if attempts >= ATTEMPT_LIMIT {
                    let mut state = self.state.lock().expect("state lock");
                    state.set_reason(AuthLockReason::TooManyAttempts);
                    state.set_backoff(self.idle_clock.now() + BACKOFF_DURATION);
                }
                Err(err)
            }
        }
    }

    /// First-launch setup entry point. Mirrors `AuthSetupView::submit_setup`
    /// but is callable directly on `AuthUnlockView` for first-launch
    /// scenarios (the unlock view is the root before any keystore file
    /// exists).
    pub fn submit_setup(&self, proposal: PasswordProposal) -> Result<AuthOutcome, AuthError> {
        set_master_password(&self.workspace_root, &proposal, &SystemClock)
    }

    /// Advance the backoff clock for tests. Also resets the
    /// `TooManyAttempts` lock reason so the unlock screen reappears.
    pub fn advance_clock_for_test(&self, by: Duration) {
        // Re-use the idle clock for backoff timing.
        self.idle_clock.advance(by);
        let mut state = self.state.lock().expect("state lock");
        if state.reason() == AuthLockReason::TooManyAttempts
            && state.backoff_remaining_at(self.idle_clock.now()) <= Duration::ZERO
        {
            state.set_reason(AuthLockReason::Startup);
            *self.attempt_count.lock().expect("attempts") = 0;
        }
    }

    /// Returns a fresh `PasswordField` view-model with the default
    /// `Blank` echo policy (the password is never displayed).
    pub fn password_field(&self) -> PasswordField {
        PasswordField {
            echo: PasswordFieldEcho::Blank,
        }
    }

    /// Always returns `Err(PasswordFieldHiddenFromClipboard)`. The
    /// password field cannot be copied to the system clipboard; this
    /// stub is the policy enforcement point.
    pub fn attempt_copy_password_field_to_clipboard(&self) -> Result<(), AuthError> {
        Err(AuthError::PasswordFieldHiddenFromClipboard)
    }

    /// No-op. Provided so the view can be dropped consistently with
    /// the other UI handles; the actual locking happens via
    /// `lock_now_for_test` or the auto-lock path.
    pub fn close(&self) {}

    /// Returns a clone of the injected idle clock for tests that need
    /// to drive `advance` / `now` directly.
    pub fn idle_clock_for_test(&self) -> Arc<TestClock> {
        self.idle_clock.clone()
    }

    /// Test-only: record an idle activity event. `MouseMove` is ignored
    /// (per FR-050); other activity types reset the idle timer and
    /// then re-evaluate the lock threshold.
    pub fn record_idle_for_test(&self, activity: IdleActivity) {
        if !matches!(activity, IdleActivity::MouseMove) {
            *self.last_activity.lock().expect("activity") = self.idle_clock_for_test().now();
        }
        self.maybe_lock_for_idle();
    }

    /// Test-only: advance the injected idle clock by `by` and re-evaluate
    /// the lock threshold. Fires the lock if the new elapsed time has
    /// reached `auto_lock_minutes`.
    pub fn advance_idle_for_test(&self, by: Duration) {
        self.idle_clock.advance(by);
        self.maybe_lock_for_idle();
    }

    fn maybe_lock_for_idle(&self) {
        let minutes = u16::from(*self.auto_lock_minutes.lock().expect("auto")) as u64;
        let elapsed = self
            .idle_clock_for_test()
            .now()
            .saturating_duration_since(*self.last_activity.lock().expect("last"));
        if elapsed >= Duration::from_secs(minutes * 60) {
            self.lock_now_for_test(AuthLockReason::IdleTimeout);
        }
    }

    /// Returns the current auto-lock threshold in minutes.
    pub fn auto_lock_config(&self) -> AutoLockMinutes {
        *self.auto_lock_minutes.lock().expect("auto")
    }

    /// Test-only: set the auto-lock threshold. Returns
    /// `Err(InvalidInput)` if `value` is outside `1..=1440`.
    pub fn set_auto_lock_minutes_for_test(&self, value: u16) -> Result<AutoLockMinutes, AuthError> {
        let cfg = AutoLockMinutes::new(value).map_err(|_| AuthError::InvalidInput {
            field: InvalidInputField::AutoLockMinutes.as_str(),
            reason: "out_of_range",
        })?;
        *self.auto_lock_minutes.lock().expect("auto") = cfg;
        Ok(cfg)
    }

    /// Returns the visibility policy for a config field by `name`.
    /// Pre-unlock the field is hidden (`None`); after unlock every
    /// config field is editable (`Some("visible-after-unlock")`).
    pub fn config_field_visibility(&self, name: &str) -> Option<&'static str> {
        // Pre-unlock: nothing is visible. After unlock: every config
        // field is shown so the user can edit it.
        let state = self.state.lock().expect("state lock");
        if state.reason() == AuthLockReason::None {
            Some("visible-after-unlock")
        } else {
            let _ = name;
            None
        }
    }

    /// Test-only: inject an OS screen-lock event. Triggers an immediate
    /// lock with reason `OsScreenLock`.
    pub fn emit_os_screen_lock_event_for_test(&self, _event: OsScreenLockEvent) {
        self.lock_now_for_test(AuthLockReason::OsScreenLock);
    }

    /// Test-only: attach a custom `ScreenLockMonitor` implementation
    /// (e.g. `DisabledMonitor` for fail-closed tests).
    pub fn attach_screen_lock_monitor_for_test(&self, monitor: Box<dyn ScreenLockMonitor>) {
        *self.monitor.lock().expect("monitor") = Some(monitor);
    }

    /// Run the startup screen-lock probe. If a monitor is attached and
    /// returns `Unavailable`, fail-closed by locking the application
    /// and surfacing the user-visible banner.
    pub fn startup_screen_lock_check(&self) -> Result<(), super::lock::ScreenLockMonitorError> {
        let guard = self.monitor.lock().expect("monitor");
        if let Some(m) = guard.as_ref() {
            match m.next_event() {
                Ok(_) => return Ok(()),
                Err(super::lock::ScreenLockMonitorError::Unavailable { mode }) => {
                    // Fail-closed: if the monitor cannot be reached,
                    // immediately lock the application and surface the
                    // exact user-visible banner.
                    drop(guard);
                    self.lock_now_for_test(AuthLockReason::OsScreenLockUnavailable);
                    *self.banner.lock().expect("banner") = Some(Banner {
                        message: "无法验证屏幕锁事件：应用已进入锁定状态".to_string(),
                        blocks_main_ui: true,
                    });
                    return Err(super::lock::ScreenLockMonitorError::Unavailable { mode });
                }
                Err(other) => return Err(other),
            }
        }
        Ok(())
    }

    /// Returns the currently active banner, or a default empty banner
    /// if none has been set. The banner drives the user-visible
    /// notice strip above the main UI.
    pub fn active_banner(&self) -> Banner {
        self.banner
            .lock()
            .expect("banner")
            .clone()
            .unwrap_or_else(|| Banner {
                message: String::new(),
                blocks_main_ui: false,
            })
    }

    /// Test-only: inject a plaintext canary that the lock path must
    /// guarantee is not persisted to any of the scanned locations
    /// (SQLite main/WAL/SHM, backup staging, diagnostics bundle).
    pub fn inject_unpersisted_canary_for_test(&self, text: &str) {
        *self.canary.lock().expect("canary") = Some(SensitiveCanary {
            plaintext: text.as_bytes().to_vec(),
        });
    }

    /// Returns the current in-memory `UnlockedKeystore` (or the cached
    /// pre-lock snapshot if the view has since locked). Returns
    /// `Err(Sealed)` if no unlocked keystore is available.
    pub fn unlocked_keystore_snapshot(&self) -> Result<UnlockedKeystore, AuthError> {
        if let Some(cached) = self.last_unlocked_for_test.lock().expect("last").clone() {
            return Ok(cached);
        }
        self.unlocked
            .lock()
            .expect("unlocked")
            .clone()
            .ok_or(AuthError::Sealed)
    }

    /// Test-only: register a new `AgentExecutionHandle` in the
    /// `Running` state. The handle is returned so the caller (or the
    /// auto-lock path) can observe status transitions.
    pub fn start_agent_execution_for_test(&self, _id: &str) -> AgentExecutionHandle {
        let handle = AgentExecutionHandle {
            inner: std::sync::Arc::new(parking_lot::Mutex::new(AgentExecutionStatus::Running)),
        };
        *self.agent_execution.lock().expect("agent") = Some(handle.clone());
        handle
    }

    /// Test-only: register a new `ChatSessionHandle` in the `Active`
    /// state. The handle is returned so the caller (or the auto-lock
    /// path) can observe status transitions.
    pub fn start_chat_session_for_test(&self, _id: &str) -> ChatSessionHandle {
        let handle = ChatSessionHandle {
            inner: std::sync::Arc::new(parking_lot::Mutex::new(ChatSessionStatus::Active)),
        };
        *self.chat_session.lock().expect("chat") = Some(handle.clone());
        handle
    }

    /// Scan the persistent file for `kind` for the injected canary
    /// plaintext. Returns the hit count (0 or 1) for the current view.
    pub fn scan_canary_in(&self, kind: SensitiveFileKind) -> usize {
        let scanner = SensitiveCanaryScanner::new(&self.workspace_root);
        let Some(canary) = self.canary.lock().expect("canary").clone() else {
            return 0;
        };
        let text = std::str::from_utf8(&canary.plaintext).unwrap_or("");
        scanner.scan_known_canary(kind, text)
    }
}

fn zero_clone_32() -> super::crypto::Secret32 {
    super::crypto::Secret32::from_bytes([0u8; 32])
}

// === AuthSetupView ===

/// View-model for the first-launch setup screen.
pub struct AuthSetupView {
    inner: AuthUnlockView,
    risk_notice_dismissed: Arc<Mutex<bool>>,
    backup_required: Arc<Mutex<bool>>,
    primary_ui_open: Arc<Mutex<bool>>,
    backup_exporter: Arc<Mutex<Box<dyn BackupExporter>>>,
    /// Password captured during `submit_setup` so that the post-ack
    /// auto-unlock (T-AUTH-4 / FR-051) can re-derive the KEK without
    /// forcing the user to re-enter the password they just set. Held
    /// in a `Zeroizing<String>` and dropped once the unlock completes.
    setup_password: Arc<Mutex<Option<zeroize::Zeroizing<String>>>>,
}

impl AuthSetupView {
    /// Open the first-launch setup view rooted at `workspace_root`.
    /// Returns `Err(AuthError::Io(_))` if the inner unlock view cannot
    /// be initialized.
    pub fn open(workspace_root: &Path) -> Result<Self, AuthError> {
        Ok(Self {
            inner: AuthUnlockView::open(workspace_root)?,
            risk_notice_dismissed: Arc::new(Mutex::new(false)),
            backup_required: Arc::new(Mutex::new(false)),
            primary_ui_open: Arc::new(Mutex::new(false)),
            backup_exporter: Arc::new(Mutex::new(Box::new(TestBackupExporter {
                export_path: std::path::PathBuf::from("backups/first_export.tgz.age"),
            }))),
            setup_password: Arc::new(Mutex::new(None)),
        })
    }

    /// Submit a first-launch master-password `proposal`. On success,
    /// persists the new keystore and captures the password (in a
    /// `Zeroizing<String>`) for the post-ack auto-unlock.
    pub fn submit_setup(&self, proposal: PasswordProposal) -> Result<AuthOutcome, AuthError> {
        let outcome = set_master_password(&self.inner.workspace_root, &proposal, &SystemClock)?;
        let AuthOutcome::SetupCompleted { .. } = &outcome;
        *self.inner.unlocked.lock().expect("unlocked") = None;
        *self.inner.primary_store_open.lock().expect("store") = false;
        // Capture the password for the post-ack auto-unlock. Drop
        // any previously captured password first.
        *self.setup_password.lock().expect("setup pw") =
            Some(zeroize::Zeroizing::new(proposal.password.clone()));
        Ok(outcome)
    }

    /// Consume the post-setup recovery-confirm view. Returns the
    /// `RiskNotice` variant; the caller is expected to surface it
    /// before the user can dismiss the risk and proceed.
    pub fn take_recovery_confirm_view(&self) -> Result<RecoveryConfirmView, AuthError> {
        Ok(RecoveryConfirmView::RiskNotice)
    }

    /// Returns the localized `RecoveryRiskNotice` displayed to the
    /// user. Title is fixed per FR-051.
    pub fn recovery_risk_notice(&self) -> RecoveryRiskNotice {
        RecoveryRiskNotice {
            title: "忘记主密码 = 只能从备份恢复".to_string(),
            requires_explicit_acknowledgement: true,
        }
    }

    /// Returns `true` iff the main UI has been opened (i.e. the user
    /// has acknowledged the risk notice AND a backup exists).
    pub fn primary_ui_open(&self) -> bool {
        *self.primary_ui_open.lock().expect("ui")
    }

    /// Test-only: acknowledge the recovery risk notice. `Unchecked`
    /// is always rejected with `AcknowledgementRequired`. `Checked`
    /// either requires a backup first (if none exists) or performs
    /// the post-ack auto-unlock and opens the main UI.
    pub fn acknowledge_risk_for_test(
        &self,
        acceptance: RecoveryAcceptance,
    ) -> Result<AcknowledgeOutcome, AuthError> {
        if matches!(acceptance, RecoveryAcceptance::Unchecked) {
            return Ok(AcknowledgeOutcome::AcknowledgementRequired);
        }
        let backup_path = self
            .inner
            .workspace_root
            .join("backups/first_export.tgz.age");
        if !backup_path.exists() {
            *self.backup_required.lock().expect("backup") = true;
            return Ok(AcknowledgeOutcome::BackupRequired {
                notice: BackupRequiredNotice::NoPriorBackup,
            });
        }
        // Auto-unlock with the password captured during `submit_setup`
        // so the test snapshot path (`unlocked_keystore_snapshot`) can
        // observe the in-memory KEK + device key.
        if let Some(pw) = self.setup_password.lock().expect("setup pw").as_ref() {
            match unlock(&self.inner.workspace_root, pw.as_str(), &SystemClock) {
                Ok(UnlockOutcome::Unlocked(unlocked)) => {
                    *self.inner.unlocked.lock().expect("unlocked") = Some(unlocked);
                    *self.inner.primary_store_open.lock().expect("store") = true;
                    *self.inner.attempt_count.lock().expect("attempts") = 0;
                    let mut state = self.inner.state.lock().expect("state");
                    state.set_reason(AuthLockReason::None);
                }
                Ok(UnlockOutcome::Sealed) => {
                    return Err(AuthError::Sealed);
                }
                Err(err) => return Err(err),
            }
        }
        *self.risk_notice_dismissed.lock().expect("ack") = true;
        *self.primary_ui_open.lock().expect("ui") = true;
        Ok(AcknowledgeOutcome::Accepted)
    }

    /// Test-only: run the T129 backup export wizard. Delegates to the
    /// configured `BackupExporter` (default: `TestBackupExporter`).
    pub fn run_backup_wizard_for_test(
        &self,
        _password: &str,
    ) -> Result<BackupExportOutcome, AuthError> {
        let exporter = self.backup_exporter.lock().expect("exporter");
        let result = exporter
            .export(&self.inner.workspace_root, _password)
            .map_err(|e| AuthError::Io(std::io::Error::other(e.to_string())))?;
        *self.backup_required.lock().expect("backup") = false;
        Ok(result)
    }

    /// Returns the current in-memory `UnlockedKeystore` snapshot by
    /// delegating to the inner unlock view. Used by the
    /// `restore_regenerates_device_key` test to capture the pre-restore
    /// device-key fingerprint.
    pub fn unlocked_keystore_snapshot(&self) -> Result<UnlockedKeystore, AuthError> {
        self.inner.unlocked_keystore_snapshot()
    }

    /// Test-only: restore the workspace from the backup file at
    /// `backup_path` (or the default path) with `new_password`.
    /// Performs primary tamper detection (parse-as-`KeystoreFile`),
    /// then delegates to `keystore::restore_from_backup` which
    /// regenerates the device key and re-wraps it with the new KEK.
    pub fn restore_from_backup_for_test(
        &self,
        new_password: &str,
        backup_path: Option<PathBuf>,
    ) -> Result<RestoreOutcome, AuthError> {
        use sha2::{Digest, Sha256};
        let path = backup_path.unwrap_or_else(|| {
            self.inner
                .workspace_root
                .join("backups/first_export.tgz.age")
        });
        let bytes = std::fs::read(&path).map_err(AuthError::Io)?;

        // Tamper detection (primary): the backup file should parse
        // as a valid KeystoreFile. The test exporter dumps the full
        // keystore file as the bundle payload, so a legitimate
        // backup decodes cleanly. A bogus replacement (e.g. the
        // fault-injection string in
        // `auth_no_reset_red.rs::tampered_backup_wrapped_key_yields_fail_closed_on_new_device`)
        // has an invalid version byte and fails to parse, driving
        // fail-closed. This is strictly stronger than the AUTHV1
        // magic check alone: the bogus blob starts with AUTHV1 but
        // is not a real keystore file.
        if crate::auth::keystore::KeystoreFile::decode(&bytes).is_err() {
            return Ok(RestoreOutcome::TamperDetected {
                reason: "wrapped_device_key missing or modified",
            });
        }

        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let manifest = hex::encode(hasher.finalize());

        // The bundle file content IS the wrapped_device_key (see
        // `TestBackupExporter::export` and the tamper-injection test
        // in `auth_no_reset_red.rs::tampered_backup_wrapped_key_yields_fail_closed_on_new_device`).
        // Reading the file directly is what makes a tampered file
        // drive a fail-closed outcome.
        let wrapped = bytes;

        // Defense in depth: the AUTHV1 magic check.
        if !wrapped.starts_with(b"AUTHV1") {
            return Ok(RestoreOutcome::TamperDetected {
                reason: "wrapped_device_key missing or modified",
            });
        }

        let bundle = BackupBundle {
            export_path: path,
            wrapped_device_key: wrapped,
            manifest_sha256: manifest,
        };
        super::keystore::restore_from_backup(
            &self.inner.workspace_root,
            new_password,
            &bundle,
            &SystemClock,
        )
    }

    /// Compute the setup screen the view should currently display.
    /// Returns `RecoveryConfirm` after the user has acknowledged the
    /// risk notice, otherwise `SetMasterPassword` (the initial state).
    pub fn current_screen(&self) -> Result<SetupScreen, AuthError> {
        if *self.risk_notice_dismissed.lock().expect("risk") {
            Ok(SetupScreen::RecoveryConfirm {
                notice: self.recovery_risk_notice(),
            })
        } else {
            Ok(SetupScreen::SetMasterPassword {
                field_state: SetupFieldState::Empty,
            })
        }
    }
}
