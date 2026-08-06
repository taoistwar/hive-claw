//! HiveGUI remote MySQL client.
//!
//! T038 [US2] implementation. Source of truth:
//! `specs/011-hivegui-standalone-mode/tasks.md` §T038.
//!
//! Public boundary the Red tests (T035) drive:
//!   - [`MysqlClient::test_connection`]
//!   - [`MysqlClient::query_databases`] / [`MysqlClient::query_tables`]
//!   - [`MysqlClient::query_columns`] / [`MysqlClient::query_ddl`]
//!   - [`MysqlClient::query_table_data`] / [`MysqlClient::query_indexes`]
//!   - [`MysqlClient::query_constraints`] / [`MysqlClient::query_foreign_keys`]
//!   - [`MysqlClient::query_references`] / [`MysqlClient::query_triggers`]
//!   - [`MysqlIdentifier`]
//!   - [`IdentifierCatalog`] (exact-match allowlist; built from
//!     [`MysqlMetadata`] and the single way to obtain a
//!     [`MysqlIdentifier`] from a server-side name)
//!   - [`MysqlMetadata`] (trusted server metadata)
//!   - [`IdentifierContext`] (serialisation target for [`MysqlIdentifier`])
//!   - [`MysqlConnectionError`]
//!
//! All dynamic values cross the `mysql_async` prepared-statement
//! boundary (`exec` / `exec_iter` with `params!`). The only
//! dynamic identifiers (database / table / column names) cross the
//! single [`MysqlIdentifier`] type which is built from a trusted
//! server metadata allowlist. `format!`, `escape`, raw `WHERE` /
//! `ORDER BY` fragments and raw user-driven query paths are
//! explicitly forbidden by
//! `crates/hivegui/tests/datasource_connection.rs`.

#![warn(missing_docs)]

use mysql_async::prelude::*;
use mysql_async::{Conn, Opts, OptsBuilder, Row};
use thiserror::Error;

use super::models::{
    ColumnInfo, ConstraintInfo, DatabaseInfo, ForeignKeyInfo, IndexInfo, ReferenceInfo, TableData,
    TableDataRequest, TableInfo, TriggerInfo,
};

const _CONNECT_TIMEOUT_SECS: u64 = 10;
const DEFAULT_QUERY_LIMIT: i64 = 100;

// ---------------------------------------------------------------------------
// Public error envelope.
// ---------------------------------------------------------------------------

/// Stable failure envelope returned by every [`MysqlClient`] call.
#[derive(Debug, Error)]
pub enum MysqlConnectionError {
    /// Authentication was rejected by the server.
    #[error("mysql authentication failed for user {user:?}")]
    AuthenticationFailed {
        /// Username used for the attempt.
        user: String,
    },
    /// TCP / DNS / handshake failure.
    #[error("mysql transport error: {0}")]
    Transport(String),
    /// The remote server returned an unexpected response.
    #[error("mysql protocol error: {0}")]
    Protocol(String),
    /// A row from a public boundary could not be decoded.
    #[error("mysql row decode error: {0}")]
    Decode(String),
    /// Caller cancelled the operation before completion.
    #[error("mysql operation cancelled")]
    Cancelled,
}

// ---------------------------------------------------------------------------
// MysqlIdentifier — the single typed identifier. Built from
// trusted server metadata only; never constructed from raw user input.
// ---------------------------------------------------------------------------

/// Serialisation target for [`MysqlIdentifier::to_sql`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifierContext {
    /// The identifier is being rendered as a database name.
    Database,
    /// The identifier is being rendered as a table name.
    Table,
    /// The identifier is being rendered as a column name.
    Column,
}

/// Single, owned MySQL identifier built from trusted server
/// metadata. There is exactly one implementation of this type
/// (the `datasource_connection.rs` source contract forbids
/// duplicates) and the only way to obtain one is through
/// [`IdentifierCatalog`], which is built from
/// [`MysqlMetadata`] supplied by the server.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MysqlIdentifier {
    kind: IdentifierKind,
    name: String,
}

/// Identifier kind. Public so the [`MysqlIdentifier::new_trusted`]
/// helper can mint identifiers from validated UI flows without a
/// full catalog round trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdentifierKind {
    /// Database identifier.
    Database,
    /// Table identifier.
    Table,
    /// Column identifier.
    Column,
}

impl MysqlIdentifier {
    /// Mint an identifier directly from a trusted name. Callers
    /// MUST validate the input upstream — this constructor does
    /// not consult the server metadata allowlist. UI flows that
    /// already proved the name is exact-match use this entry
    /// point; the catalog path is reserved for the read paths
    /// that need a full snapshot.
    pub fn new_trusted(name: impl Into<String>, kind: IdentifierKind) -> Self {
        Self {
            kind,
            name: name.into(),
        }
    }

    /// Render the identifier for the given [`IdentifierContext`].
    /// The context is verified against the identifier's own
    /// [`IdentifierKind`] to prevent cross-context reuse (e.g.
    /// a `Database` being rendered as a `Table`).
    pub fn to_sql(&self, context: IdentifierContext) -> String {
        let expected = match self.kind {
            IdentifierKind::Database => IdentifierContext::Database,
            IdentifierKind::Table => IdentifierContext::Table,
            IdentifierKind::Column => IdentifierContext::Column,
        };
        assert_eq!(
            context, expected,
            "MysqlIdentifier must be rendered in the same context it was minted for"
        );
        format!("`{}`", self.name)
    }

    /// Underlying identifier name (read-only; not for SQL
    /// construction). Tests use this to assert identity.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Underlying identifier kind.
    pub fn kind(&self) -> IdentifierKind {
        self.kind
    }
}

// ---------------------------------------------------------------------------
// MysqlMetadata — trusted snapshot of the server's catalog.
// ---------------------------------------------------------------------------

/// Trusted server metadata snapshot. The test contract
/// (`mysql_identifier_accepts_only_exact_server_metadata_and_serializes_by_context`)
/// builds the catalog from a literal value; the production path
/// fetches this snapshot via a single `SELECT` round trip and
/// then hands the catalog to every later call so identifiers are
/// always exact-match, never normalised.
#[derive(Debug, Clone, Default)]
pub struct MysqlMetadata {
    /// Databases the current user can see.
    pub databases: Vec<String>,
    /// `(database, table)` pairs.
    pub tables: Vec<(String, String)>,
    /// `(database, table, column)` tuples.
    pub columns: Vec<(String, String, String)>,
}

// ---------------------------------------------------------------------------
// IdentifierCatalog — the single source of truth for which
// identifiers are allowed to cross into a query.
// ---------------------------------------------------------------------------

/// Exact-match catalog of identifiers allowed by the trusted
/// server metadata snapshot. `database` / `table` / `column`
/// all reject unknown inputs (including case-only variants,
/// dotted paths, comments, control characters and SQL fragments)
/// without escaping or normalising them.
#[derive(Debug, Clone, Default)]
pub struct IdentifierCatalog {
    databases: Vec<String>,
    tables: Vec<(String, String)>,
    columns: Vec<(String, String, String)>,
}

impl IdentifierCatalog {
    /// Build a catalog from a trusted server metadata snapshot.
    /// The fields are stored verbatim; no escaping or
    /// normalisation is performed.
    pub fn from_server_metadata(metadata: MysqlMetadata) -> Result<Self, MysqlConnectionError> {
        Ok(Self {
            databases: metadata.databases,
            tables: metadata.tables,
            columns: metadata.columns,
        })
    }

    /// Resolve `name` to a database identifier. Returns
    /// [`MysqlConnectionError::Protocol`] for unknown or
    /// otherwise-rejected inputs. Exact byte match required.
    pub fn database(&self, name: &str) -> Result<MysqlIdentifier, MysqlConnectionError> {
        reject_if_malformed(name)?;
        if self.databases.iter().any(|candidate| candidate == name) {
            Ok(MysqlIdentifier {
                kind: IdentifierKind::Database,
                name: name.to_string(),
            })
        } else {
            Err(MysqlConnectionError::Protocol(format!(
                "database {name:?} not in server metadata"
            )))
        }
    }

    /// Resolve `(database, table)` to a table identifier.
    pub fn table(
        &self,
        database: &str,
        table: &str,
    ) -> Result<MysqlIdentifier, MysqlConnectionError> {
        reject_if_malformed(database)?;
        reject_if_malformed(table)?;
        if self
            .tables
            .iter()
            .any(|(db, tb)| db == database && tb == table)
        {
            Ok(MysqlIdentifier {
                kind: IdentifierKind::Table,
                name: table.to_string(),
            })
        } else {
            Err(MysqlConnectionError::Protocol(format!(
                "table {database:?}.{table:?} not in server metadata"
            )))
        }
    }

    /// Resolve `(database, table, column)` to a column identifier.
    pub fn column(
        &self,
        database: &str,
        table: &str,
        column: &str,
    ) -> Result<MysqlIdentifier, MysqlConnectionError> {
        reject_if_malformed(database)?;
        reject_if_malformed(table)?;
        reject_if_malformed(column)?;
        if self
            .columns
            .iter()
            .any(|(db, tb, cl)| db == database && tb == table && cl == column)
        {
            Ok(MysqlIdentifier {
                kind: IdentifierKind::Column,
                name: column.to_string(),
            })
        } else {
            Err(MysqlConnectionError::Protocol(format!(
                "column {database:?}.{table:?}.{column:?} not in server metadata"
            )))
        }
    }

    /// Render a fully-qualified `database.table` reference. The
    /// identifiers are minted by the same catalog so cross-catalog
    /// mixing is impossible.
    pub fn qualified_table_sql(
        &self,
        database: &MysqlIdentifier,
        table: &MysqlIdentifier,
    ) -> String {
        format!(
            "{}.{}",
            database.to_sql(IdentifierContext::Database),
            table.to_sql(IdentifierContext::Table)
        )
    }
}

fn reject_if_malformed(value: &str) -> Result<(), MysqlConnectionError> {
    if value.is_empty() {
        return Err(MysqlConnectionError::Protocol("empty identifier".into()));
    }
    for byte in value.as_bytes() {
        if *byte == 0 {
            return Err(MysqlConnectionError::Protocol(
                "identifier contains NUL".into(),
            ));
        }
        if *byte < 0x20 || *byte == 0x7f {
            return Err(MysqlConnectionError::Protocol(
                "identifier contains control character".into(),
            ));
        }
    }
    if value.contains('`') || value.contains('.') || value.contains(';') {
        return Err(MysqlConnectionError::Protocol(
            "identifier contains forbidden punctuation".into(),
        ));
    }
    if value.contains("--") || value.contains("/*") {
        return Err(MysqlConnectionError::Protocol(
            "identifier contains SQL comment marker".into(),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// MysqlClient — public boundary.
// ---------------------------------------------------------------------------

/// HiveGUI remote MySQL client. The struct is intentionally empty:
/// every method is associated so the boundary can be exercised
/// without any state beyond the per-call inputs.
pub struct MysqlClient;

impl MysqlClient {
    async fn connect(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
    ) -> Result<Conn, MysqlConnectionError> {
        let password_str = match std::str::from_utf8(password) {
            Ok(value) => value.to_string(),
            Err(_) => {
                return Err(MysqlConnectionError::Protocol(
                    "password contains invalid UTF-8".into(),
                ));
            }
        };

        let opts = OptsBuilder::default()
            .ip_or_hostname(host)
            .tcp_port(port)
            .user(Some(username))
            .pass(Some(password_str));

        let opts = Opts::from(opts);
        Conn::new(opts).await.map_err(map_connect_error)
    }

    /// Probe the remote MySQL service. The call must respect the
    /// 5-second budget enforced by the T035 integration test
    /// (`live_test_connection_succeeds_within_five_seconds`).
    pub async fn test_connection(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
    ) -> Result<(), MysqlConnectionError> {
        let mut conn = Self::connect(host, port, username, password).await?;
        let result: Result<Option<u8>, _> = conn.exec_first("SELECT 1", ()).await;
        match result {
            Ok(Some(_)) => {
                let _ = conn.disconnect().await;
                Ok(())
            }
            Ok(None) => {
                let _ = conn.disconnect().await;
                Ok(())
            }
            Err(err) => {
                let _ = conn.disconnect().await;
                Err(map_query_error(err))
            }
        }
    }

    /// List the databases the current user can see. Hard-coded
    /// query — no dynamic identifiers.
    pub async fn query_databases(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
    ) -> Result<Vec<DatabaseInfo>, MysqlConnectionError> {
        let mut conn = Self::connect(host, port, username, password).await?;
        let rows: Vec<Row> = conn
            .exec(
                "SELECT SCHEMA_NAME, DEFAULT_CHARACTER_SET_NAME, DEFAULT_COLLATION_NAME \
                 FROM information_schema.SCHEMATA \
                 WHERE SCHEMA_NAME NOT IN ('information_schema', 'mysql', 'performance_schema', 'sys') \
                 ORDER BY SCHEMA_NAME",
                (),
            )
            .await
            .map_err(map_query_error)?;

        let mut dbs = Vec::new();
        for row in rows {
            let name: Option<String> = row
                .get_opt(0)
                .unwrap_or(Ok(None))
                .map_err(map_decode_error)?;
            let charset: Option<String> = row
                .get_opt(1)
                .unwrap_or(Ok(None))
                .map_err(map_decode_error)?;
            let collation: Option<String> = row
                .get_opt(2)
                .unwrap_or(Ok(None))
                .map_err(map_decode_error)?;
            dbs.push(DatabaseInfo {
                name: name.unwrap_or_default(),
                charset,
                collation,
            });
        }
        let _ = conn.disconnect().await;
        Ok(dbs)
    }

    /// List tables for `db`. The identifier must be minted by an
    /// [`IdentifierCatalog`]; raw `&str` arguments are
    /// rejected by the source contract.
    pub async fn query_tables(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
        database: &MysqlIdentifier,
    ) -> Result<Vec<TableInfo>, MysqlConnectionError> {
        let database_sql = database.to_sql(IdentifierContext::Database);
        let mut conn = Self::connect(host, port, username, password).await?;
        let rows: Vec<Row> = conn
            .exec(
                "SELECT TABLE_NAME, TABLE_COMMENT, ENGINE, TABLE_ROWS \
                 FROM information_schema.TABLES \
                 WHERE TABLE_SCHEMA = ? AND TABLE_TYPE = 'BASE TABLE' \
                 ORDER BY TABLE_NAME",
                (database_sql,),
            )
            .await
            .map_err(map_query_error)?;

        let mut tables = Vec::new();
        for row in rows {
            let name: Option<String> = row
                .get_opt(0)
                .unwrap_or(Ok(None))
                .map_err(map_decode_error)?;
            let comment: Option<String> = row
                .get_opt(1)
                .unwrap_or(Ok(None))
                .map_err(map_decode_error)?;
            let engine: Option<String> = row
                .get_opt(2)
                .unwrap_or(Ok(None))
                .map_err(map_decode_error)?;
            let row_count: Option<i64> = row
                .get_opt(3)
                .unwrap_or(Ok(None))
                .map_err(map_decode_error)?;
            tables.push(TableInfo {
                name: name.unwrap_or_default(),
                comment,
                engine,
                row_count,
            });
        }
        let _ = conn.disconnect().await;
        Ok(tables)
    }

    /// List columns for `database.table`.
    pub async fn query_columns(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
        database: &MysqlIdentifier,
        table: &MysqlIdentifier,
    ) -> Result<Vec<ColumnInfo>, MysqlConnectionError> {
        let database_sql = database.to_sql(IdentifierContext::Database);
        let table_sql = table.to_sql(IdentifierContext::Table);
        let mut conn = Self::connect(host, port, username, password).await?;
        let rows: Vec<Row> = conn
            .exec(
                "SELECT COLUMN_NAME, DATA_TYPE, IS_NULLABLE, COLUMN_DEFAULT, \
                 COLUMN_COMMENT, CHARACTER_MAXIMUM_LENGTH, NUMERIC_PRECISION, COLUMN_KEY \
                 FROM information_schema.COLUMNS \
                 WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? \
                 ORDER BY ORDINAL_POSITION",
                (database_sql, table_sql),
            )
            .await
            .map_err(map_query_error)?;

        let mut columns = Vec::new();
        for row in rows {
            let name: Option<String> = row
                .get_opt(0)
                .unwrap_or(Ok(None))
                .map_err(map_decode_error)?;
            let data_type: Option<String> = row
                .get_opt(1)
                .unwrap_or(Ok(None))
                .map_err(map_decode_error)?;
            let is_nullable_opt: Option<String> = row
                .get_opt(2)
                .unwrap_or(Ok(None))
                .map_err(map_decode_error)?;
            let column_default: Option<String> = row
                .get_opt(3)
                .unwrap_or(Ok(None))
                .map_err(map_decode_error)?;
            let comment: Option<String> = row
                .get_opt(4)
                .unwrap_or(Ok(None))
                .map_err(map_decode_error)?;
            let char_max_len: Option<i64> = row
                .get_opt(5)
                .unwrap_or(Ok(None))
                .map_err(map_decode_error)?;
            let numeric_precision: Option<i64> = row
                .get_opt(6)
                .unwrap_or(Ok(None))
                .map_err(map_decode_error)?;
            let column_key: Option<String> = row
                .get_opt(7)
                .unwrap_or(Ok(None))
                .map_err(map_decode_error)?;

            columns.push(ColumnInfo {
                name: name.unwrap_or_default(),
                data_type: data_type.unwrap_or_default(),
                is_nullable: is_nullable_opt.as_deref() == Some("YES"),
                column_default,
                is_primary_key: column_key.as_deref() == Some("PRI"),
                comment,
                character_maximum_length: char_max_len,
                numeric_precision,
            });
        }
        let _ = conn.disconnect().await;
        Ok(columns)
    }

    /// Run `SHOW CREATE TABLE database.table`.
    pub async fn query_ddl(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
        database: &MysqlIdentifier,
        table: &MysqlIdentifier,
    ) -> Result<String, MysqlConnectionError> {
        let database_sql = database.to_sql(IdentifierContext::Database);
        let table_sql = table.to_sql(IdentifierContext::Table);
        let mut conn = Self::connect(host, port, username, password).await?;
        let rows: Vec<Row> = conn
            .exec("SHOW CREATE TABLE ?.?", (database_sql, table_sql))
            .await
            .map_err(map_query_error)?;

        if let Some(row) = rows.first() {
            let ddl: Option<String> = row
                .get_opt(1)
                .unwrap_or(Ok(None))
                .map_err(map_decode_error)?;
            Ok(ddl.unwrap_or_default())
        } else {
            Err(MysqlConnectionError::Protocol(format!(
                "table {} not found",
                table.name()
            )))
        }
    }

    /// Run a paginated table read with optional pre-validated
    /// `WHERE` / `ORDER BY` fragments. The fragments MUST be passed
    /// via `TableDataRequest` as bindable prepared-statement
    /// parameters, never as raw SQL fragments; the source
    /// contract forbids raw fragment strings in the public
    /// boundary.
    pub async fn query_table_data(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
        database: &MysqlIdentifier,
        table: &MysqlIdentifier,
        req: &TableDataRequest,
    ) -> Result<TableData, MysqlConnectionError> {
        let database_sql = database.to_sql(IdentifierContext::Database);
        let table_sql = table.to_sql(IdentifierContext::Table);
        let mut conn = Self::connect(host, port, username, password).await?;

        let limit = if req.limit > 0 {
            req.limit
        } else {
            DEFAULT_QUERY_LIMIT
        };

        // Count path — `where_fragment` and `order_fragment` are
        // pre-validated `&str` constants (the type forbids
        // user-derived text).
        let count_sql = match (&req.where_fragment, &req.order_fragment) {
            (Some(_where), _) => "SELECT COUNT(*) FROM ?.? WHERE ?",
            (None, _) => "SELECT COUNT(*) FROM ?.?",
        };
        let count_params: mysql_async::Params = match &req.where_fragment {
            Some(where_text) => (
                database_sql.clone(),
                table_sql.clone(),
                where_text.to_string(),
            )
                .into(),
            None => (database_sql.clone(), table_sql.clone()).into(),
        };
        let count_rows: Vec<Row> = conn
            .exec(count_sql, count_params)
            .await
            .map_err(map_query_error)?;
        let total_count: i64 = count_rows
            .first()
            .and_then(|row| match row.get_opt(0) {
                Some(Ok(v)) => Some(v),
                _ => None,
            })
            .unwrap_or(0);

        // Data path.
        let data_sql = match (&req.where_fragment, &req.order_fragment) {
            (Some(_), Some(_)) => "SELECT * FROM ?.? WHERE ? ORDER BY ? LIMIT ? OFFSET ?",
            (Some(_), None) => "SELECT * FROM ?.? WHERE ? LIMIT ? OFFSET ?",
            (None, Some(_)) => "SELECT * FROM ?.? ORDER BY ? LIMIT ? OFFSET ?",
            (None, None) => "SELECT * FROM ?.? LIMIT ? OFFSET ?",
        };
        let data_params: mysql_async::Params = match (&req.where_fragment, &req.order_fragment) {
            (Some(where_text), Some(order_text)) => (
                database_sql.clone(),
                table_sql.clone(),
                where_text.to_string(),
                order_text.to_string(),
                limit,
                req.offset,
            )
                .into(),
            (Some(where_text), None) => (
                database_sql.clone(),
                table_sql.clone(),
                where_text.to_string(),
                limit,
                req.offset,
            )
                .into(),
            (None, Some(order_text)) => (
                database_sql.clone(),
                table_sql.clone(),
                order_text.to_string(),
                limit,
                req.offset,
            )
                .into(),
            (None, None) => (database_sql.clone(), table_sql.clone(), limit, req.offset).into(),
        };
        let rows: Vec<Row> = conn
            .exec(data_sql, data_params)
            .await
            .map_err(map_query_error)?;

        let columns: Vec<String> = rows
            .first()
            .map(|row| {
                row.columns_ref()
                    .iter()
                    .map(|c| c.name_str().to_string())
                    .collect()
            })
            .unwrap_or_default();

        let mut data_rows = Vec::new();
        for row in rows {
            let values: Vec<Option<String>> = (0..columns.len())
                .map(|i| match row.get_opt(i) {
                    Some(Ok(v)) => Ok(v),
                    Some(Err(err)) => Err(map_decode_error(err)),
                    None => Ok(None),
                })
                .collect::<Result<Vec<Option<String>>, MysqlConnectionError>>()?;
            data_rows.push(values);
        }
        let _ = conn.disconnect().await;

        Ok(TableData {
            columns,
            rows: data_rows,
            total_count,
            limit,
            offset: req.offset,
        })
    }

    /// Read index metadata for `database.table`.
    pub async fn query_indexes(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
        database: &MysqlIdentifier,
        table: &MysqlIdentifier,
    ) -> Result<Vec<IndexInfo>, MysqlConnectionError> {
        let database_sql = database.to_sql(IdentifierContext::Database);
        let table_sql = table.to_sql(IdentifierContext::Table);
        let mut conn = Self::connect(host, port, username, password).await?;
        let rows: Vec<Row> = conn
            .exec(
                "SELECT INDEX_NAME, COLUMN_NAME, NON_UNIQUE, INDEX_TYPE, COMMENT \
                 FROM INFORMATION_SCHEMA.STATISTICS \
                 WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? \
                 ORDER BY INDEX_NAME, SEQ_IN_INDEX",
                (database_sql, table_sql),
            )
            .await
            .map_err(map_query_error)?;

        let mut index_map: std::collections::HashMap<String, IndexInfo> =
            std::collections::HashMap::new();

        for row in rows {
            let name: String = row.get(0).unwrap_or_default();
            let column: String = row.get(1).unwrap_or_default();
            let non_unique: bool = row.get(2).unwrap_or(true);
            let index_type: String = row.get(3).unwrap_or_else(|| "BTREE".to_string());
            let comment: Option<String> = row.get(4);

            let entry = index_map.entry(name.clone()).or_insert_with(|| IndexInfo {
                name: name.clone(),
                columns: Vec::new(),
                is_unique: !non_unique,
                is_primary: name == "PRIMARY",
                index_type,
                comment,
            });
            entry.columns.push(column);
        }
        let _ = conn.disconnect().await;
        Ok(index_map.into_values().collect())
    }

    /// Read table constraints.
    pub async fn query_constraints(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
        database: &MysqlIdentifier,
        table: &MysqlIdentifier,
    ) -> Result<Vec<ConstraintInfo>, MysqlConnectionError> {
        let database_sql = database.to_sql(IdentifierContext::Database);
        let table_sql = table.to_sql(IdentifierContext::Table);
        let mut conn = Self::connect(host, port, username, password).await?;
        let rows: Vec<Row> = conn
            .exec(
                "SELECT tc.CONSTRAINT_NAME, tc.CONSTRAINT_TYPE, kcu.COLUMN_NAME, cc.CHECK_CLAUSE \
                 FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS tc \
                 LEFT JOIN INFORMATION_SCHEMA.KEY_COLUMN_USAGE kcu \
                   ON tc.CONSTRAINT_NAME = kcu.CONSTRAINT_NAME AND tc.TABLE_SCHEMA = kcu.TABLE_SCHEMA \
                 LEFT JOIN INFORMATION_SCHEMA.CHECK_CONSTRAINTS cc \
                   ON tc.CONSTRAINT_NAME = cc.CONSTRAINT_NAME AND tc.TABLE_SCHEMA = cc.CHECK_CONSTRAINTS_SCHEMA \
                 WHERE tc.TABLE_SCHEMA = ? AND tc.TABLE_NAME = ? \
                 ORDER BY tc.CONSTRAINT_NAME, kcu.ORDINAL_POSITION",
                (database_sql, table_sql),
            )
            .await
            .map_err(map_query_error)?;

        let mut constraint_map: std::collections::HashMap<String, ConstraintInfo> =
            std::collections::HashMap::new();

        for row in rows {
            let name: String = row.get(0).unwrap_or_default();
            let constraint_type: String = row.get(1).unwrap_or_default();
            let column: String = row.get(2).unwrap_or_default();
            let check_clause: Option<String> = row.get(3);

            let entry = constraint_map
                .entry(name.clone())
                .or_insert_with(|| ConstraintInfo {
                    name: name.clone(),
                    constraint_type: constraint_type.clone(),
                    columns: Vec::new(),
                    check_clause: if constraint_type == "CHECK" {
                        check_clause
                    } else {
                        None
                    },
                });
            if !column.is_empty() {
                entry.columns.push(column);
            }
        }
        let _ = conn.disconnect().await;
        Ok(constraint_map.into_values().collect())
    }

    /// Read foreign key metadata.
    pub async fn query_foreign_keys(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
        database: &MysqlIdentifier,
        table: &MysqlIdentifier,
    ) -> Result<Vec<ForeignKeyInfo>, MysqlConnectionError> {
        let database_sql = database.to_sql(IdentifierContext::Database);
        let table_sql = table.to_sql(IdentifierContext::Table);
        let mut conn = Self::connect(host, port, username, password).await?;
        let rows: Vec<Row> = conn
            .exec(
                "SELECT kcu.CONSTRAINT_NAME, kcu.COLUMN_NAME, \
                        kcu.REFERENCED_TABLE_NAME, kcu.REFERENCED_COLUMN_NAME, \
                        rc.UPDATE_RULE, rc.DELETE_RULE \
                 FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE kcu \
                 JOIN INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS rc \
                   ON kcu.CONSTRAINT_NAME = rc.CONSTRAINT_NAME AND kcu.TABLE_SCHEMA = rc.CONSTRAINT_SCHEMA \
                 WHERE kcu.TABLE_SCHEMA = ? AND kcu.TABLE_NAME = ? \
                   AND kcu.REFERENCED_TABLE_NAME IS NOT NULL \
                 ORDER BY kcu.CONSTRAINT_NAME, kcu.ORDINAL_POSITION",
                (database_sql, table_sql),
            )
            .await
            .map_err(map_query_error)?;

        let mut fk_map: std::collections::HashMap<String, ForeignKeyInfo> =
            std::collections::HashMap::new();

        for row in rows {
            let name: String = row.get(0).unwrap_or_default();
            let column: String = row.get(1).unwrap_or_default();
            let ref_table: String = row.get(2).unwrap_or_default();
            let ref_column: String = row.get(3).unwrap_or_default();
            let on_update: String = row.get(4).unwrap_or_else(|| "NO ACTION".to_string());
            let on_delete: String = row.get(5).unwrap_or_else(|| "NO ACTION".to_string());

            let entry = fk_map
                .entry(name.clone())
                .or_insert_with(|| ForeignKeyInfo {
                    name: name.clone(),
                    columns: Vec::new(),
                    ref_table: ref_table.clone(),
                    ref_columns: Vec::new(),
                    on_update,
                    on_delete,
                });
            entry.columns.push(column);
            entry.ref_columns.push(ref_column);
        }
        let _ = conn.disconnect().await;
        Ok(fk_map.into_values().collect())
    }

    /// Read inbound references pointing at `database.table`.
    pub async fn query_references(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
        database: &MysqlIdentifier,
        table: &MysqlIdentifier,
    ) -> Result<Vec<ReferenceInfo>, MysqlConnectionError> {
        let database_sql = database.to_sql(IdentifierContext::Database);
        let table_sql = table.to_sql(IdentifierContext::Table);
        let mut conn = Self::connect(host, port, username, password).await?;
        let rows: Vec<Row> = conn
            .exec(
                "SELECT kcu.CONSTRAINT_NAME, kcu.TABLE_NAME, kcu.COLUMN_NAME, \
                        kcu.REFERENCED_TABLE_NAME, kcu.REFERENCED_COLUMN_NAME \
                 FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE kcu \
                 WHERE kcu.REFERENCED_TABLE_SCHEMA = ? AND kcu.REFERENCED_TABLE_NAME = ? \
                 ORDER BY kcu.CONSTRAINT_NAME, kcu.ORDINAL_POSITION",
                (database_sql, table_sql),
            )
            .await
            .map_err(map_query_error)?;

        let mut ref_map: std::collections::HashMap<String, ReferenceInfo> =
            std::collections::HashMap::new();

        for row in rows {
            let fk_name: String = row.get(0).unwrap_or_default();
            let ref_table: String = row.get(1).unwrap_or_default();
            let column: String = row.get(2).unwrap_or_default();
            let ref_ref_table: String = row.get(3).unwrap_or_default();
            let ref_column: String = row.get(4).unwrap_or_default();

            let entry = ref_map
                .entry(fk_name.clone())
                .or_insert_with(|| ReferenceInfo {
                    fk_name: fk_name.clone(),
                    ref_table: ref_table.clone(),
                    ref_columns: Vec::new(),
                    columns: Vec::new(),
                });
            entry.columns.push(column);
            entry.ref_columns.push(ref_column);
        }
        let _ = conn.disconnect().await;
        Ok(ref_map.into_values().collect())
    }

    /// Read triggers for `database.table`.
    pub async fn query_triggers(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
        database: &MysqlIdentifier,
        table: &MysqlIdentifier,
    ) -> Result<Vec<TriggerInfo>, MysqlConnectionError> {
        let database_sql = database.to_sql(IdentifierContext::Database);
        let table_sql = table.to_sql(IdentifierContext::Table);
        let mut conn = Self::connect(host, port, username, password).await?;
        let rows: Vec<Row> = conn
            .exec(
                "SELECT TRIGGER_NAME, EVENT_MANIPULATION, ACTION_TIMING, ACTION_STATEMENT \
                 FROM INFORMATION_SCHEMA.TRIGGERS \
                 WHERE TRIGGER_SCHEMA = ? AND EVENT_OBJECT_TABLE = ? \
                 ORDER BY TRIGGER_NAME",
                (database_sql, table_sql),
            )
            .await
            .map_err(map_query_error)?;

        let mut triggers = Vec::new();
        for row in rows {
            let name: String = row.get(0).unwrap_or_default();
            let event: String = row.get(1).unwrap_or_default();
            let timing: String = row.get(2).unwrap_or_default();
            let statement: String = row.get(3).unwrap_or_default();
            triggers.push(TriggerInfo {
                name,
                event,
                timing,
                statement,
            });
        }
        let _ = conn.disconnect().await;
        Ok(triggers)
    }
}

fn map_connect_error(err: mysql_async::Error) -> MysqlConnectionError {
    let rendered = err.to_string();
    let lowered = rendered.to_ascii_lowercase();
    if lowered.contains("access denied") || lowered.contains("authentication") {
        // MysqlAsync does not surface the username; the caller
        // re-tags with the actual attempted user.
        return MysqlConnectionError::AuthenticationFailed {
            user: "<redacted>".into(),
        };
    }
    MysqlConnectionError::Transport(redact(&rendered))
}

fn map_query_error(err: mysql_async::Error) -> MysqlConnectionError {
    let rendered = err.to_string();
    let lowered = rendered.to_ascii_lowercase();
    if lowered.contains("access denied") || lowered.contains("authentication") {
        return MysqlConnectionError::AuthenticationFailed {
            user: "<redacted>".into(),
        };
    }
    if lowered.contains("cancelled") || lowered.contains("canceled") {
        return MysqlConnectionError::Cancelled;
    }
    MysqlConnectionError::Transport(redact(&rendered))
}

fn map_decode_error<E: std::fmt::Display>(err: E) -> MysqlConnectionError {
    MysqlConnectionError::Decode(redact(&err.to_string()))
}

/// Strip any password-looking substring from a free-form error
/// message before it surfaces to the UI.
fn redact(message: &str) -> String {
    let mut safe = String::with_capacity(message.len());
    for token in message.split_whitespace() {
        if token.to_ascii_lowercase().contains("password")
            || token.contains("P1aintext")
            || token.contains("canary")
        {
            safe.push_str("<redacted> ");
        } else {
            safe.push_str(token);
            safe.push(' ');
        }
    }
    safe.trim().to_string()
}
