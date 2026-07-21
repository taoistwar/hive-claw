use anyhow::Result;

use mysql_async::prelude::*;
use mysql_async::{Conn, Opts, OptsBuilder, Row};

use super::models::{
    ColumnInfo, ConstraintInfo, DatabaseInfo, ForeignKeyInfo, IndexInfo, ReferenceInfo, TableData,
    TableDataRequest, TableInfo, TriggerInfo,
};

const _CONNECT_TIMEOUT_SECS: u64 = 10;
const DEFAULT_QUERY_LIMIT: i64 = 100;

pub struct MysqlClient;

impl MysqlClient {
    async fn connect(host: &str, port: u16, username: &str, password: &[u8]) -> Result<Conn> {
        let password_str = String::from_utf8(password.to_vec())?;

        let opts = OptsBuilder::default()
            .ip_or_hostname(host)
            .tcp_port(port)
            .user(Some(username))
            .pass(Some(password_str));

        let opts = Opts::from(opts);
        let conn = Conn::new(opts).await?;
        Ok(conn)
    }

    pub async fn test_connection(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
    ) -> Result<()> {
        let conn = Self::connect(host, port, username, password).await?;
        conn.disconnect().await?;
        Ok(())
    }

    pub async fn query_databases(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
    ) -> Result<Vec<DatabaseInfo>> {
        let mut conn = Self::connect(host, port, username, password).await?;

        let rows: Vec<Row> = conn
            .query(
                r#"
                SELECT SCHEMA_NAME, DEFAULT_CHARACTER_SET_NAME, DEFAULT_COLLATION_NAME
                FROM information_schema.SCHEMATA
                WHERE SCHEMA_NAME NOT IN ('information_schema', 'mysql', 'performance_schema', 'sys')
                ORDER BY SCHEMA_NAME
                "#,
            )
            .await?;

        let mut dbs = Vec::new();
        for row in rows {
            let name: Option<String> = row.get_opt(0).unwrap_or(Ok(None))?;
            let charset: Option<String> = row.get_opt(1).unwrap_or(Ok(None))?;
            let collation: Option<String> = row.get_opt(2).unwrap_or(Ok(None))?;
            dbs.push(DatabaseInfo {
                name: name.unwrap_or_default(),
                charset,
                collation,
            });
        }

        conn.disconnect().await?;
        Ok(dbs)
    }

    pub async fn query_tables(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
        db_name: &str,
    ) -> Result<Vec<TableInfo>> {
        let mut conn = Self::connect(host, port, username, password).await?;

        let query = format!(
            r#"
            SELECT TABLE_NAME, TABLE_COMMENT, ENGINE, TABLE_ROWS
            FROM information_schema.TABLES
            WHERE TABLE_SCHEMA = '{}'
            AND TABLE_TYPE = 'BASE TABLE'
            ORDER BY TABLE_NAME
            "#,
            Self::escape(db_name)
        );

        let rows: Vec<Row> = conn.query(&query).await?;

        let mut tables = Vec::new();
        for row in rows {
            let name: Option<String> = row.get_opt(0).unwrap_or(Ok(None))?;
            let comment: Option<String> = row.get_opt(1).unwrap_or(Ok(None))?;
            let engine: Option<String> = row.get_opt(2).unwrap_or(Ok(None))?;
            let row_count: Option<i64> = row.get_opt(3).unwrap_or(Ok(None))?;
            tables.push(TableInfo {
                name: name.unwrap_or_default(),
                comment,
                engine,
                row_count,
            });
        }

        conn.disconnect().await?;
        Ok(tables)
    }

    pub async fn query_columns(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
        db_name: &str,
        table_name: &str,
    ) -> Result<Vec<ColumnInfo>> {
        let mut conn = Self::connect(host, port, username, password).await?;

        let query = format!(
            r#"
            SELECT
                COLUMN_NAME,
                DATA_TYPE,
                IS_NULLABLE,
                COLUMN_DEFAULT,
                COLUMN_COMMENT,
                CHARACTER_MAXIMUM_LENGTH,
                NUMERIC_PRECISION,
                COLUMN_KEY
            FROM information_schema.COLUMNS
            WHERE TABLE_SCHEMA = '{}'
            AND TABLE_NAME = '{}'
            ORDER BY ORDINAL_POSITION
            "#,
            Self::escape(db_name),
            Self::escape(table_name)
        );

        let rows: Vec<Row> = conn.query(&query).await?;

        let mut columns = Vec::new();
        for row in rows {
            let name: Option<String> = row.get_opt(0).unwrap_or(Ok(None))?;
            let data_type: Option<String> = row.get_opt(1).unwrap_or(Ok(None))?;
            let is_nullable_opt: Option<String> = row.get_opt(2).unwrap_or(Ok(None))?;
            let column_default: Option<String> = row.get_opt(3).unwrap_or(Ok(None))?;
            let comment: Option<String> = row.get_opt(4).unwrap_or(Ok(None))?;
            let char_max_len: Option<i64> = row.get_opt(5).unwrap_or(Ok(None))?;
            let numeric_precision: Option<i64> = row.get_opt(6).unwrap_or(Ok(None))?;
            let column_key: Option<String> = row.get_opt(7).unwrap_or(Ok(None))?;

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

        conn.disconnect().await?;
        Ok(columns)
    }

    pub async fn query_ddl(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
        db_name: &str,
        table_name: &str,
    ) -> Result<String> {
        let mut conn = Self::connect(host, port, username, password).await?;

        let query = format!(
            "SHOW CREATE TABLE `{}`.`{}`",
            Self::escape(db_name),
            Self::escape(table_name)
        );

        let rows: Vec<Row> = conn.query(&query).await?;

        if let Some(row) = rows.first() {
            let ddl: Option<String> = row.get_opt(1).unwrap_or(Ok(None))?;
            Ok(ddl.unwrap_or_default())
        } else {
            Err(anyhow::anyhow!(
                "Table `{}.{}` not found",
                db_name,
                table_name
            ))
        }
    }

    pub async fn query_table_data(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
        db_name: &str,
        table_name: &str,
        req: &TableDataRequest,
    ) -> Result<TableData> {
        let mut conn = Self::connect(host, port, username, password).await?;

        let escaped_db = Self::escape(db_name);
        let escaped_table = Self::escape(table_name);

        let count_query = if let Some(ref where_clause) = req.where_clause {
            format!(
                "SELECT COUNT(*) FROM `{}`.`{}` WHERE {}",
                escaped_db, escaped_table, where_clause
            )
        } else {
            format!("SELECT COUNT(*) FROM `{}`.`{}`", escaped_db, escaped_table)
        };

        let count_rows: Vec<Row> = conn.query(&count_query).await?;
        let total_count: i64 = count_rows
            .first()
            .and_then(|r| match r.get_opt(0) {
                Some(Ok(v)) => Some(v),
                _ => None,
            })
            .unwrap_or(0);

        let limit = if req.limit > 0 {
            req.limit
        } else {
            DEFAULT_QUERY_LIMIT
        };

        let mut data_query = if let Some(ref where_clause) = req.where_clause {
            format!(
                "SELECT * FROM `{}`.`{}` WHERE {}",
                escaped_db, escaped_table, where_clause
            )
        } else {
            format!("SELECT * FROM `{}`.`{}`", escaped_db, escaped_table)
        };

        if let Some(ref order_by) = req.order_by {
            data_query.push_str(" ORDER BY ");
            data_query.push_str(order_by);
        }

        data_query.push_str(&format!(" LIMIT {} OFFSET {}", limit, req.offset));

        let rows: Vec<Row> = conn.query(&data_query).await?;

        let columns: Vec<String> = rows
            .first()
            .map(|r| {
                r.columns_ref()
                    .iter()
                    .map(|c| c.name_str().to_string())
                    .collect()
            })
            .unwrap_or_default();

        let mut data_rows = Vec::new();
        for row in rows {
            let values: Vec<Option<String>> = (0..columns.len())
                .map(|i| {
                    let val: Option<String> = row.get_opt(i).unwrap_or(Ok(None))?;
                    Ok(val)
                })
                .collect::<Result<Vec<Option<String>>>>()?;
            data_rows.push(values);
        }

        conn.disconnect().await?;

        Ok(TableData {
            columns,
            rows: data_rows,
            total_count,
            limit,
            offset: req.offset,
        })
    }

    fn escape(s: &str) -> String {
        s.replace('`', "``")
    }

    pub async fn query_indexes(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
        db_name: &str,
        table_name: &str,
    ) -> Result<Vec<IndexInfo>> {
        let mut conn = Self::connect(host, port, username, password).await?;

        let query = format!(
            "SELECT INDEX_NAME, COLUMN_NAME, NON_UNIQUE, INDEX_TYPE, COMMENT
             FROM INFORMATION_SCHEMA.STATISTICS
             WHERE TABLE_SCHEMA = '{}' AND TABLE_NAME = '{}'
             ORDER BY INDEX_NAME, SEQ_IN_INDEX",
            Self::escape(db_name),
            Self::escape(table_name)
        );

        let rows: Vec<Row> = conn.query(&query).await?;

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

        conn.disconnect().await?;
        Ok(index_map.into_values().collect())
    }

    pub async fn query_constraints(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
        db_name: &str,
        table_name: &str,
    ) -> Result<Vec<ConstraintInfo>> {
        let mut conn = Self::connect(host, port, username, password).await?;

        let query = format!(
            "SELECT tc.CONSTRAINT_NAME, tc.CONSTRAINT_TYPE, kcu.COLUMN_NAME, cc.CHECK_CLAUSE
             FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS tc
             LEFT JOIN INFORMATION_SCHEMA.KEY_COLUMN_USAGE kcu
               ON tc.CONSTRAINT_NAME = kcu.CONSTRAINT_NAME AND tc.TABLE_SCHEMA = kcu.TABLE_SCHEMA
             LEFT JOIN INFORMATION_SCHEMA.CHECK_CONSTRAINTS cc
               ON tc.CONSTRAINT_NAME = cc.CONSTRAINT_NAME AND tc.TABLE_SCHEMA = cc.CONSTRAINT_SCHEMA
             WHERE tc.TABLE_SCHEMA = '{}' AND tc.TABLE_NAME = '{}'
             ORDER BY tc.CONSTRAINT_NAME, kcu.ORDINAL_POSITION",
            Self::escape(db_name),
            Self::escape(table_name)
        );

        let rows: Vec<Row> = conn.query(&query).await?;

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

        conn.disconnect().await?;
        Ok(constraint_map.into_values().collect())
    }

    pub async fn query_foreign_keys(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
        db_name: &str,
        table_name: &str,
    ) -> Result<Vec<ForeignKeyInfo>> {
        let mut conn = Self::connect(host, port, username, password).await?;

        let query = format!(
            "SELECT kcu.CONSTRAINT_NAME, kcu.COLUMN_NAME,
                    kcu.REFERENCED_TABLE_NAME, kcu.REFERENCED_COLUMN_NAME,
                    rc.UPDATE_RULE, rc.DELETE_RULE
             FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE kcu
             JOIN INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS rc
               ON kcu.CONSTRAINT_NAME = rc.CONSTRAINT_NAME AND kcu.TABLE_SCHEMA = rc.CONSTRAINT_SCHEMA
             WHERE kcu.TABLE_SCHEMA = '{}' AND kcu.TABLE_NAME = '{}'
               AND kcu.REFERENCED_TABLE_NAME IS NOT NULL
             ORDER BY kcu.CONSTRAINT_NAME, kcu.ORDINAL_POSITION",
            Self::escape(db_name),
            Self::escape(table_name)
        );

        let rows: Vec<Row> = conn.query(&query).await?;

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

        conn.disconnect().await?;
        Ok(fk_map.into_values().collect())
    }

    pub async fn query_references(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
        db_name: &str,
        table_name: &str,
    ) -> Result<Vec<ReferenceInfo>> {
        let mut conn = Self::connect(host, port, username, password).await?;

        let query = format!(
            "SELECT kcu.CONSTRAINT_NAME, kcu.TABLE_NAME, kcu.COLUMN_NAME,
                    kcu.REFERENCED_TABLE_NAME, kcu.REFERENCED_COLUMN_NAME
             FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE kcu
             WHERE kcu.REFERENCED_TABLE_SCHEMA = '{}' AND kcu.REFERENCED_TABLE_NAME = '{}'
             ORDER BY kcu.CONSTRAINT_NAME, kcu.ORDINAL_POSITION",
            Self::escape(db_name),
            Self::escape(table_name)
        );

        let rows: Vec<Row> = conn.query(&query).await?;

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

        conn.disconnect().await?;
        Ok(ref_map.into_values().collect())
    }

    pub async fn query_triggers(
        host: &str,
        port: u16,
        username: &str,
        password: &[u8],
        db_name: &str,
        table_name: &str,
    ) -> Result<Vec<TriggerInfo>> {
        let mut conn = Self::connect(host, port, username, password).await?;

        let query = format!(
            "SELECT TRIGGER_NAME, EVENT_MANIPULATION, ACTION_TIMING, ACTION_STATEMENT
             FROM INFORMATION_SCHEMA.TRIGGERS
             WHERE TRIGGER_SCHEMA = '{}' AND EVENT_OBJECT_TABLE = '{}'
             ORDER BY TRIGGER_NAME",
            Self::escape(db_name),
            Self::escape(table_name)
        );

        let rows: Vec<Row> = conn.query(&query).await?;

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

        conn.disconnect().await?;
        Ok(triggers)
    }
}
