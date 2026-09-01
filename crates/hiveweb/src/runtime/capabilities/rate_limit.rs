//! Process-wide capability rate limits.
//!
//! `network.http` is bounded per Plugin/session, while `log.emit` uses a
//! per-Plugin token bucket. The dispatcher owns the policy decision and keeps
//! the returned network permit alive for the full outbound operation.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const NETWORK_HTTP_MAX_CONCURRENT: usize = 8;
pub const LOG_EMIT_MAX_PER_SECOND: usize = 100;
const LOG_BUCKET_IDLE_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct NetworkLimitKey {
    plugin_id: i64,
    session_id: Option<i64>,
}

#[derive(Debug)]
struct LogBucket {
    tokens: f64,
    last_refill: Instant,
    last_seen: Instant,
}

impl LogBucket {
    fn full(now: Instant) -> Self {
        Self {
            tokens: LOG_EMIT_MAX_PER_SECOND as f64,
            last_refill: now,
            last_seen: now,
        }
    }

    fn try_consume(&mut self, now: Instant) -> bool {
        // Test helpers may deliberately present timestamps out of order, and
        // production callers can otherwise sample `Instant` before contending
        // on this bucket's mutex. Never move either bucket clock backwards or
        // the same elapsed interval could be refilled more than once.
        let effective_now = now.max(self.last_refill).max(self.last_seen);
        let elapsed = effective_now.duration_since(self.last_refill);
        let refilled = elapsed.as_secs_f64() * LOG_EMIT_MAX_PER_SECOND as f64;
        self.tokens = (self.tokens + refilled).min(LOG_EMIT_MAX_PER_SECOND as f64);
        self.last_refill = effective_now;
        self.last_seen = effective_now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

#[derive(Debug)]
pub struct CapabilityRateLimits {
    network: Mutex<HashMap<NetworkLimitKey, Arc<Semaphore>>>,
    logs: Mutex<HashMap<i64, LogBucket>>,
}

#[derive(Debug)]
pub struct NetworkPermit {
    _permit: OwnedSemaphorePermit,
}

impl CapabilityRateLimits {
    pub fn new() -> Self {
        Self {
            network: Mutex::new(HashMap::new()),
            logs: Mutex::new(HashMap::new()),
        }
    }

    pub fn try_acquire_network(
        &self,
        plugin_id: i64,
        session_id: Option<i64>,
    ) -> Option<NetworkPermit> {
        let key = NetworkLimitKey {
            plugin_id,
            session_id,
        };
        let mut network = self
            .network
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        network.retain(|_, semaphore| {
            semaphore.available_permits() < NETWORK_HTTP_MAX_CONCURRENT
                || Arc::strong_count(semaphore) > 1
        });
        let semaphore = Arc::clone(
            network
                .entry(key)
                .or_insert_with(|| Arc::new(Semaphore::new(NETWORK_HTTP_MAX_CONCURRENT))),
        );
        drop(network);

        semaphore
            .try_acquire_owned()
            .ok()
            .map(|permit| NetworkPermit { _permit: permit })
    }

    pub fn try_acquire_log(&self, plugin_id: i64) -> bool {
        let mut logs = self
            .logs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Sample the production clock only after acquiring the bucket mutex so
        // lock acquisition order and timestamp order cannot diverge.
        Self::try_acquire_log_locked(&mut logs, plugin_id, Instant::now())
    }

    #[cfg(test)]
    pub(crate) fn try_acquire_log_at(&self, plugin_id: i64, now: Instant) -> bool {
        let mut logs = self
            .logs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::try_acquire_log_locked(&mut logs, plugin_id, now)
    }

    fn try_acquire_log_locked(
        logs: &mut HashMap<i64, LogBucket>,
        plugin_id: i64,
        now: Instant,
    ) -> bool {
        logs.retain(|_, bucket| {
            now.checked_duration_since(bucket.last_seen)
                .unwrap_or_default()
                <= LOG_BUCKET_IDLE_TTL
        });
        logs.entry(plugin_id)
            .or_insert_with(|| LogBucket::full(now))
            .try_consume(now)
    }
}

impl Default for CapabilityRateLimits {
    fn default() -> Self {
        Self::new()
    }
}

static GLOBAL_RATE_LIMITS: OnceLock<Arc<CapabilityRateLimits>> = OnceLock::new();

pub fn global() -> Arc<CapabilityRateLimits> {
    Arc::clone(GLOBAL_RATE_LIMITS.get_or_init(|| Arc::new(CapabilityRateLimits::new())))
}

#[cfg(test)]
mod tests {
    use super::{CapabilityRateLimits, LOG_EMIT_MAX_PER_SECOND, NETWORK_HTTP_MAX_CONCURRENT};
    use std::{
        sync::{
            Arc, Barrier,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    #[test]
    fn network_limit_is_eight_per_plugin_and_session() {
        let limits = CapabilityRateLimits::new();
        let held = (0..NETWORK_HTTP_MAX_CONCURRENT)
            .map(|_| {
                limits
                    .try_acquire_network(7, Some(11))
                    .expect("first eight calls must be admitted")
            })
            .collect::<Vec<_>>();

        assert!(
            limits.try_acquire_network(7, Some(11)).is_none(),
            "the ninth concurrent call must be rejected"
        );
        assert!(
            limits.try_acquire_network(7, Some(12)).is_some(),
            "another session has an independent budget"
        );
        assert!(
            limits.try_acquire_network(8, Some(11)).is_some(),
            "another Plugin has an independent budget"
        );
        let no_session = (0..NETWORK_HTTP_MAX_CONCURRENT)
            .map(|_| {
                limits
                    .try_acquire_network(9, None)
                    .expect("None uses one shared sentinel bucket")
            })
            .collect::<Vec<_>>();
        assert!(
            limits.try_acquire_network(9, None).is_none(),
            "calls without a session must not bypass the shared limit"
        );

        drop(held);
        drop(no_session);
        assert!(
            limits.try_acquire_network(7, Some(11)).is_some(),
            "a completed request must release its permit"
        );
    }

    #[test]
    fn log_limit_is_a_hundred_per_second_per_plugin_token_bucket() {
        let limits = CapabilityRateLimits::new();
        let started = Instant::now();

        for _ in 0..LOG_EMIT_MAX_PER_SECOND {
            assert!(limits.try_acquire_log_at(7, started));
        }
        assert!(
            !limits.try_acquire_log_at(7, started),
            "the immediate 101st record must be rejected"
        );
        assert!(
            limits.try_acquire_log_at(8, started),
            "another Plugin has an independent token bucket"
        );
        assert!(
            !limits.try_acquire_log_at(7, started + Duration::from_millis(5)),
            "half a token is not enough"
        );
        assert!(
            limits.try_acquire_log_at(7, started + Duration::from_millis(10)),
            "one token must refill after 10ms"
        );
    }

    #[test]
    fn out_of_order_log_timestamps_never_rewind_or_double_refill_the_bucket() {
        let limits = CapabilityRateLimits::new();
        let started = Instant::now();

        for _ in 0..LOG_EMIT_MAX_PER_SECOND {
            assert!(limits.try_acquire_log_at(7, started));
        }
        assert!(limits.try_acquire_log_at(7, started + Duration::from_millis(10)));
        assert!(
            !limits.try_acquire_log_at(7, started),
            "an older timestamp must not rewind the bucket clock"
        );
        assert!(
            limits.try_acquire_log_at(7, started + Duration::from_millis(20)),
            "exactly one new token must refill at the next 10ms boundary"
        );
        assert!(
            !limits.try_acquire_log_at(7, started + Duration::from_millis(20)),
            "the rewound timestamp must not allow the same interval to refill twice"
        );

        let logs = limits
            .logs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bucket = logs.get(&7).expect("Plugin bucket must remain active");
        assert_eq!(bucket.last_refill, started + Duration::from_millis(20));
        assert_eq!(bucket.last_seen, started + Duration::from_millis(20));
    }

    #[test]
    fn concurrent_network_admission_never_exceeds_eight() {
        const CONTENDERS: usize = 64;
        let limits = Arc::new(CapabilityRateLimits::new());
        let start = Arc::new(Barrier::new(CONTENDERS + 1));
        let finish = Arc::new(Barrier::new(CONTENDERS + 1));
        let admitted = Arc::new(AtomicUsize::new(0));
        let threads = (0..CONTENDERS)
            .map(|_| {
                let limits = Arc::clone(&limits);
                let start = Arc::clone(&start);
                let finish = Arc::clone(&finish);
                let admitted = Arc::clone(&admitted);
                thread::spawn(move || {
                    start.wait();
                    let permit = limits.try_acquire_network(7, Some(11));
                    if permit.is_some() {
                        admitted.fetch_add(1, Ordering::Relaxed);
                    }
                    finish.wait();
                    drop(permit);
                })
            })
            .collect::<Vec<_>>();

        start.wait();
        finish.wait();
        assert_eq!(
            admitted.load(Ordering::Relaxed),
            NETWORK_HTTP_MAX_CONCURRENT
        );
        for thread in threads {
            thread.join().expect("network contender must not panic");
        }
    }

    #[test]
    fn concurrent_log_admission_never_exceeds_bucket_capacity() {
        const CONTENDERS: usize = 256;
        let limits = Arc::new(CapabilityRateLimits::new());
        let start = Arc::new(Barrier::new(CONTENDERS + 1));
        let admitted = Arc::new(AtomicUsize::new(0));
        let now = Instant::now();
        let threads = (0..CONTENDERS)
            .map(|_| {
                let limits = Arc::clone(&limits);
                let start = Arc::clone(&start);
                let admitted = Arc::clone(&admitted);
                thread::spawn(move || {
                    start.wait();
                    if limits.try_acquire_log_at(7, now) {
                        admitted.fetch_add(1, Ordering::Relaxed);
                    }
                })
            })
            .collect::<Vec<_>>();

        start.wait();
        for thread in threads {
            thread.join().expect("log contender must not panic");
        }
        assert_eq!(admitted.load(Ordering::Relaxed), LOG_EMIT_MAX_PER_SECOND);
    }

    #[test]
    fn concurrent_out_of_order_log_calls_cannot_double_refill_one_interval() {
        const CONTENDERS: usize = 32;
        let limits = Arc::new(CapabilityRateLimits::new());
        let started = Instant::now();
        for _ in 0..LOG_EMIT_MAX_PER_SECOND {
            assert!(limits.try_acquire_log_at(7, started));
        }

        let newer_limits = Arc::clone(&limits);
        let newer = thread::spawn(move || {
            newer_limits.try_acquire_log_at(7, started + Duration::from_millis(10))
        });
        assert!(
            newer.join().expect("newer contender must not panic"),
            "the first elapsed interval must refill one token"
        );

        let older_limits = Arc::clone(&limits);
        let older = thread::spawn(move || older_limits.try_acquire_log_at(7, started));
        assert!(
            !older.join().expect("older contender must not panic"),
            "a later lock acquisition with an older clock must not rewind the bucket"
        );

        let start = Arc::new(Barrier::new(CONTENDERS + 1));
        let admitted = Arc::new(AtomicUsize::new(0));
        let threads = (0..CONTENDERS)
            .map(|_| {
                let limits = Arc::clone(&limits);
                let start = Arc::clone(&start);
                let admitted = Arc::clone(&admitted);
                thread::spawn(move || {
                    start.wait();
                    if limits.try_acquire_log_at(7, started + Duration::from_millis(20)) {
                        admitted.fetch_add(1, Ordering::Relaxed);
                    }
                })
            })
            .collect::<Vec<_>>();
        start.wait();
        for thread in threads {
            thread.join().expect("log contender must not panic");
        }
        assert_eq!(
            admitted.load(Ordering::Relaxed),
            1,
            "one 10ms refill interval must admit exactly one concurrent log"
        );
    }

    #[test]
    fn idle_entries_are_evicted_without_removing_active_network_keys() {
        let limits = CapabilityRateLimits::new();
        let active = limits
            .try_acquire_network(7, Some(11))
            .expect("first network call is admitted");
        let transient = limits
            .try_acquire_network(8, Some(12))
            .expect("second key is admitted");
        drop(transient);
        assert_eq!(
            limits
                .network
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            2
        );

        let sweep = limits
            .try_acquire_network(9, Some(13))
            .expect("sweep trigger is admitted");
        assert_eq!(
            limits
                .network
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            2,
            "the active key and new key remain; the idle key is removed"
        );
        drop(active);
        drop(sweep);
        let final_sweep = limits
            .try_acquire_network(10, Some(14))
            .expect("final sweep trigger is admitted");
        assert_eq!(
            limits
                .network
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            1,
            "released keys are removed on the next admission"
        );
        drop(final_sweep);

        let started = Instant::now();
        assert!(limits.try_acquire_log_at(7, started));
        assert!(limits.try_acquire_log_at(8, started + Duration::from_secs(61)));
        let logs = limits
            .logs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(!logs.contains_key(&7));
        assert!(logs.contains_key(&8));
    }
}
