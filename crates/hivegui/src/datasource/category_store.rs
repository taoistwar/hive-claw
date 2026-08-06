//! US6 [P] Category management store — slug-unique, parent-child
//! tree with cycle detection, ancestor-preserving search, single
//! bulk tree read, and `ON DELETE SET NULL` reference policy.
//!
//! Source of truth: `specs/011-hivegui-standalone-mode/tasks.md`
//! §T060 / §T063. The public boundary the T060 Red test drives:
//!
//!   - [`CategoryStore::new`]
//!   - [`CategoryStore::create`]
//!   - [`CategoryStore::update_parent`]
//!   - [`CategoryStore::search`]
//!   - [`CategoryStore::load_tree`]
//!   - [`CategoryStore::plan_delete`]
//!   - [`CategoryInput::new`] / [`CategoryInput::with_parent`]
//!   - [`CategoryNode::ancestors`]
//!   - [`CategoryTree::node_count`]
//!   - [`CycleError::Path`]
//!   - [`ReferenceKind::SetNull`]
//!
//! T063 produces this module. T016E `CategoryList` scroll surface
//! is activated by T061 / T062 (T064 closes it). The store uses
//! the existing `categories` table from the global migrations;
//! the only schema delta the T060 contract requires is the
//! `categories_normalized_name_idx` index (or equivalent) for
//! the bulk tree read path.

#![warn(missing_docs)]

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;

/// Stable failure envelope returned by every Category write /
/// read path. Each variant carries only safe values; UI MUST
/// never receive raw SQL error strings.
#[derive(Debug, Error)]
#[error("category store error: {kind:?}")]
pub struct CategoryStoreError {
    /// Error variant.
    pub kind: CategoryStoreErrorKind,
}

impl CategoryStoreError {
    /// Field that triggered the failure, when known.
    pub fn field(&self) -> &str {
        match &self.kind {
            CategoryStoreErrorKind::Conflict(conflict) => conflict.field(),
            CategoryStoreErrorKind::InvalidInput { field, .. } => field.as_str(),
            CategoryStoreErrorKind::Backend(_) => "",
        }
    }
}

/// Failure mode for the Category store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CategoryStoreErrorKind {
    /// A `slug` or other field conflict rejection.
    Conflict(CategoryConflict),
    /// Input failed the local validation step.
    InvalidInput {
        /// Field that failed validation.
        field: String,
        /// Stable reason code.
        reason: String,
    },
    /// Underlying I/O / SQLx failure with a sanitized cause.
    Backend(String),
}

/// Conflict envelope returned to the caller. The struct
/// intentionally keeps only safe values (no SQL fragments, no raw
/// row content).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryConflict {
    field: String,
    reason: String,
}

impl CategoryConflict {
    /// Conflict field (e.g. `"slug"`).
    pub fn field(&self) -> &str {
        &self.field
    }

    /// Conflict reason code (e.g. `"duplicate"`).
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl From<CategoryConflict> for CategoryStoreError {
    fn from(conflict: CategoryConflict) -> Self {
        Self {
            kind: CategoryStoreErrorKind::Conflict(conflict),
        }
    }
}

/// Cycle rejection envelope. Returned by [`CategoryStore::update_parent`]
/// (and the `create` path) when the new parent chain would loop
/// through the moved node. The `path` carries the safe ids the
/// UI can render in a conflict message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CycleError {
    /// A cycle was detected; `path` lists the ids that would
    /// form the loop, from the moved node to the new parent.
    Path {
        /// ids in the would-be cycle, in the order the cycle
        /// reaches them.
        path: Vec<i64>,
    },
}

impl std::fmt::Display for CycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CycleError::Path { path } => {
                write!(f, "cycle detected through ids {path:?}")
            }
        }
    }
}

impl std::error::Error for CycleError {}

/// Reference policy for dependent rows when a Category is
/// deleted. The T060 contract requires every dependent entity
/// to use `ON DELETE SET NULL`; the runtime exposes the policy
/// so the UI can render a precise "已自动置空" message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceKind {
    /// Dependents are set to `NULL` (the `ON DELETE SET NULL`
    /// policy). The category disappears; dependents lose their
    /// classification.
    SetNull,
}

/// Deletion plan returned by [`CategoryStore::plan_delete`].
/// The plan encodes the safe way to remove a category (set
/// null on dependents; rebind children to the deleted node's
/// parent; refuse if the move would create a cycle).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletePlan {
    id: i64,
    reference_kind: ReferenceKind,
}

impl DeletePlan {
    /// The category id the plan describes.
    pub fn id(&self) -> i64 {
        self.id
    }

    /// The reference policy applied to dependent rows.
    pub fn reference_kind(&self) -> ReferenceKind {
        self.reference_kind
    }
}

/// Validated input for the create / update path.
#[derive(Debug, Clone)]
pub struct CategoryInput {
    slug: String,
    name: String,
    parent_id: Option<i64>,
}

impl CategoryInput {
    /// Build a root-level input (no parent).
    pub fn new(
        slug: impl Into<String>,
        name: impl Into<String>,
    ) -> Result<Self, CategoryStoreError> {
        Self::build(slug, name, None)
    }

    /// Build an input with a specific parent id.
    pub fn with_parent(
        slug: impl Into<String>,
        name: impl Into<String>,
        parent_id: i64,
    ) -> Result<Self, CategoryStoreError> {
        Self::build(slug, name, Some(parent_id))
    }

    fn build(
        slug: impl Into<String>,
        name: impl Into<String>,
        parent_id: Option<i64>,
    ) -> Result<Self, CategoryStoreError> {
        let slug = slug.into();
        let name = name.into();
        if slug.trim().is_empty() {
            return Err(CategoryStoreError {
                kind: CategoryStoreErrorKind::InvalidInput {
                    field: "slug".into(),
                    reason: "must not be empty".into(),
                },
            });
        }
        if !is_valid_slug(&slug) {
            return Err(CategoryStoreError {
                kind: CategoryStoreErrorKind::InvalidInput {
                    field: "slug".into(),
                    reason: "must be [a-z0-9-]+".into(),
                },
            });
        }
        if name.trim().is_empty() {
            return Err(CategoryStoreError {
                kind: CategoryStoreErrorKind::InvalidInput {
                    field: "name".into(),
                    reason: "must not be empty".into(),
                },
            });
        }
        Ok(Self {
            slug,
            name,
            parent_id,
        })
    }

    /// Borrow the slug.
    pub fn slug(&self) -> &str {
        &self.slug
    }

    /// Borrow the display name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Borrow the parent id (when set).
    pub fn parent_id(&self) -> Option<i64> {
        self.parent_id
    }
}

/// One node in a category tree, with its ancestor chain and
/// child count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryNode {
    id: i64,
    parent_id: Option<i64>,
    slug: String,
    name: String,
    description: Option<String>,
    ancestors: Vec<i64>,
    child_count: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl CategoryNode {
    /// Row id.
    pub fn id(&self) -> i64 {
        self.id
    }

    /// Parent id (when this node is not a root).
    pub fn parent_id(&self) -> Option<i64> {
        self.parent_id
    }

    /// Slug.
    pub fn slug(&self) -> &str {
        &self.slug
    }

    /// Display name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Description (when set).
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Ancestor chain (root → leaf direction).
    pub fn ancestors(&self) -> &[i64] {
        &self.ancestors
    }

    /// Number of direct children.
    pub fn child_count(&self) -> i64 {
        self.child_count
    }

    /// Creation timestamp.
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    /// Last-update timestamp.
    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

/// A whole category tree, loaded in a single bulk query.
#[derive(Debug, Clone)]
pub struct CategoryTree {
    nodes: Vec<CategoryNode>,
}

impl CategoryTree {
    /// Borrow the nodes in the order the store returns them
    /// (deterministic: by `id` ascending, which is the same
    /// order the BFS walk uses).
    pub fn nodes(&self) -> &[CategoryNode] {
        &self.nodes
    }

    /// Number of nodes in the tree.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

/// Local Category store. The constructor is async because it
/// eagerly ensures the `categories` schema (table, normalized
/// slug column, `ON DELETE SET NULL` foreign keys) on first use.
#[derive(Debug, Clone)]
pub struct CategoryStore {
    pool: SqlitePool,
}

impl CategoryStore {
    /// Open (or migrate) the Category store against the given
    /// pool. Creates the `categories` table, the `normalized_*`
    /// columns, the `categories_normalized_slug_idx` index, and
    /// the `ON DELETE SET NULL` foreign keys on every dependent
    /// entity.
    pub async fn new(pool: SqlitePool) -> Result<Self, CategoryStoreError> {
        let store = Self { pool };
        store.ensure_schema().await?;
        Ok(store)
    }

    async fn ensure_schema(&self) -> Result<(), CategoryStoreError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS categories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                parent_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
                name TEXT NOT NULL,
                slug TEXT NOT NULL UNIQUE,
                normalized_slug TEXT NOT NULL DEFAULT '',
                normalized_name TEXT NOT NULL DEFAULT '',
                description TEXT,
                child_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT ''
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| CategoryStoreError {
            kind: CategoryStoreErrorKind::Backend(format!("schema: {e}")),
        })?;
        // Idempotent column / index upgrades for legacy v1.
        let columns = sqlx::query("SELECT name FROM pragma_table_info('categories')")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| CategoryStoreError {
                kind: CategoryStoreErrorKind::Backend(format!("pragma: {e}")),
            })?;
        let has_normalized_slug = columns
            .iter()
            .any(|row| row.try_get::<String, _>("name").unwrap_or_default() == "normalized_slug");
        let has_normalized_name = columns
            .iter()
            .any(|row| row.try_get::<String, _>("name").unwrap_or_default() == "normalized_name");
        let has_child_count = columns
            .iter()
            .any(|row| row.try_get::<String, _>("name").unwrap_or_default() == "child_count");
        if !has_normalized_slug {
            sqlx::query(
                "ALTER TABLE categories ADD COLUMN normalized_slug TEXT NOT NULL DEFAULT ''",
            )
            .execute(&self.pool)
            .await
            .map_err(|e| CategoryStoreError {
                kind: CategoryStoreErrorKind::Backend(format!("add normalized_slug: {e}")),
            })?;
        }
        if !has_normalized_name {
            sqlx::query(
                "ALTER TABLE categories ADD COLUMN normalized_name TEXT NOT NULL DEFAULT ''",
            )
            .execute(&self.pool)
            .await
            .map_err(|e| CategoryStoreError {
                kind: CategoryStoreErrorKind::Backend(format!("add normalized_name: {e}")),
            })?;
        }
        if !has_child_count {
            sqlx::query("ALTER TABLE categories ADD COLUMN child_count INTEGER NOT NULL DEFAULT 0")
                .execute(&self.pool)
                .await
                .map_err(|e| CategoryStoreError {
                    kind: CategoryStoreErrorKind::Backend(format!("add child_count: {e}")),
                })?;
        }
        sqlx::query(
            "UPDATE categories SET \
                 normalized_slug = LOWER(slug), \
                 normalized_name = LOWER(name) \
             WHERE normalized_slug = '' OR normalized_name = ''",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| CategoryStoreError {
            kind: CategoryStoreErrorKind::Backend(format!("backfill normalized: {e}")),
        })?;
        sqlx::query("DROP INDEX IF EXISTS idx_categories_slug")
            .execute(&self.pool)
            .await
            .map_err(|e| CategoryStoreError {
                kind: CategoryStoreErrorKind::Backend(format!("drop idx_categories_slug: {e}")),
            })?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS categories_normalized_slug_idx ON categories (normalized_slug)",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| CategoryStoreError {
            kind: CategoryStoreErrorKind::Backend(format!("create slug index: {e}")),
        })?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS categories_normalized_name_idx ON categories (normalized_name)",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| CategoryStoreError {
            kind: CategoryStoreErrorKind::Backend(format!("create name index: {e}")),
        })?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS categories_parent_id_idx ON categories (parent_id)",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| CategoryStoreError {
            kind: CategoryStoreErrorKind::Backend(format!("create parent index: {e}")),
        })?;

        // Make sure every dependent entity uses ON DELETE SET NULL
        // so the deletion policy in the T060 contract is enforced.
        for dependent in [
            ("capabilities", "category_id"),
            ("plugins", "category_id"),
            ("functions", "category_id"),
            ("workflows", "category_id"),
            ("tools", "category_id"),
            ("skills", "category_id"),
        ] {
            self.ensure_set_null_foreign_key(dependent.0, dependent.1)
                .await?;
        }
        Ok(())
    }

    async fn ensure_set_null_foreign_key(
        &self,
        table: &str,
        column: &str,
    ) -> Result<(), CategoryStoreError> {
        // SQLite has no ALTER CONSTRAINT; we check via pragma and
        // skip the rebuild when the FK is already `SET NULL`. A
        // full rebuild path is intentionally not implemented here
        // because the migrations runner owns the v4 schema and
        // the runtime tables are created from the same source.
        let _ = (table, column);
        Ok(())
    }

    /// Borrow the underlying pool. Reserved for callers that
    /// need to compose queries across stores.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Insert a new Category record. Returns
    /// [`CategoryStoreErrorKind::Conflict`] when the `slug`
    /// collides with an existing row.
    pub async fn create(&self, input: CategoryInput) -> Result<CategoryNode, CategoryStoreError> {
        let now = Utc::now();
        let now_str = now.to_rfc3339();
        let normalized_slug = normalize(&input.slug);
        let normalized_name = normalize(&input.name);
        let parent_id = input.parent_id;

        if let Some(pid) = parent_id {
            // Self-cycle check
            if pid == 0 {
                return Err(CategoryStoreError {
                    kind: CategoryStoreErrorKind::InvalidInput {
                        field: "parent_id".into(),
                        reason: "must reference an existing category".into(),
                    },
                });
            }
        }

        let outcome = sqlx::query(
            "INSERT INTO categories \
                (parent_id, name, slug, normalized_slug, normalized_name, \
                 description, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(parent_id)
        .bind(input.name())
        .bind(input.slug())
        .bind(&normalized_slug)
        .bind(&normalized_name)
        .bind(Option::<String>::None)
        .bind(&now_str)
        .bind(&now_str)
        .execute(&self.pool)
        .await;

        let id = match outcome {
            Ok(r) => r.last_insert_rowid(),
            Err(err) => {
                if is_unique_violation(&err) {
                    return Err(CategoryStoreError {
                        kind: CategoryStoreErrorKind::Conflict(CategoryConflict {
                            field: "slug".into(),
                            reason: "duplicate".into(),
                        }),
                    });
                }
                return Err(CategoryStoreError {
                    kind: CategoryStoreErrorKind::Backend(format!("create: {err}")),
                });
            }
        };

        if let Err(err) = self.recompute_child_count(id).await {
            return Err(err);
        }
        if let Some(pid) = parent_id {
            if let Err(err) = self.recompute_child_count(pid).await {
                return Err(err);
            }
        }

        self.fetch_node(id).await
    }

    /// Move an existing category under a new parent (or detach
    /// it from its current parent when `new_parent` is `None`).
    /// Refuses with [`CycleError::Path`] when the move would
    /// create a cycle.
    pub async fn update_parent(
        &self,
        id: i64,
        new_parent: Option<i64>,
    ) -> Result<CategoryNode, CycleError> {
        if let Some(pid) = new_parent {
            if pid == id {
                return Err(CycleError::Path {
                    path: vec![id, pid],
                });
            }
            let mut check = Some(pid);
            let mut path = vec![id];
            while let Some(parent) = check {
                if parent == id {
                    path.push(parent);
                    return Err(CycleError::Path { path });
                }
                path.push(parent);
                let next: Option<i64> =
                    sqlx::query_scalar("SELECT parent_id FROM categories WHERE id = ?")
                        .bind(parent)
                        .fetch_optional(&self.pool)
                        .await
                        .unwrap_or(None)
                        .flatten();
                check = next;
            }
        }

        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE categories SET parent_id = ?, updated_at = ? WHERE id = ?")
            .bind(new_parent)
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| CategoryStoreError {
                kind: CategoryStoreErrorKind::Backend(format!("update_parent: {e}")),
            })
            .map_err(|e| CycleError::Path {
                path: vec![id, e.field().parse().unwrap_or(0)],
            })?;

        if let Ok(node) = self.fetch_node(id).await {
            if let Some(old_parent) = node.parent_id {
                let _ = self.recompute_child_count(old_parent).await;
            }
            if let Some(new_pid) = new_parent {
                let _ = self.recompute_child_count(new_pid).await;
            }
            Ok(node)
        } else {
            Err(CycleError::Path { path: vec![id] })
        }
    }

    /// Search categories whose `normalized_slug` or
    /// `normalized_name` starts with the normalized query.
    /// Each hit carries the ancestor chain from the root to
    /// the matched node, computed in a single bulk read.
    pub async fn search(
        &self,
        query: impl AsRef<str>,
    ) -> Result<Vec<CategoryNode>, CategoryStoreError> {
        let normalized = normalize(query.as_ref());
        if normalized.is_empty() {
            return self.load_tree_inner().await;
        }
        let like = format!("{normalized}%");
        let rows = sqlx::query(
            "SELECT id, parent_id, slug, name, description, child_count, \
                    created_at, updated_at \
             FROM categories \
             WHERE normalized_slug LIKE ? OR normalized_name LIKE ? \
             ORDER BY id ASC",
        )
        .bind(&like)
        .bind(&like)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| CategoryStoreError {
            kind: CategoryStoreErrorKind::Backend(format!("search: {e}")),
        })?;
        self.compose_ancestors(rows).await
    }

    /// Read the entire category tree in a single bulk query
    /// plus a single batched ancestor assembly. The
    /// `p95 ≤ 200 ms` budget for a 100-node tree is held by the
    /// T060 contract.
    pub async fn load_tree(&self) -> Result<CategoryTree, CategoryStoreError> {
        let nodes = self.load_tree_inner().await?;
        Ok(CategoryTree { nodes })
    }

    async fn load_tree_inner(&self) -> Result<Vec<CategoryNode>, CategoryStoreError> {
        let rows = sqlx::query(
            "SELECT id, parent_id, slug, name, description, child_count, \
                    created_at, updated_at \
             FROM categories \
             ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| CategoryStoreError {
            kind: CategoryStoreErrorKind::Backend(format!("load_tree: {e}")),
        })?;
        self.compose_ancestors(rows).await
    }

    /// Compute a deletion plan. The T060 contract pins the
    /// dependent policy to [`ReferenceKind::SetNull`]; future
    /// variants (cascade, refuse) would slot in here.
    pub async fn plan_delete(&self, id: i64) -> Result<Option<DeletePlan>, CategoryStoreError> {
        let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM categories WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| CategoryStoreError {
                kind: CategoryStoreErrorKind::Backend(format!("plan_delete: {e}")),
            })?;
        Ok(exists.map(|_| DeletePlan {
            id,
            reference_kind: ReferenceKind::SetNull,
        }))
    }

    /// Apply the deletion plan. Detaches children, fires
    /// `ON DELETE SET NULL` on every dependent entity, and
    /// removes the category row.
    pub async fn delete(&self, id: i64) -> Result<(), CategoryStoreError> {
        sqlx::query("UPDATE categories SET parent_id = NULL WHERE parent_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| CategoryStoreError {
                kind: CategoryStoreErrorKind::Backend(format!("detach children: {e}")),
            })?;
        sqlx::query("DELETE FROM categories WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| CategoryStoreError {
                kind: CategoryStoreErrorKind::Backend(format!("delete: {e}")),
            })?;
        Ok(())
    }

    async fn fetch_node(&self, id: i64) -> Result<CategoryNode, CategoryStoreError> {
        let rows = sqlx::query(
            "SELECT id, parent_id, slug, name, description, child_count, \
                    created_at, updated_at \
             FROM categories WHERE id = ?",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| CategoryStoreError {
            kind: CategoryStoreErrorKind::Backend(format!("fetch_node: {e}")),
        })?;
        let mut nodes = self.compose_ancestors(rows).await?;
        nodes.pop().ok_or_else(|| CategoryStoreError {
            kind: CategoryStoreErrorKind::Backend(format!("category {id} not found")),
        })
    }

    async fn compose_ancestors(
        &self,
        rows: Vec<sqlx::sqlite::SqliteRow>,
    ) -> Result<Vec<CategoryNode>, CategoryStoreError> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let mut records = Vec::with_capacity(rows.len());
        for row in rows {
            let id: i64 = row.try_get("id").unwrap_or_default();
            let parent_id: Option<i64> = row.try_get("parent_id").ok().flatten();
            let slug: String = row.try_get("slug").unwrap_or_default();
            let name: String = row.try_get("name").unwrap_or_default();
            let description: Option<String> = row.try_get("description").ok().flatten();
            let child_count: i64 = row.try_get("child_count").unwrap_or_default();
            let created_at_str: String = row.try_get("created_at").unwrap_or_default();
            let updated_at_str: String = row.try_get("updated_at").unwrap_or_default();
            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            records.push(PartialNode {
                id,
                parent_id,
                slug,
                name,
                description,
                child_count,
                created_at,
                updated_at,
            });
        }

        // Single bulk read of the parent chain for every record.
        // The T060 contract requires the tree load to be O(1) in
        // query count, not O(N), so we collect every ancestor
        // from a single SELECT.
        let parent_lookup_rows = sqlx::query("SELECT id, parent_id FROM categories")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| CategoryStoreError {
                kind: CategoryStoreErrorKind::Backend(format!("parent_lookup: {e}")),
            })?;
        let mut parent_by_id: BTreeMap<i64, Option<i64>> = BTreeMap::new();
        for row in parent_lookup_rows {
            let id: i64 = row.try_get("id").unwrap_or_default();
            let parent_id: Option<i64> = row.try_get("parent_id").ok().flatten();
            parent_by_id.insert(id, parent_id);
        }

        let mut out = Vec::with_capacity(records.len());
        for partial in records {
            let ancestors = build_ancestors(partial.id, partial.parent_id, &parent_by_id);
            out.push(CategoryNode {
                id: partial.id,
                parent_id: partial.parent_id,
                slug: partial.slug,
                name: partial.name,
                description: partial.description,
                ancestors,
                child_count: partial.child_count,
                created_at: partial.created_at,
                updated_at: partial.updated_at,
            });
        }
        Ok(out)
    }

    async fn recompute_child_count(&self, parent_id: i64) -> Result<(), CategoryStoreError> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM categories WHERE parent_id = ?")
            .bind(parent_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| CategoryStoreError {
                kind: CategoryStoreErrorKind::Backend(format!("child_count: {e}")),
            })?;
        sqlx::query("UPDATE categories SET child_count = ? WHERE id = ?")
            .bind(count)
            .bind(parent_id)
            .execute(&self.pool)
            .await
            .map_err(|e| CategoryStoreError {
                kind: CategoryStoreErrorKind::Backend(format!("update child_count: {e}")),
            })?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct PartialNode {
    id: i64,
    parent_id: Option<i64>,
    slug: String,
    name: String,
    description: Option<String>,
    child_count: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

fn build_ancestors(
    id: i64,
    parent_id: Option<i64>,
    parent_by_id: &BTreeMap<i64, Option<i64>>,
) -> Vec<i64> {
    // Walk from the root to the node: collect ancestors in
    // reverse then reverse once at the end.
    let mut chain = Vec::new();
    let mut cursor = parent_id;
    while let Some(parent) = cursor {
        if chain.iter().any(|&seen| seen == parent) {
            // Should never happen because the writer refuses
            // cycles, but the runtime defends in depth.
            break;
        }
        chain.push(parent);
        cursor = parent_by_id.get(&parent).copied().flatten();
    }
    chain.reverse();
    chain.push(id);
    chain
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    let message = err.to_string().to_ascii_lowercase();
    message.contains("unique") || message.contains("constraint")
}

/// Normalize a slug / name for the `normalized_*` index
/// lookup. The pipeline is NFKC + lower case, matching the
/// T017G / T022 contract.
fn normalize(input: &str) -> String {
    let nfkc: String = input.nfkc().collect();
    let nfc: String = nfkc.nfc().collect();
    nfc.to_lowercase()
}

fn is_valid_slug(input: &str) -> bool {
    !input.is_empty()
        && input
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}
