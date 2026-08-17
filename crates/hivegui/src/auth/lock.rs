//! Auth lock state, idle clock, OS screen-lock event stream
//! (T-AUTH-2 / T-AUTH-3, FR-050 / SC-034).

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::config::AutoLockMinutes;

/// Reason the keystore is (or is not) locked, driving the unlock UI and
/// auto-lock policy.
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

/// User activity that can reset the idle auto-lock timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdleActivity {
    /// A keyboard key press.
    KeyPress,
    /// A mouse-down on the main window.
    MainWindowMouseDown,
    /// An application focus change.
    FocusChange,
    /// Mouse movement (ignored by the idle policy).
    MouseMove,
}

// Re-export the unified Clock trait and concrete clock types from `crypto`
// so that `lock::Clock`, `lock::SystemClock`, `lock::TestClock` are all
// aliases of `auth::crypto::{Clock, SystemClock, TestClock}`.
pub use super::crypto::{Clock, SystemClock, TestClock};

/// Error returned by an OS screen-lock monitor.
#[derive(Debug, Error)]
pub enum ScreenLockMonitorError {
    #[error("OS screen-lock monitor unavailable; mode = {mode:?}")]
    /// The monitor is unavailable; `mode` describes the fail-closed behaviour.
    Unavailable {
        /// Fail-closed mode reported by the monitor.
        mode: super::config::LockErrorMode,
    },
    #[error("internal monitor error: {0}")]
    /// An unexpected internal monitor failure.
    Internal(String),
}

/// Normalized OS screen-lock event, independent of platform source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OsScreenLockEvent {
    /// Linux: the screensaver active state changed.
    LinuxScreenSaverActiveChanged,
    /// macOS: the displays went to sleep.
    MacOsScreensDidSleep,
    /// Windows: a session change (WTS) was observed.
    WindowsWtSessionChange,
}

/// Abstract OS screen-lock monitor; yields normalized lock events.
pub trait ScreenLockMonitor: Send + Sync {
    /// Block until the next lock event, or return `Ok(None)` if the stream
    /// ended. Returns `Err` if the monitor is unavailable or failed.
    fn next_event(&self) -> Result<Option<OsScreenLockEvent>, ScreenLockMonitorError>;
    /// Human-readable name of the monitor implementation (for diagnostics).
    fn describe(&self) -> &'static str;
}

/// A monitor that is always disabled and reports `Unavailable`, used to
/// exercise the fail-closed path in tests.
pub struct DisabledMonitor {
    /// Fail-closed mode reported when `next_event` is called.
    pub mode: super::config::LockErrorMode,
    /// Diagnostic label identifying this disabled monitor.
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

/// A resolved lock trigger that the unlock UI can render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LockEvent {
    /// Idle auto-lock fired after `minutes` of inactivity.
    IdleTimeout {
        /// Idle duration (in minutes) that triggered the lock.
        minutes: u16,
    },
    /// OS reported a screen lock.
    OsScreenLock,
    /// Too many failed unlock attempts.
    TooManyAttempts,
    /// The user explicitly requested a lock.
    UserRequested,
}

/// Source of an idle reset, used to distinguish activity types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdleSource {
    /// A keyboard key press.
    KeyPress,
    /// A mouse-down on the main window.
    MainWindowMouseDown,
    /// An application focus change.
    FocusChange,
    /// Mouse movement.
    MouseMove,
}

/// Tracks last user activity and decides when the idle auto-lock should fire.
pub struct IdleTracker {
    /// Configured auto-lock idle threshold.
    pub auto_lock_minutes: AutoLockMinutes,
    /// Instant of the last recorded (non-MouseMove) activity.
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
