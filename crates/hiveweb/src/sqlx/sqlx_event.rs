use std::borrow::BorrowMut;
use tracing::field::{Field, Visit};

/// Fields extracted from a `sqlx::query` tracing event.
///
/// Internal to the shared parser used by both `SqlxLayer` and `SlowQueryLayer`.
#[derive(Debug, Default)]
pub(crate) struct SqlxQueryFields {
    /// The query summary — first 4 words of the SQL statement.
    pub(crate) summary: String,
    /// The full SQL statement. sqlx wraps it as `\n\n{SQL}\n` when the query
    /// is longer than the summary.
    pub(crate) sql: String,
    /// Number of rows affected (INSERT/UPDATE/DELETE).
    pub(crate) rows_affected: Option<u64>,
    /// Number of rows returned (SELECT).
    pub(crate) rows_returned: Option<u64>,
    /// Human-friendly elapsed time with units (e.g., "1.23ms").
    pub(crate) elapsed: Option<String>,
    /// Elapsed time in seconds as f64 — search-friendly numeric field.
    pub(crate) elapsed_secs: Option<f64>,
    /// Whether the query was detected as a slow query by sqlx itself.
    pub(crate) is_slow: bool,
    /// The slow-statement duration threshold from sqlx LogSettings.
    pub(crate) slow_threshold: Option<String>,
    /// Catch-all for any future fields sqlx might add.
    pub(crate) unknown: Option<String>,
}

/// Visitor that extracts fields from a `sqlx::query` tracing event.
///
/// Handles both `record_str` and `record_debug` — sqlx may emit fields
/// using either method depending on the type.
pub(crate) struct SqlxQueryVisitor<'a> {
    pub(crate) fields: &'a mut SqlxQueryFields,
}

impl Visit for SqlxQueryVisitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        let fields = self.fields.borrow_mut();
        match field.name() {
            "summary" => fields.summary = value.to_string(),
            "db.statement" => fields.sql = value.to_string(),
            "rows_affected" => fields.rows_affected = value.parse().ok(),
            "rows_returned" => fields.rows_returned = value.parse().ok(),
            "elapsed" => fields.elapsed = Some(value.to_string()),
            "elapsed_secs" => fields.elapsed_secs = value.parse().ok(),
            _ => {
                fields
                    .unknown
                    .get_or_insert(String::new())
                    .push_str(&format!("\n{}={}", field.name(), value));
            }
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let fields = self.fields.borrow_mut();
        let value = format!("{:?}", value);
        match field.name() {
            "summary" => fields.summary = value,
            "db.statement" => fields.sql = value,
            "rows_affected" => fields.rows_affected = value.parse().ok(),
            "rows_returned" => fields.rows_returned = value.parse().ok(),
            "elapsed" => fields.elapsed = Some(value),
            "elapsed_secs" => fields.elapsed_secs = value.parse().ok(),
            "slow_threshold" => {
                fields.is_slow = true;
                fields.slow_threshold = Some(value);
            }
            _ => {
                fields
                    .unknown
                    .get_or_insert(String::new())
                    .push_str(&format!("\n{}={:?}", field.name(), value));
            }
        }
    }
}
