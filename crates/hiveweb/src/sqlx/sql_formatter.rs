//! SQL formatting utilities for pretty-printing sqlx query statements.
//!
//! sqlx emits `db.statement` as `\n\n{SQL}\n` when the query is longer than
//! the summary (first 4 words). This module provides formatting options to
//! make those statements more readable in log output.
//!
//! ## Addressing sqlx issue #2677
//!
//! The [issue](https://github.com/transact-rs/sqlx/issues/2677) requests
//! pretty-printed SQL with bound parameter values. This module handles the
//! formatting side; bound parameter substitution is not currently possible
//! because sqlx does not expose bound values in its tracing events.

/// SQL formatting variants.
///
/// ```rust
/// use sqlx_demo::sql_formatter::{SqlFormat, format_sql};
///
/// let raw = "\n\nSELECT id, name\nFROM users\nWHERE active = true\n";
///
/// assert_eq!(
///     format_sql(raw, SqlFormat::Compact),
///     "SELECT id, name FROM users WHERE active = true"
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SqlFormat {
    /// Pass through the `db.statement` as-is from sqlx.
    /// sqlx emits `"\n\n{SQL}\n"` which includes leading/trailing newlines.
    Raw,

    /// Compact single-line: strip leading `\n\n`, collapse all whitespace
    /// sequences (including newlines) into single spaces, trim.
    #[default]
    Compact,

    /// Pretty-printed with indentation: strip the sqlx wrapping, detect SQL
    /// clause keywords (SELECT, FROM, WHERE, JOIN, etc.) and align them with
    /// consistent indentation for readability.
    Pretty,
}

/// SQL clause keywords recognised by the pretty-printer.
const SQL_CLAUSE_KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "JOIN",
    "LEFT",
    "RIGHT",
    "INNER",
    "OUTER",
    "CROSS",
    "FULL",
    "NATURAL",
    "ON",
    "AND",
    "OR",
    "NOT",
    "IN",
    "EXISTS",
    "ORDER",
    "GROUP",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "INSERT",
    "INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE",
    "RETURNING",
    "WITH",
    "UNION",
    "ALL",
    "AS",
    "DISTINCT",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "ASC",
    "DESC",
    "NULLS",
    "FIRST",
    "LAST",
    "FETCH",
    "NEXT",
    "ROWS",
    "ONLY",
    "FOR",
    "LOCK",
    "SHARE",
    "NOWAIT",
    "SKIP",
    "LOCKED",
    "CREATE",
    "ALTER",
    "DROP",
    "TABLE",
    "INDEX",
    "VIEW",
    "BETWEEN",
    "LIKE",
    "ILIKE",
    "IS",
    "NULL",
    "TRUE",
    "FALSE",
    "USING",
    "LATERAL",
];

/// Format a SQL string using the given format variant.
///
/// If the SQL is short (doesn't contain the `\n\n` prefix that sqlx adds),
/// it is returned as-is regardless of the format setting.
pub fn format_sql(sql: &str, format: SqlFormat) -> String {
    match format {
        SqlFormat::Raw => sql.to_string(),
        SqlFormat::Compact => compact_sql(sql),
        SqlFormat::Pretty => pretty_sql(sql),
    }
}

/// Strip the sqlx wrapping and collapse all whitespace to single spaces.
///
/// sqlx emits `db.statement` as `"\n\n{SQL}\n"` only when the query is
/// longer than the summary (first 4 words). Short queries have the full
/// SQL in the `summary` field, not `db.statement`, so `db.statement`
/// may be empty.
pub fn compact_sql(sql: &str) -> String {
    let sql = strip_sqlx_wrapping(sql);
    if sql.is_empty() {
        return String::new();
    }
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Pretty-print a SQL statement with clause-based indentation.
///
/// Strips the sqlx newline wrapping, then applies a simple keyword-based
/// indentation heuristic:
/// - The first keyword (e.g., `SELECT`) gets no extra indent.
/// - Subsequent clause keywords at the start of a line get a 2-space indent.
/// - Continuation lines (column lists, conditions) get a 4-space indent.
///
/// This does NOT use a full SQL parser; it uses heuristics that cover
/// the most common cases well.
pub fn pretty_sql(sql: &str) -> String {
    let sql = strip_sqlx_wrapping(sql);
    if sql.is_empty() {
        return String::new();
    }

    let lines: Vec<&str> = sql.lines().collect();
    if lines.is_empty() {
        return String::new();
    }

    // Short-circuit: if the SQL is a single line (no actual newlines),
    // return it as-is — no formatting needed.
    if lines.len() == 1 {
        return lines[0].trim().to_string();
    }

    let mut result = String::new();
    let mut first_keyword_seen = false;

    for line in &lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            result.push('\n');
            continue;
        }

        let first_word = trimmed
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_uppercase();

        let is_clause_keyword = SQL_CLAUSE_KEYWORDS.iter().any(|kw| kw == &first_word);

        if is_clause_keyword && !first_keyword_seen {
            // First major keyword: no indent
            first_keyword_seen = true;
            result.push_str(trimmed);
        } else if is_clause_keyword {
            // Subsequent clause keywords: 2-space indent
            result.push_str("  ");
            result.push_str(trimmed);
        } else {
            // Continuation lines (columns, values, conditions): 4-space indent
            result.push_str("    ");
            result.push_str(trimmed);
        }

        result.push('\n');
    }

    // Remove trailing newline
    result.trim_end().to_string()
}

/// Strip the `\n\n` prefix and trailing `\n` that sqlx adds to `db.statement`.
fn strip_sqlx_wrapping(sql: &str) -> &str {
    let sql = sql.strip_prefix("\n\n").unwrap_or(sql);
    sql.strip_suffix('\n').unwrap_or(sql)
}

/// Truncate a SQL string to `max_len` characters, appending a truncation
/// marker if the original is longer.
pub fn truncate_sql(sql: &str, max_len: usize) -> String {
    let sql = compact_sql(sql);
    if sql.len() <= max_len {
        return sql;
    }
    let truncate_at = max_len.saturating_sub(15); // leave room for marker
    if truncate_at == 0 {
        return sql[..max_len.min(sql.len())].to_string();
    }
    format!("{}… (truncated)", &sql[..truncate_at])
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- compact_sql tests ---

    #[test]
    fn compact_strips_sqlx_wrapping() {
        let input = "\n\nSELECT *\nFROM users\nWHERE id = $1\n";
        let expected = "SELECT * FROM users WHERE id = $1";
        assert_eq!(compact_sql(input), expected);
    }

    #[test]
    fn compact_empty_string() {
        assert_eq!(compact_sql(""), "");
    }

    #[test]
    fn compact_short_no_wrapping() {
        // Short query — no sqlx wrapping
        assert_eq!(compact_sql("SELECT 1"), "SELECT 1");
    }

    #[test]
    fn compact_only_newlines() {
        assert_eq!(compact_sql("\n\n\n"), "");
    }

    // --- pretty_sql tests ---

    #[test]
    fn pretty_basic_select() {
        let input = "\n\nSELECT id, name, email\nFROM users\nWHERE active = true\nORDER BY name\n";
        let output = pretty_sql(input);
        assert!(output.starts_with("SELECT id, name, email"));
        assert!(output.contains("  FROM users"));
        assert!(output.contains("  WHERE active = true"));
        assert!(output.contains("  ORDER BY name"));
    }

    #[test]
    fn pretty_with_join() {
        let input = "\n\nSELECT u.name, o.total\nFROM users u\nJOIN orders o ON u.id = o.user_id\nWHERE o.status = 'paid'\n";
        let output = pretty_sql(input);
        assert!(output.contains("  FROM users u"));
        // JOIN 后面的 ON 在同一行，因为输入中它们是同一行
        assert!(output.contains("  JOIN orders o ON u.id = o.user_id"));
        assert!(output.contains("  WHERE o.status = 'paid'"));
    }

    #[test]
    fn pretty_empty() {
        assert_eq!(pretty_sql(""), "");
    }

    #[test]
    fn pretty_short_no_wrapping() {
        assert_eq!(pretty_sql("BEGIN"), "BEGIN");
    }

    // --- format_sql dispatcher ---

    #[test]
    fn format_raw_is_passthrough() {
        let input = "\n\nSELECT 1\n";
        assert_eq!(format_sql(input, SqlFormat::Raw), input);
    }

    #[test]
    fn format_compact_uses_compact_sql() {
        let input = "\n\nSELECT *\nFROM t\n";
        assert_eq!(format_sql(input, SqlFormat::Compact), compact_sql(input));
    }

    #[test]
    fn format_pretty_uses_pretty_sql() {
        let input = "\n\nSELECT *\nFROM t\n";
        assert_eq!(format_sql(input, SqlFormat::Pretty), pretty_sql(input));
    }

    // --- truncate_sql tests ---

    #[test]
    fn truncate_short_enough() {
        assert_eq!(truncate_sql("SELECT 1", 20), "SELECT 1");
    }

    #[test]
    fn truncate_long() {
        let long = "SELECT a, b, c, d, e, f, g, h, i, j, k, l, m, n, o, p FROM massive_table";
        let result = truncate_sql(long, 50);
        assert!(result.len() <= 50);
        assert!(result.ends_with("… (truncated)"));
    }
}
