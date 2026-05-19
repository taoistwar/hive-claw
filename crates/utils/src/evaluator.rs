//! Post-run evaluation for background tasks (port of
//! `nanobot.utils.evaluator`).
//!
//! The Python original calls out to an `LLMProvider` with a tool-use prompt
//! to decide whether a background result deserves a user notification. Here
//! we expose the same high-level signature but delegate the LLM call to a
//! pluggable trait. If no evaluator is provided — or it fails — we default
//! to `true` (notify) to match the Python fail-safe behaviour.

use async_trait::async_trait;

/// Trait implemented by the concrete LLM wrapper in the provider crate.
///
/// The default fallback is `true` so that important messages are never
/// silently dropped.
#[async_trait]
pub trait NotificationEvaluator: Send + Sync {
    /// Return `true` when the user should be notified.
    async fn should_notify(&self, response: &str, task_context: &str, model: &str) -> bool;
}

/// Decide whether a background-task result should be delivered to the user.
///
/// When `evaluator` is `None` we default to `true` (notify), matching the
/// Python fail-safe behaviour.
pub async fn evaluate_response(
    response: &str,
    task_context: &str,
    evaluator: Option<&dyn NotificationEvaluator>,
    model: &str,
) -> bool {
    match evaluator {
        Some(e) => {
            match std::panic::AssertUnwindSafe(e.should_notify(response, task_context, model))
                .catch_unwind_if_possible()
                .await
            {
                Some(v) => v,
                None => {
                    log::warn!("evaluate_response panicked, defaulting to notify");
                    true
                }
            }
        }
        None => true,
    }
}

// --- small helper to emulate the `future.catch_unwind()` pattern without
//     forcing every caller's trait bounds -----------------------------------

use std::future::Future;

#[allow(async_fn_in_trait)]
pub(crate) trait CatchUnwindIfPossible: Future + Sized {
    async fn catch_unwind_if_possible(self) -> Option<Self::Output> {
        Some(self.await)
    }
}

impl<T: Future> CatchUnwindIfPossible for std::panic::AssertUnwindSafe<T> {}
