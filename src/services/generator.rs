//! 任务生成器服务
//! 
//! 根据用户输入和模板生成 Azkaban 任务配置

use crate::config::AppConfig;
use crate::models::task::{Task, TaskTemplate};
use anyhow::Result;
use serde::{Serialize, Deserialize};

/// 任务生成器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratorConfig {
    pub template_path: String,
    pub output_path: String,
    pub ai_enabled: bool,
}

/// 任务生成上下文
#[derive(Debug, Clone)]
pub struct GenerationContext {
    pub template: TaskTemplate,
    pub task_name: String,
    pub description: String,
    pub source_tables: Vec<String>,
    pub target_table: String,
    pub schedule: String,
    pub dependencies: Vec<String>,
    pub parameters: Vec<(String, String)>,
    pub sql_query: Option<String>,
    pub ai_suggestions: Option<String>,
}

/// 生成的任务
#[derive(Debug, Clone)]
pub struct GeneratedTask {
    pub task: Task,
    pub flow_file: String,
    pub sql_file: Option<String>,
    pub config_files: Vec<String>,
}

/// 任务生成器服务
pub struct GeneratorService {
    config: AppConfig,
    generator_config: GeneratorConfig,
}

impl GeneratorService {
    /// 创建新的任务生成器
    pub fn new(config: AppConfig) -> Self {
        Self {
            generator_config: GeneratorConfig {
                template_path: "templates".to_string(),
                output_path: "generated".to_string(),
                ai_enabled: true,
            },
            config,
        }
    }

    /// 生成任务配置
    pub async fn generate(&self, context: GenerationContext) -> Result<GeneratedTask> {
        tracing::info!("Generating task: {}", context.task_name);

        // 根据模板类型生成不同的配置
        let _flow_content = match context.template {
            TaskTemplate::SingleTableAgg => {
                self.generate_single_table_agg(&context)?
            }
            TaskTemplate::MultiTableJoin => {
                self.generate_multi_table_join(&context)?
            }
            TaskTemplate::IncrementalSync => {
                self.generate_incremental_sync(&context)?
            }
            TaskTemplate::FullSync => {
                self.generate_full_sync(&context)?
            }
            TaskTemplate::Deduplication => {
                self.generate_deduplication(&context)?
            }
            TaskTemplate::SCD => {
                self.generate_scd(&context)?
            }
            TaskTemplate::MetricCalc => {
                self.generate_metric_calc(&context)?
            }
        };

        // 生成 SQL（如果有）
        let sql_content = if let Some(sql) = context.sql_query {
            Some(sql)
        } else {
            // 根据模板生成默认 SQL
            Some(self.generate_default_sql(&context)?)
        };

        // 生成任务对象
        let task = Task {
            id: uuid::Uuid::new_v4().to_string(),
            name: context.task_name.clone(),
            description: context.description.clone(),
            template: context.template.to_string(),
            status: crate::models::task::TaskStatus::Pending,
            created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            updated_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        };

        // 生成 flow 文件内容
        let flow_file = format!("{}.flow", context.task_name);
        
        Ok(GeneratedTask {
            task,
            flow_file,
            sql_file: sql_content.map(|_| format!("{}.sql", context.task_name)),
            config_files: vec![],
        })
    }

    /// 生成单表聚合任务
    fn generate_single_table_agg(&self, ctx: &GenerationContext) -> Result<String> {
        let mut flow = String::new();
        
        flow.push_str(&format!(r#"# Azkaban Flow: {}
# Description: {}
# Template: Single Table Aggregation

nodes:
  - name: {}_prep
    type: hiveql
    config:
      script: {}.sql
      params:
        - source_table: {}
        - target_table: {}
        - agg_columns: {}

  - name: {}_validate
    type: command
    config:
      command: python
      args:
        - scripts/validate_data.py
        - --table={}
        - --check=stats
    dependsOn:
      - {}_prep

  - name: {}_notify
    type: email
    config:
      recipients: {}
      subject: "Task Completed: {}"
    dependsOn:
      - {}_validate
"#,
            ctx.task_name,
            ctx.description,
            ctx.task_name,
            ctx.task_name,
            ctx.source_tables.first().unwrap_or(&"source_table".to_string()),
            ctx.target_table,
            "column1, column2",
            ctx.task_name,
            ctx.target_table,
            ctx.task_name,
            ctx.task_name,
            "team@example.com",
            ctx.task_name,
            ctx.task_name,
        ));

        Ok(flow)
    }

    /// 生成多表关联任务
    fn generate_multi_table_join(&self, ctx: &GenerationContext) -> Result<String> {
        let mut flow = String::new();
        
        flow.push_str(&format!(r#"# Azkaban Flow: {}
# Description: {}
# Template: Multi Table Join

nodes:
  - name: {}_stage
    type: hiveql
    config:
      script: {}_stage.sql
      params:
        - source_tables: {}
        - stage_table: {}_stage

  - name: {}_join
    type: hiveql
    config:
      script: {}_join.sql
      params:
        - stage_table: {}_stage
        - target_table: {}
    dependsOn:
      - {}_stage

  - name: {}_check
    type: command
    config:
      command: python
      args:
        - scripts/data_quality_check.py
        - --table={}
    dependsOn:
      - {}_join
"#,
            ctx.task_name,
            ctx.description,
            ctx.task_name,
            ctx.task_name,
            ctx.source_tables.join(", "),
            ctx.task_name,
            ctx.task_name,
            ctx.task_name,
            ctx.task_name,
            ctx.target_table,
            ctx.task_name,
            ctx.task_name,
            ctx.target_table,
            ctx.task_name,
        ));

        Ok(flow)
    }

    /// 生成增量同步任务
    fn generate_incremental_sync(&self, ctx: &GenerationContext) -> Result<String> {
        let mut flow = String::new();
        
        flow.push_str(&format!(r#"# Azkaban Flow: {}
# Description: {}
# Template: Incremental Sync

nodes:
  - name: {}_extract
    type: hiveql
    config:
      script: {}_extract.sql
      params:
        - source_table: {}
        - watermark_column: updated_at
        - last_watermark: ${{last_watermark}}

  - name: {}_transform
    type: hiveql
    config:
      script: {}_transform.sql
      params:
        - staging_table: {}_staging
        - target_table: {}
    dependsOn:
      - {}_extract

  - name: {}_update_watermark
    type: command
    config:
      command: python
      args:
        - scripts/update_watermark.py
        - --task={}
    dependsOn:
      - {}_transform
"#,
            ctx.task_name,
            ctx.description,
            ctx.task_name,
            ctx.task_name,
            ctx.source_tables.first().unwrap_or(&"source_table".to_string()),
            ctx.task_name,
            ctx.task_name,
            ctx.task_name,
            ctx.target_table,
            ctx.task_name,
            ctx.task_name,
            ctx.task_name,
            ctx.task_name,
        ));

        Ok(flow)
    }

    /// 生成全量同步任务
    fn generate_full_sync(&self, ctx: &GenerationContext) -> Result<String> {
        let mut flow = String::new();
        
        flow.push_str(&format!(r#"# Azkaban Flow: {}
# Description: {}
# Template: Full Sync

nodes:
  - name: {}_truncate
    type: hiveql
    config:
      script: TRUNCATE TABLE {}

  - name: {}_load
    type: hiveql
    config:
      script: {}_load.sql
      params:
        - source_table: {}
        - target_table: {}
    dependsOn:
      - {}_truncate

  - name: {}_analyze
    type: hiveql
    config:
      script: ANALYZE TABLE {} COMPUTE STATISTICS
    dependsOn:
      - {}_load
"#,
            ctx.task_name,
            ctx.description,
            ctx.task_name,
            ctx.target_table,
            ctx.task_name,
            ctx.task_name,
            ctx.source_tables.first().unwrap_or(&"source_table".to_string()),
            ctx.target_table,
            ctx.task_name,
            ctx.task_name,
            ctx.target_table,
            ctx.task_name,
        ));

        Ok(flow)
    }

    /// 生成去重清洗任务
    fn generate_deduplication(&self, ctx: &GenerationContext) -> Result<String> {
        let mut flow = String::new();
        
        flow.push_str(&format!(r#"# Azkaban Flow: {}
# Description: {}
# Template: Deduplication

nodes:
  - name: {}_dedup
    type: hiveql
    config:
      script: {}_dedup.sql
      params:
        - source_table: {}
        - target_table: {}
        - dedup_keys: id,version

  - name: {}_quality
    type: command
    config:
      command: python
      args:
        - scripts/dq_check.py
        - --table={}
        - --check=duplicates
    dependsOn:
      - {}_dedup
"#,
            ctx.task_name,
            ctx.description,
            ctx.task_name,
            ctx.task_name,
            ctx.source_tables.first().unwrap_or(&"source_table".to_string()),
            ctx.target_table,
            ctx.task_name,
            ctx.target_table,
            ctx.task_name,
        ));

        Ok(flow)
    }

    /// 生成 SCD (缓慢变化维) 任务
    fn generate_scd(&self, ctx: &GenerationContext) -> Result<String> {
        let mut flow = String::new();
        
        flow.push_str(&format!(r#"# Azkaban Flow: {}
# Description: {}
# Template: SCD Type 2

nodes:
  - name: {}_detect_changes
    type: hiveql
    config:
      script: {}_detect_changes.sql
      params:
        - source_table: {}
        - target_table: {}
        - scd_keys: id
        - track_columns: name,value,status

  - name: {}_update_history
    type: hiveql
    config:
      script: {}_update_history.sql
      params:
        - target_table: {}
    dependsOn:
      - {}_detect_changes

  - name: {}_expire_old
    type: hiveql
    config:
      script: {}_expire_old.sql
      params:
        - target_table: {}
    dependsOn:
      - {}_update_history
"#,
            ctx.task_name,
            ctx.description,
            ctx.task_name,
            ctx.task_name,
            ctx.source_tables.first().unwrap_or(&"source_table".to_string()),
            ctx.target_table,
            ctx.task_name,
            ctx.task_name,
            ctx.target_table,
            ctx.task_name,
            ctx.task_name,
            ctx.task_name,
            ctx.target_table,
            ctx.task_name,
        ));

        Ok(flow)
    }

    /// 生成指标计算任务
    fn generate_metric_calc(&self, ctx: &GenerationContext) -> Result<String> {
        let mut flow = String::new();
        
        flow.push_str(&format!(r#"# Azkaban Flow: {}
# Description: {}
# Template: Metric Calculation

nodes:
  - name: {}_calc_metrics
    type: hiveql
    config:
      script: {}_calc_metrics.sql
      params:
        - source_table: {}
        - target_table: {}
        - metrics: dau,retention,conversion

  - name: {}_export
    type: command
    config:
      command: python
      args:
        - scripts/export_metrics.py
        - --table={}
        - --output=s3://metrics/
    dependsOn:
      - {}_calc_metrics
"#,
            ctx.task_name,
            ctx.description,
            ctx.task_name,
            ctx.task_name,
            ctx.source_tables.first().unwrap_or(&"source_table".to_string()),
            ctx.target_table,
            ctx.task_name,
            ctx.target_table,
            ctx.task_name,
        ));

        Ok(flow)
    }

    /// 生成默认 SQL
    fn generate_default_sql(&self, ctx: &GenerationContext) -> Result<String> {
        let sql = match ctx.template {
            TaskTemplate::SingleTableAgg => self.gen_single_table_agg_sql(ctx)?,
            TaskTemplate::MultiTableJoin => self.gen_multi_table_join_sql(ctx)?,
            TaskTemplate::IncrementalSync => self.gen_incremental_sync_sql(ctx)?,
            TaskTemplate::FullSync => self.gen_full_sync_sql(ctx)?,
            TaskTemplate::Deduplication => self.gen_deduplication_sql(ctx)?,
            TaskTemplate::SCD => self.gen_scd_sql(ctx)?,
            TaskTemplate::MetricCalc => self.gen_metric_calc_sql(ctx)?,
        };

        Ok(sql)
    }

    /// 单表聚合 SQL
    fn gen_single_table_agg_sql(&self, ctx: &GenerationContext) -> Result<String> {
        let binding = "source_table".to_string();
        let source = ctx.source_tables.first().unwrap_or(&binding);
        Ok(format!(r#"-- Single Table Aggregation: {}
-- Source: {} -> Target: {}

INSERT OVERWRITE TABLE {}
PARTITION (dt = '${{hivevar:dt}}')
SELECT
    user_id,
    DATE(created_at) as event_date,
    COUNT(*) as event_count,
    SUM(COALESCE(amount, 0)) as total_amount,
    AVG(COALESCE(amount, 0)) as avg_amount,
    MIN(created_at) as first_event_time,
    MAX(created_at) as last_event_time
FROM {}
WHERE dt <= '${{hivevar:dt}}'
  AND user_id IS NOT NULL
GROUP BY user_id, DATE(created_at)
HAVING COUNT(*) > 0;
"#, ctx.task_name, source, ctx.target_table, ctx.target_table, source))
    }

    /// 多表关联 SQL
    fn gen_multi_table_join_sql(&self, ctx: &GenerationContext) -> Result<String> {
        let (source_a, source_b) = if ctx.source_tables.len() >= 2 {
            (ctx.source_tables[0].clone(), ctx.source_tables[1].clone())
        } else {
            ("table_a".to_string(), "table_b".to_string())
        };
        Ok(format!(r#"-- Multi-Table Join: {}
-- Sources: {} + {} -> Target: {}

INSERT OVERWRITE TABLE {}
PARTITION (dt = '${{hivevar:dt}}')
SELECT
    a.user_id,
    a.user_name,
    b.order_id,
    b.order_amount,
    b.order_status,
    b.created_at as order_time
FROM (
    SELECT user_id, user_name, email
    FROM {}
    WHERE dt = '${{hivevar:dt}}'
      AND is_deleted = 0
) a
INNER JOIN (
    SELECT order_id, user_id, order_amount, order_status, created_at
    FROM {}
    WHERE dt = '${{hivevar:dt}}'
      AND order_status IN ('paid', 'shipped', 'completed')
) b ON a.user_id = b.user_id
WHERE a.user_id IS NOT NULL
  AND b.order_id IS NOT NULL;
"#, ctx.task_name, source_a, source_b, ctx.target_table, ctx.target_table, source_a, source_b))
    }

    /// 增量同步 SQL
    fn gen_incremental_sync_sql(&self, ctx: &GenerationContext) -> Result<String> {
        let binding = "source_table".to_string();
        let source = ctx.source_tables.first().unwrap_or(&binding);
        Ok(format!(r#"-- Incremental Sync: {}
-- Source: {} -> Target: {}

INSERT OVERWRITE TABLE {}
PARTITION (dt = '${{hivevar:dt}}')
SELECT
    s.*
FROM {} s
WHERE s.dt = '${{hivevar:dt}}'
  AND s.updated_at >= (
    SELECT COALESCE(MAX(updated_at), '1970-01-01 00:00:00')
    FROM {} t
    WHERE t.dt < '${{hivevar:dt}}'
  )
  AND s.is_deleted = 0;
"#, ctx.task_name, source, ctx.target_table, source, ctx.target_table, source))
    }

    /// 全量同步 SQL
    fn gen_full_sync_sql(&self, ctx: &GenerationContext) -> Result<String> {
        let binding = "source_table".to_string();
        let source = ctx.source_tables.first().unwrap_or(&binding);
        Ok(format!(r#"-- Full Sync: {}
-- Source: {} -> Target: {}

INSERT OVERWRITE TABLE {}
PARTITION (dt = '${{hivevar:dt}}')
SELECT
    *
FROM {}
WHERE dt = '${{hivevar:dt}}'
  AND is_deleted = 0;
"#, ctx.task_name, source, ctx.target_table, ctx.target_table, source))
    }

    /// 去重清洗 SQL
    fn gen_deduplication_sql(&self, ctx: &GenerationContext) -> Result<String> {
        let binding = "source_table".to_string();
        let source = ctx.source_tables.first().unwrap_or(&binding);
        Ok(format!(r#"-- Deduplication: {}
-- Source: {} -> Target: {}

INSERT OVERWRITE TABLE {}
PARTITION (dt = '${{hivevar:dt}}')
SELECT
    t.*
FROM (
    SELECT
        *,
        ROW_NUMBER() OVER (
            PARTITION BY user_id, order_id, created_at
            ORDER BY updated_at DESC
        ) as rn
    FROM {}
    WHERE dt = '${{hivevar:dt}}'
      AND user_id IS NOT NULL
      AND order_id IS NOT NULL
) t
WHERE t.rn = 1;
"#, ctx.task_name, source, ctx.target_table, ctx.target_table, source))
    }

    /// SCD Type 2 缓慢变化维 SQL
    fn gen_scd_sql(&self, ctx: &GenerationContext) -> Result<String> {
        let binding = "source_table".to_string();
        let source = ctx.source_tables.first().unwrap_or(&binding);
        Ok(format!(r#"-- SCD Type 2: {}
-- Source: {} -> Target: {}

-- Step 1: Close expired records
UPDATE {}
SET is_current = 0,
    end_date = '${{hivevar:dt}}'
WHERE user_id IN (
    SELECT DISTINCT user_id
    FROM {}
    WHERE dt = '${{hivevar:dt}}'
)
AND is_current = 1;

-- Step 2: Insert new records
INSERT OVERWRITE TABLE {}
PARTITION (dt = '${{hivevar:dt}}')
SELECT
    ROW_NUMBER() OVER (ORDER BY user_id) as sk_user_id,
    s.user_id,
    s.user_name,
    s.email,
    s.phone,
    s.city,
    '${{hivevar:dt}}' as start_date,
    '9999-12-31' as end_date,
    1 as is_current,
    s.created_at,
    s.updated_at
FROM {} s
WHERE s.dt = '${{hivevar:dt}}'
  AND NOT EXISTS (
    SELECT 1
    FROM {} t
    WHERE t.user_id = s.user_id
      AND t.is_current = 1
  );
"#, ctx.task_name, source, ctx.target_table, ctx.target_table, source, source, ctx.target_table, source))
    }

    /// 指标计算 SQL
    fn gen_metric_calc_sql(&self, ctx: &GenerationContext) -> Result<String> {
        let binding = "source_table".to_string();
        let source = ctx.source_tables.first().unwrap_or(&binding);
        Ok(format!(r#"-- Metric Calculation: {}
-- Source: {} -> Target: {}

INSERT OVERWRITE TABLE {}
PARTITION (dt = '${{hivevar:dt}}', metric_date = '${{hivevar:metric_date}}')
SELECT
    'dau' as metric_name,
    COUNT(DISTINCT user_id) as metric_value,
    COUNT(DISTINCT CASE WHEN is_new_user = 1 THEN user_id END) as new_users,
    COUNT(DISTINCT CASE WHEN is_active = 1 THEN user_id END) as active_users
FROM {}
WHERE dt >= DATE_SUB('${{hivevar:metric_date}}', 1)
  AND dt <= '${{hivevar:metric_date}}'
  AND user_id IS NOT NULL

UNION ALL

SELECT
    'gmv' as metric_name,
    SUM(order_amount) as metric_value,
    COUNT(*) as order_count,
    AVG(order_amount) as avg_order_value
FROM {}
WHERE dt >= DATE_SUB('${{hivevar:metric_date}}', 1)
  AND dt <= '${{hivevar:metric_date}}'
  AND order_status IN ('paid', 'completed')
  AND order_amount > 0;
"#, ctx.task_name, source, ctx.target_table, ctx.target_table, source, source))
    }

    /// 验证生成的任务配置
    pub fn validate(&self, task: &GeneratedTask) -> Result<Vec<String>> {
        let mut errors = vec![];

        // 验证任务名称
        if task.task.name.is_empty() {
            errors.push("Task name cannot be empty".to_string());
        }

        // 验证 flow 文件
        if task.flow_file.is_empty() {
            errors.push("Flow file cannot be empty".to_string());
        }

        // 验证 SQL 文件（如果存在）
        if let Some(sql_file) = &task.sql_file {
            if sql_file.is_empty() {
                errors.push("SQL file cannot be empty".to_string());
            }
        }

        Ok(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_generator_service_creation() {
        let config = AppConfig::default();
        let service = GeneratorService::new(config);
        
        assert!(service.generator_config.ai_enabled);
    }
}
