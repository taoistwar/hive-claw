//! Auth lock state, idle clock, OS screen-lock event stream
//! (T-AUTH-2 / T-AUTH-3, FR-050 / SC-034).

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::config::{AutoLockMinutes, LockErrorMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthLockReason {
    /// The application is running but the lock state is *not* engaged.
    None,
    /// Cold start: the wrapped keystore is sealed.
    Startup,
    /// Idle >= `auto_lock_minutes`.
    IdleTimeout,
    /// OS-reported screen lock (Linux / macOS / Windows).
    OsScreenLock,
    /// OS screen-lock monitor is disabled/unavailable → fail-closed.
    OsScreenLockUnavailable,
    /// 5 wrong passwords in a row.
    TooManyAttempts,
    /// User explicitly requested lock.
    UserRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdleActivity {
    KeyPress,
    MainWindowMouseDown,
    FocusChange,
    MouseMove,
}

// Re-export the unified Clock trait and concrete clock types from `crypto`
// so that `lock::Clock`, `lock::SystemClock`, `lock::TestClock` are all
// aliases of `auth::crypto::{Clock, SystemClock, TestClock}`.
pub use super::crypto::{Clock, SystemClock, TestClock};

#[derive(Debug, Error)]
pub enum ScreenLockMonitorError {
    #[error("OS screen-lock monitor unavailable; mode = {mode:?}")]
    Unavailable { mode: super::config::LockErrorMode },
    #[error("internal monitor error: {0}")]
    Internal(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OsScreenLockEvent {
    LinuxScreenSaverActiveChanged,
    MacOsScreensDidSleep,
    WindowsWtSessionChange,
}

pub trait ScreenLockMonitor: Send + Sync {
    fn next_event(&self) -> Result<Option<OsScreenLockEvent>, ScreenLockMonitorError>;
    fn describe(&self) -> &'static str;
}

pub struct DisabledMonitor {
    pub mode: super::config::LockErrorMode,
    pub label: &'static str,
}

impl ScreenLockMonitor for DisabledMonitor {
    fn next_event(&self) -> Result<Option<OsScreenLockEvent>, ScreenLockMonitorError> {
        Err(ScreenLockMonitorError::Unavailable { mode: self.mode })
    }
    fn describe(&self) -> &'static str {
        self.label
    }
}

impl DisabledMonitor {
    /// Build a `DisabledMonitor` with `LockErrorMode::FailClosed`. Any
    /// `next_event()` call returns `Err(Unavailable)` so the consumer
    /// can trigger a fail-closed lock.
    pub fn fail_closed() -> Self {
        Self {
            mode: super::config::LockErrorMode::FailClosed,
            label: "disabled-for-test-fail-closed",
        }
    }

    /// Build a `DisabledMonitor` with the given `LockErrorMode`. The
    /// `label` field is set to a mode-specific string for diagnostics.
    pub fn fail_closed_for_mode(mode: super::config::LockErrorMode) -> Self {
        let label = match mode {
            super::config::LockErrorMode::FailClosed => "disabled-for-test-fail-closed",
            super::config::LockErrorMode::Disabled => "disabled-for-test-disabled",
        };
        Self { mode, label }
    }

    /// Test-only: build a `Box<dyn ScreenLockMonitor>` with the requested
    /// `LockErrorMode`. The test files use the
    /// `ScreenLockMonitor::disabled_for_test` qualified path, which
    /// only resolves for concrete impls (the trait is object-safe so
    /// associated fns are not callable through `dyn`).
    pub fn disabled_for_test(mode: super::config::LockErrorMode) -> Box<dyn ScreenLockMonitor> {
        Box::new(Self::fail_closed_for_mode(mode))
    }
}

/// Typed lock-state wrapper. Reason is private; transitions are explicit.
#[derive(Debug, Clone)]
pub struct AuthLockState {
    reason: AuthLockReason,
    backoff_until: Option<Instant>,
    unlocked_secrets_zeroed: bool,
}

impl AuthLockState {
    /// Construct a new lock state with the given `reason`. No
    /// backoff is active and no secrets have been zeroized yet.
    pub fn new(reason: AuthLockReason) -> Self {
        Self {
            reason,
            backoff_until: None,
            unlocked_secrets_zeroed: false,
        }
    }

    /// Returns the current `AuthLockReason` (e.g. `None`,
    /// `Startup`, `TooManyAttempts`).
    pub fn reason(&self) -> AuthLockReason {
        self.reason
    }

    /// Set the lock `reason`. Transitions to `None` also clear the
    /// backoff deadline and the "secrets zeroed" sentinel so a
    /// successful unlock resets both.
    pub fn set_reason(&mut self, reason: AuthLockReason) {
        self.reason = reason;
        if matches!(reason, AuthLockReason::None) {
            self.backoff_until = None;
            self.unlocked_secrets_zeroed = false;
        }
    }

    /// Returns the wall-clock backoff time remaining until
    /// submissions are accepted again. Zero when no backoff is
    /// active.
    pub fn backoff_remaining(&self) -> Duration {
        match self.backoff_until {
            Some(until) => until.saturating_duration_since(Instant::now()),
            None => Duration::ZERO,
        }
    }

    /// `backoff_remaining` using an injected clock. Used by tests that
    /// advance a `TestClock` to simulate the 5-minute backoff elapsing.
    pub fn backoff_remaining_at(&self, now: Instant) -> Duration {
        match self.backoff_until {
            Some(until) => until.saturating_duration_since(now),
            None => Duration::ZERO,
        }
    }

    /// Set the backoff deadline to `until`. After this instant the
    /// state will report zero backoff remaining and submissions are
    /// accepted again.
    pub fn set_backoff(&mut self, until: Instant) {
        self.backoff_until = Some(until);
    }

    /// Mark that the in-memory KEK and device key have been zeroized
    /// (e.g. by `lock_now_for_test`). Idempotent.
    pub fn zero_secrets(&mut self) {
        self.unlocked_secrets_zeroed = true;
    }

    /// Returns `true` iff `zero_secrets` was called since the last
    /// `set_reason(None)`. Used by tests to assert the lock path
    /// actually zeroized the in-memory secrets.
    pub fn unlocked_secrets_zeroed(&self) -> bool {
        self.unlocked_secrets_zeroed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LockEvent {
    IdleTimeout { minutes: u16 },
    OsScreenLock,
    TooManyAttempts,
    UserRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdleSource {
    KeyPress,
    MainWindowMouseDown,
    FocusChange,
    MouseMove,
}

pub struct IdleTracker {
    pub auto_lock_minutes: AutoLockMinutes,
    pub last_activity: parking_lot::Mutex<Instant>,
}

impl IdleTracker {
    /// Construct a new `IdleTracker` with the given clock and
    /// `auto_lock_minutes` threshold. The initial `last_activity` is set
    /// to `clock.now()`.
    pub fn new(clock: &dyn Clock, auto_lock_minutes: AutoLockMinutes) -> Self {
        Self {
            auto_lock_minutes,
            last_activity: parking_lot::Mutex::new(clock.now()),
        }
    }

    /// Record user activity. `MouseMove` is intentionally ignored (per
    /// the FR-050 idle-reset policy) so a wiggling cursor cannot prevent
    /// the auto-lock from firing.
    pub fn record(&self, clock: &dyn Clock, activity: IdleActivity) {
        if !matches!(activity, IdleActivity::MouseMove) {
            *self.last_activity.lock() = clock.now();
        }
    }

    /// Returns the elapsed time since the last recorded (non-MouseMove)
    /// activity, as observed by `clock`.
    pub fn elapsed(&self, clock: &dyn Clock) -> Duration {
        clock
            .now()
            .saturating_duration_since(*self.last_activity.lock())
    }

    /// Returns `true` iff the elapsed idle time has reached or exceeded
    /// `auto_lock_minutes`. The consumer should fire the lock transition.
    pub fn should_lock(&self, clock: &dyn Clock) -> bool {
        let elapsed = self.elapsed(clock);
        let minutes = u16::from(self.auto_lock_minutes) as u64;
        elapsed >= Duration::from_secs(minutes * 60)
    }
}
