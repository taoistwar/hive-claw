use std::time::Duration;
use tracing::Level;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

use super::sql_highlighter::SqlHighlighter;

use std::sync::LazyLock;

static SQL_H: LazyLock<SqlHighlighter> = LazyLock::new(SqlHighlighter::new);

use super::sql_formatter::{SqlFormat, format_sql};
use super::sqlx_event::{SqlxQueryFields, SqlxQueryVisitor};

pub struct SqlxLayer {
    format: SqlFormat,
    log_level: Level,
    min_duration: Option<Duration>,
}

impl Default for SqlxLayer {
    fn default() -> Self {
        Self {
            format: SqlFormat::Compact,
            log_level: Level::DEBUG,
            min_duration: None,
        }
    }
}

impl SqlxLayer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_format(mut self, format: SqlFormat) -> Self {
        self.format = format;
        self
    }

    pub fn with_log_level(mut self, level: Level) -> Self {
        self.log_level = level;
        self
    }

    pub fn with_min_duration(mut self, duration: Duration) -> Self {
        self.min_duration = Some(duration);
        self
    }
}

impl<S> Layer<S> for SqlxLayer
where
    S: tracing::Subscriber,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != "sqlx::query" {
            return;
        }

        let mut fields = SqlxQueryFields::default();
        let mut visitor = SqlxQueryVisitor {
            fields: &mut fields,
        };
        event.record(&mut visitor);

        if let Some(min_dur) = self.min_duration
            && let Some(secs) = fields.elapsed_secs
            && secs < min_dur.as_secs_f64()
        {
            return;
        }

        let mut sql = fields.sql;

        sql = SQL_H.highlight_sql(&sql);

        sql = format_sql(&sql, self.format);

        match self.log_level {
            Level::ERROR => tracing::event!(
                target: "sqlx::formatted_query", Level::ERROR,
                sql = %sql, summary = %fields.summary,
                rows_affected = fields.rows_affected, rows_returned = fields.rows_returned,
                elapsed = fields.elapsed, elapsed_secs = fields.elapsed_secs,
                is_slow = fields.is_slow, unknown = fields.unknown,
            ),
            Level::WARN => tracing::event!(
                target: "sqlx::formatted_query", Level::WARN,
                sql = %sql, summary = %fields.summary,
                rows_affected = fields.rows_affected, rows_returned = fields.rows_returned,
                elapsed = fields.elapsed, elapsed_secs = fields.elapsed_secs,
                is_slow = fields.is_slow, unknown = fields.unknown,
            ),
            Level::INFO => tracing::event!(
                target: "sqlx::formatted_query", Level::INFO,
                sql = %sql, summary = %fields.summary,
                rows_affected = fields.rows_affected, rows_returned = fields.rows_returned,
                elapsed = fields.elapsed, elapsed_secs = fields.elapsed_secs,
                is_slow = fields.is_slow, unknown = fields.unknown,
            ),
            Level::DEBUG => tracing::event!(
                target: "sqlx::formatted_query", Level::DEBUG,
                sql = %sql, summary = %fields.summary,
                rows_affected = fields.rows_affected, rows_returned = fields.rows_returned,
                elapsed = fields.elapsed, elapsed_secs = fields.elapsed_secs,
                is_slow = fields.is_slow, unknown = fields.unknown,
            ),
            Level::TRACE => tracing::event!(
                target: "sqlx::formatted_query", Level::TRACE,
                sql = %sql, summary = %fields.summary,
                rows_affected = fields.rows_affected, rows_returned = fields.rows_returned,
                elapsed = fields.elapsed, elapsed_secs = fields.elapsed_secs,
                is_slow = fields.is_slow, unknown = fields.unknown,
            ),
        }
    }
}
