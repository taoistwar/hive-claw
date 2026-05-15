//! 元数据服务
//! 
//! 负责连接 Hive Metastore (MySQL) 并管理元数据缓存

use crate::config::AppConfig;
use anyhow::{Result, Context};
use sqlx::{MySql, MySqlPool, Pool, Row};
use std::sync::Arc;
use serde::{Serialize, Deserialize};

/// 数据库元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseMetadata {
    pub name: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub owner: Option<String>,
    pub created_at: Option<String>,
}

/// 表元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableMetadata {
    pub database: String,
    pub name: String,
    pub table_type: String,
    pub columns: Vec<ColumnMetadata>,
    pub partitions: Vec<PartitionMetadata>,
    pub location: Option<String>,
    pub owner: Option<String>,
    pub created_at: Option<String>,
    pub last_accessed: Option<String>,
    pub comment: Option<String>,
}

/// 列元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnMetadata {
    pub name: String,
    pub data_type: String,
    pub comment: Option<String>,
    pub position: i32,
    pub nullable: bool,
}

/// 分区元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionMetadata {
    pub name: String,
    pub data_type: String,
    pub values: Option<Vec<String>>,
}

/// 元数据服务
pub struct MetadataService {
    config: AppConfig,
    pool: Option<Arc<Pool<MySql>>>,
}

impl MetadataService {
    /// 创建新的元数据服务
    pub fn new(config: AppConfig) -> Self {
        Self {
            config,
            pool: None,
        }
    }

    /// 初始化数据库连接池
    pub async fn initialize(&mut self) -> Result<()> {
        let hive_config = &self.config.hive_metastore;
        
        let connection_string = format!(
            "mysql://{}:{}@{}:{}/{}",
            hive_config.username,
            hive_config.password,
            hive_config.host,
            hive_config.port,
            hive_config.database
        );

        let pool = MySqlPool::connect(&connection_string)
            .await
            .context("Failed to connect to Hive Metastore")?;

        self.pool = Some(Arc::new(pool));
        tracing::info!("Connected to Hive Metastore at {}:{}", hive_config.host, hive_config.port);

        Ok(())
    }

    /// 获取所有数据库列表
    pub async fn list_databases(&self) -> Result<Vec<DatabaseMetadata>> {
        let pool = self.get_pool()?;
        
        let query = r#"
            SELECT 
                d.NAME as name,
                d.DESC as description,
                d.DB_LOCATION_URI as location,
                d.OWNER_NAME as owner,
                FROM_UNIXTIME(d.CREATE_TIME) as created_at
            FROM DBS d
            ORDER BY d.NAME
        "#;

        let rows = sqlx::query(query)
            .fetch_all(pool.as_ref())
            .await
            .context("Failed to fetch databases")?;

        let databases: Vec<DatabaseMetadata> = rows.iter().map(|row| {
            DatabaseMetadata {
                name: row.get::<String, _>("name"),
                description: row.get::<Option<String>, _>("description"),
                location: row.get::<Option<String>, _>("location"),
                owner: row.get::<Option<String>, _>("owner"),
                created_at: row.get::<Option<String>, _>("created_at"),
            }
        }).collect();

        Ok(databases)
    }

    /// 获取指定数据库的所有表
    pub async fn list_tables(&self, database: &str) -> Result<Vec<String>> {
        let pool = self.get_pool()?;
        
        let query = r#"
            SELECT t.TBL_NAME
            FROM TBLS t
            JOIN DBS d ON t.DB_ID = d.DB_ID
            WHERE d.NAME = ?
            ORDER BY t.TBL_NAME
        "#;

        let rows = sqlx::query(query)
            .bind(database)
            .fetch_all(pool.as_ref())
            .await
            .context("Failed to fetch tables")?;

        let tables: Vec<String> = rows.iter()
            .map(|row| row.get::<String, _>("TBL_NAME"))
            .collect();

        Ok(tables)
    }

    /// 获取表的详细元数据
    pub async fn get_table_metadata(&self, database: &str, table: &str) -> Result<TableMetadata> {
        let pool = self.get_pool()?;

        // 获取表基本信息
        let table_query = r#"
            SELECT 
                t.TBL_NAME as name,
                t.TBL_TYPE as table_type,
                t.LOAD_TIME as created_at,
                t.LAST_ACCESS_TIME as last_accessed,
                sd.LOCATION as location,
                s.OWNER_NAME as owner,
                s.DESC as comment
            FROM TBLS t
            JOIN DBS d ON t.DB_ID = d.DB_ID
            JOIN SDS sd ON t.SD_ID = sd.SD_ID
            LEFT JOIN SRMPS s ON sd.SERDE_ID = s.SERDE_ID
            WHERE d.NAME = ? AND t.TBL_NAME = ?
        "#;

        let row = sqlx::query(table_query)
            .bind(database)
            .bind(table)
            .fetch_optional(pool.as_ref())
            .await
            .context("Failed to fetch table info")?;

        match row {
            Some(row) => {
                // 获取列信息
                let columns = self.get_columns(pool.as_ref(), database, table).await?;
                
                // 获取分区信息
                let partitions = self.get_partitions(pool.as_ref(), database, table).await?;

                Ok(TableMetadata {
                    database: database.to_string(),
                    name: row.get::<String, _>("name"),
                    table_type: row.get::<String, _>("table_type"),
                    columns,
                    partitions,
                    location: row.get::<Option<String>, _>("location"),
                    owner: row.get::<Option<String>, _>("owner"),
                    created_at: row.get::<Option<i64>, _>("created_at").map(|t| t.to_string()),
                    last_accessed: row.get::<Option<i64>, _>("last_accessed").map(|t| t.to_string()),
                    comment: row.get::<Option<String>, _>("comment"),
                })
            }
            None => {
                anyhow::bail!("Table {}.{} not found", database, table);
            }
        }
    }

    /// 获取表的列信息
    async fn get_columns(&self, pool: &Pool<MySql>, database: &str, table: &str) -> Result<Vec<ColumnMetadata>> {
        let query = r#"
            SELECT 
                c.COLUMN_NAME as name,
                c.TYPE_NAME as data_type,
                c.COMMENT as comment,
                c.INTEGER_IDX as position,
                CASE WHEN c.NULLABLE = 1 THEN true ELSE false END as nullable
            FROM COLUMNS_V2 c
            JOIN SDS sd ON c.CD_ID = sd.CD_ID
            JOIN TBLS t ON sd.SD_ID = t.SD_ID
            JOIN DBS d ON t.DB_ID = d.DB_ID
            WHERE d.NAME = ? AND t.TBL_NAME = ?
            ORDER BY c.INTEGER_IDX
        "#;

        let rows = sqlx::query(query)
            .bind(database)
            .bind(table)
            .fetch_all(pool)
            .await
            .context("Failed to fetch columns")?;

        let columns: Vec<ColumnMetadata> = rows.iter().map(|row| {
            ColumnMetadata {
                name: row.get::<String, _>("name"),
                data_type: row.get::<String, _>("data_type"),
                comment: row.get::<Option<String>, _>("comment"),
                position: row.get::<i32, _>("position"),
                nullable: row.get::<bool, _>("nullable"),
            }
        }).collect();

        Ok(columns)
    }

    /// 获取表的分区信息
    async fn get_partitions(&self, pool: &Pool<MySql>, database: &str, table: &str) -> Result<Vec<PartitionMetadata>> {
        let query = r#"
            SELECT 
                c.PKEY_NAME as name,
                c.PKEY_TYPE as data_type
            FROM PARTITION_KEYS c
            JOIN TBLS t ON c.TBL_ID = t.TBL_ID
            JOIN DBS d ON t.DB_ID = d.DB_ID
            WHERE d.NAME = ? AND t.TBL_NAME = ?
            ORDER BY c.INTEGER_IDX
        "#;

        let rows = sqlx::query(query)
            .bind(database)
            .bind(table)
            .fetch_all(pool)
            .await
            .context("Failed to fetch partitions")?;

        let partitions: Vec<PartitionMetadata> = rows.iter().map(|row| {
            PartitionMetadata {
                name: row.get::<String, _>("name"),
                data_type: row.get::<String, _>("data_type"),
                values: None,
            }
        }).collect();

        Ok(partitions)
    }

    /// 搜索表（支持模糊匹配）
    pub async fn search_tables(&self, pattern: &str) -> Result<Vec<(String, String)>> {
        let pool = self.get_pool()?;
        
        let query = r#"
            SELECT 
                d.NAME as database,
                t.TBL_NAME as table_name
            FROM TBLS t
            JOIN DBS d ON t.DB_ID = d.DB_ID
            WHERE t.TBL_NAME LIKE ?
            ORDER BY d.NAME, t.TBL_NAME
            LIMIT 100
        "#;

        let rows = sqlx::query(query)
            .bind(format!("%{}%", pattern))
            .fetch_all(pool.as_ref())
            .await
            .context("Failed to search tables")?;

        let tables: Vec<(String, String)> = rows.iter().map(|row| {
            (
                row.get::<String, _>("database"),
                row.get::<String, _>("table_name"),
            )
        }).collect();

        Ok(tables)
    }

    /// 获取表的采样数据
    pub async fn sample_table_data(&self, database: &str, table: &str, _limit: i32) -> Result<Vec<serde_json::Value>> {
        tracing::warn!("Sample data requires HiveServer2 connection (not implemented yet)");
        Ok(vec![])
    }

    /// 获取表的基本统计信息
    pub async fn get_table_statistics(&self, database: &str, table: &str) -> Result<TableStatistics> {
        let pool = self.get_pool()?;
        
        let query = r#"
            SELECT 
                s.NUM_ROWS as num_rows,
                s.TOTAL_SIZE as total_size,
                s.RAW_SIZE as raw_size,
                s.NUM_FILES as num_files
            FROM TAB_STATS s
            JOIN TBLS t ON s.TBL_ID = t.TBL_ID
            JOIN DBS d ON t.DB_ID = d.DB_ID
            WHERE d.NAME = ? AND t.TBL_NAME = ?
        "#;

        let row = sqlx::query(query)
            .bind(database)
            .bind(table)
            .fetch_optional(pool.as_ref())
            .await
            .context("Failed to fetch table statistics")?;

        match row {
            Some(row) => {
                Ok(TableStatistics {
                    num_rows: row.get::<Option<i64>, _>("num_rows").unwrap_or(0),
                    total_size: row.get::<Option<i64>, _>("total_size").unwrap_or(0),
                    raw_size: row.get::<Option<i64>, _>("raw_size").unwrap_or(0),
                    num_files: row.get::<Option<i32>, _>("num_files").unwrap_or(0),
                })
            }
            None => {
                Ok(TableStatistics::default())
            }
        }
    }

    fn get_pool(&self) -> Result<Arc<Pool<MySql>>> {
        self.pool.clone().ok_or_else(|| {
            anyhow::anyhow!("Metadata service not initialized. Call initialize() first.")
        })
    }
}

/// 表统计信息
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TableStatistics {
    pub num_rows: i64,
    pub total_size: i64,
    pub raw_size: i64,
    pub num_files: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_metadata_service_mock() {
        let config = AppConfig::default();
        let service = MetadataService::new(config);
        
        assert!(service.pool.is_none());
    }
}
