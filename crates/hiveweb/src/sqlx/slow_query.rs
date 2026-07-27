use std::time::Duration;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

use super::sqlx_event::{SqlxQueryFields, SqlxQueryVisitor};

/// A tracing [`Layer`] that detects slow queries by examining the
/// `elapsed_secs` field on `sqlx::query` events.
///
/// When a query exceeds the configured threshold, this layer re-emits a
/// warning-level event at target `"sqlx::slow_query"` with timing details.
/// This is complementary to sqlx's built-in slow-query logging — it operates
/// at the tracing subscriber level, so it can combine with other filters
/// and formatters without changing database connection options.
///
/// ## Comparison with `SqlxLayer::with_min_duration`
///
/// - `SqlxLayer::with_min_duration` **filters out** fast queries from the
///   reformatted output entirely.
/// - `SlowQueryLayer` **re-emits** slow queries at a higher log level (WARN)
///   so they stand out, and optionally logs all query timings at DEBUG.
///
/// Use `SlowQueryLayer` when you want fast queries to still appear in logs,
/// but slow queries to be highlighted for investigation.
///
/// ## Example
///
/// ```rust
/// use hiveweb::sqlx::slow_query::SlowQueryLayer;
/// use std::time::Duration;
///
/// // Warn about queries taking 100ms or more, and log all query timings at DEBUG.
/// let layer = SlowQueryLayer::from_millis(100).with_log_all();
/// ```
pub struct SlowQueryLayer {
    /// The duration threshold. Queries taking >= this are "slow".
    threshold: Duration,
    /// If true, also emit DEBUG-level timing events for all queries.
    log_all: bool,
}

impl SlowQueryLayer {
    /// Create a new `SlowQueryLayer` with the given duration threshold.
    ///
    /// Queries whose elapsed time >= `threshold` will be re-emitted at
    /// `WARN` level under the `"sqlx::slow_query"` target.
    pub fn new(threshold: Duration) -> Self {
        Self {
            threshold,
            log_all: false,
        }
    }

    /// Convenience constructor: create with a threshold in milliseconds.
    ///
    /// ```rust
    /// use hiveweb::sqlx::slow_query::SlowQueryLayer;
    /// let layer = SlowQueryLayer::from_millis(500); // 500ms threshold
    /// ```
    pub fn from_millis(ms: u64) -> Self {
        Self::new(Duration::from_millis(ms))
    }

    /// Also emit timing information for ALL queries (not just slow ones).
    ///
    /// When enabled, every `sqlx::query` event will produce a DEBUG-level
    /// timing event at target `"sqlx::query_timing"` with the `summary`,
    /// `elapsed`, and `elapsed_secs` fields. Slow queries will additionally
    /// produce the WARN-level event as usual.
    pub fn with_log_all(mut self) -> Self {
        self.log_all = true;
        self
    }
}

impl<S> Layer<S> for SlowQueryLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        // Only process sqlx::query events
        if event.metadata().target() != "sqlx::query" {
            return;
        }

        // Extract fields using the shared visitor
        let mut fields = SqlxQueryFields::default();
        let mut visitor = SqlxQueryVisitor {
            fields: &mut fields,
        };
        event.record(&mut visitor);

        let elapsed_secs = match fields.elapsed_secs {
            Some(s) => s,
            None => return, // No timing data — cannot determine slowness
        };

        let threshold_secs = self.threshold.as_secs_f64();

        if elapsed_secs >= threshold_secs {
            // Slow query detected — emit at WARN level for visibility
            tracing::event!(
                target: "sqlx::slow_query",
                Level::WARN,
                summary = %fields.summary,
                sql = %fields.sql,
                elapsed = fields.elapsed,
                elapsed_secs = fields.elapsed_secs,
                rows_affected = fields.rows_affected,
                rows_returned = fields.rows_returned,
                slow_threshold_ms = self.threshold.as_millis() as u64,
                "slow query detected"
            );
        } else if self.log_all {
            // Normal-speed query — emit timing info at DEBUG for observability
            tracing::event!(
                target: "sqlx::query_timing",
                Level::DEBUG,
                summary = %fields.summary,
                elapsed = fields.elapsed,
                elapsed_secs = fields.elapsed_secs,
                threshold_ms = self.threshold.as_millis() as u64,
            );
        }
    }
}
