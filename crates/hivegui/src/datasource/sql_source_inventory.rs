//! `sql_source_inventory` — production SQL source-code inventory. The
//! security-remediation boundary scans every HiveGUI and HiveWeb
//! production source file for SQL call sites, validates that every
//! call site is tagged with an `owner_phase` annotation, and proves
//! that the only `AssertSqlSafe` producer lives in the HiveWeb
//! central boundary.

#![warn(missing_docs)]

use std::{
    fs,
    path::{Path, PathBuf},
};

/// Reason a call site is not allowed to construct `AssertSqlSafe`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryError {
    /// The inventory root does not exist.
    MissingRoot,
    /// A source file could not be read.
    IoError,
}

/// Phase that owns one production SQL call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerPhase {
    /// `security-remediation` phase.
    SecurityRemediation,
    /// `Foundation` phase.
    Foundation,
    /// Story-owned call site (e.g. US3, US8).
    Story {
        /// Story identifier.
        story: String,
    },
}

impl OwnerPhase {
    /// Parse an `owner_phase` annotation. Unknown values are mapped
    /// to [`OwnerPhase::Story`] to avoid failing the inventory
    /// loader for downstream stories.
    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "security-remediation" => OwnerPhase::SecurityRemediation,
            "Foundation" => OwnerPhase::Foundation,
            other => OwnerPhase::Story {
                story: other.to_string(),
            },
        }
    }
}

/// One production SQL call site observed in the workspace.
#[derive(Debug, Clone)]
pub struct InventoryEntry {
    /// Path relative to the inventory root.
    relative_path: String,
    /// One-based line number.
    line_number: usize,
    /// Owner phase parsed from the trailing comment.
    owner_phase: OwnerPhase,
    /// SQL call shape.
    kind: SqlCallKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SqlCallKind {
    /// SQLx `query!|query_as!|query_scalar!` macro.
    CheckedMacro,
    /// `sqlx::query(...)` dynamic builder.
    DynamicBuilder,
    /// `QueryBuilder` chain.
    QueryBuilder,
    /// `AssertSqlSafe` construction.
    AssertSqlSafe,
    /// `MysqlIdentifier::from_allowlist` use.
    MysqlIdentifier,
}

impl InventoryEntry {
    /// Returns the path relative to the inventory root.
    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }

    /// Returns the one-based line number.
    pub fn line_number(&self) -> usize {
        self.line_number
    }

    /// Returns the parsed owner phase.
    pub fn owner_phase(&self) -> OwnerPhase {
        self.owner_phase.clone()
    }

    /// Returns true for `QueryBuilder` calls.
    pub fn is_query_builder(&self) -> bool {
        matches!(self.kind, SqlCallKind::QueryBuilder)
    }

    /// Returns true for `AssertSqlSafe` construction.
    pub fn constructs_assert_sql_safe(&self) -> bool {
        matches!(self.kind, SqlCallKind::AssertSqlSafe)
    }

    /// Returns true for checked-macro SQLx construction.
    pub fn is_checked_macro(&self) -> bool {
        matches!(self.kind, SqlCallKind::CheckedMacro)
    }

    /// Returns true for dynamic SQLx constructor usage.
    pub fn is_dynamic_builder(&self) -> bool {
        matches!(self.kind, SqlCallKind::DynamicBuilder)
    }

    /// Returns true for `MysqlIdentifier::from_allowlist` calls.
    pub fn uses_mysql_identifier(&self) -> bool {
        matches!(self.kind, SqlCallKind::MysqlIdentifier)
    }
}

/// Aggregated inventory produced by [`load_for_test`].
#[derive(Debug, Default)]
pub struct SqlSourceInventory {
    entries: Vec<InventoryEntry>,
}

impl SqlSourceInventory {
    /// Returns the list of observed entries.
    pub fn entries(&self) -> &[InventoryEntry] {
        &self.entries
    }

    /// Returns true when the inventory is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Load the inventory for a workspace root. Returns `Ok(None)` when
/// the inventory source is unavailable; the test contract
/// distinguishes between "loader not present" and "loader present
/// but no entries found".
pub fn load_for_test(root: impl AsRef<Path>) -> Result<Option<SqlSourceInventory>, InventoryError> {
    let root = root.as_ref();
    if !root.exists() {
        return Ok(None);
    }

    let mut scan_roots: Vec<(PathBuf, PathBuf)> = Vec::new();
    let local_src = root.join("src");
    if local_src.exists() {
        // Keep HiveGUI production paths relative to the crate root so
        // Foundation filters can still match `src/datasource/...`.
        scan_roots.push((local_src, root.to_path_buf()));
    }
    let hiveweb_src = root.join("../hiveweb/src");
    if hiveweb_src.exists() {
        // HiveWeb paths are scanned separately and normalized to `db/...`
        // / `src/...` style inventory paths.
        scan_roots.push((hiveweb_src.clone(), hiveweb_src));
    }

    let mut entries = Vec::new();
    if scan_roots.is_empty() {
        return Ok(None);
    }

    for (scan_root, relative_root) in &scan_roots {
        scan_dir(relative_root, scan_root, &mut entries)?;
    }

    Ok(Some(SqlSourceInventory { entries }))
}

fn scan_dir(
    relative_root: &Path,
    dir: &Path,
    entries: &mut Vec<InventoryEntry>,
) -> Result<(), InventoryError> {
    let read_dir = match fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(_) => return Err(InventoryError::IoError),
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip target / node_modules / .git.
            if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && matches!(name, "target" | "node_modules" | ".git")
            {
                continue;
            }
            scan_dir(relative_root, &path, entries)?;
            continue;
        }
        if !is_rust_file(&path) {
            continue;
        }
        if should_skip_file(&path) {
            continue;
        }
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(_) => return Err(InventoryError::IoError),
        };
        scan_source(relative_root, &path, &source, entries);
    }
    Ok(())
}

fn should_skip_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "sql_source_inventory.rs")
}

fn is_rust_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("rs")
}

fn scan_source(root: &Path, path: &Path, source: &str, entries: &mut Vec<InventoryEntry>) {
    let relative = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let mut in_block_comment = false;
    for (idx, line) in source.lines().enumerate() {
        let line_number = idx + 1;
        let trimmed = line.trim();
        // Skip `//!` and `///` doc comments and `//` line comments.
        let is_doc_or_line_comment =
            trimmed.starts_with("//!") || trimmed.starts_with("///") || trimmed.starts_with("//");
        if is_doc_or_line_comment {
            in_block_comment = false;
            continue;
        }
        // Track /* ... */ block comments and skip them.
        if in_block_comment {
            if line.contains("*/") {
                in_block_comment = false;
            }
            continue;
        }
        if line.contains("/*") && !line.contains("*/") {
            in_block_comment = true;
            continue;
        }
        let owner_phase = extract_owner_phase(line);
        let kind = classify(trimmed);
        if let Some(kind) = kind {
            entries.push(InventoryEntry {
                relative_path: relative.clone(),
                line_number,
                owner_phase: owner_phase.unwrap_or(OwnerPhase::Story {
                    story: "unknown".to_string(),
                }),
                kind,
            });
        }
    }
}

fn extract_owner_phase(line: &str) -> Option<OwnerPhase> {
    // Look for `// query-plan: id=...; owner_phase=<phase>;`
    let marker = "owner_phase=";
    let after = line.split_once(marker)?.1;
    let token: String = after
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != ';' && *c != ',')
        .collect();
    if token.is_empty() {
        None
    } else {
        Some(OwnerPhase::parse(&token))
    }
}

fn classify(line: &str) -> Option<SqlCallKind> {
    if line.contains("QueryBuilder::") || line.contains("sqlx::QueryBuilder") {
        return Some(SqlCallKind::QueryBuilder);
    }
    if line.contains("AssertSqlSafe(") {
        return Some(SqlCallKind::AssertSqlSafe);
    }
    if line.contains("MysqlIdentifier::from_allowlist") {
        return Some(SqlCallKind::MysqlIdentifier);
    }
    if line.contains("query!(") || line.contains("query_as!(") || line.contains("query_scalar!(") {
        return Some(SqlCallKind::CheckedMacro);
    }
    if line.contains("sqlx::query(")
        || line.contains("sqlx::query_as(")
        || line.contains("sqlx::query_scalar(")
    {
        return Some(SqlCallKind::DynamicBuilder);
    }
    None
}

/// Helper used by the test suite: returns the canonical path of the
/// workspace root used to scan production SQL.
pub fn workspace_root_hint() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
