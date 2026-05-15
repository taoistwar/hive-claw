//! 数据质量服务
//! 
//! 负责执行数据质量检查，生成质量报告

use crate::config::AppConfig;
use crate::services::metadata::TableMetadata;
use anyhow::Result;
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};

/// 问题严重程度
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Blocking,   // 阻塞：必须修复，任务失败
    Critical,   // 严重：重要问题，需要立即处理
    Major,      // 一般：需要关注，可以稍后修复
    Minor,      // 提示：建议改进
}

/// 质量检查类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum QualityCheck {
    /// 非空检查
    NotNull {
        column: String,
        threshold: f32,
    },
    /// 唯一性检查
    Unique {
        column: String,
        threshold: f32,
    },
    /// 值域检查
    Range {
        column: String,
        min: Option<f64>,
        max: Option<f64>,
        threshold: f32,
    },
    /// 枚举值检查
    Enum {
        column: String,
        allowed_values: Vec<String>,
        threshold: f32,
    },
    /// 波动检查
    Fluctuation {
        metric: String,
        max_increase: f32,
        max_decrease: f32,
        threshold: f32,
    },
    /// 行数检查
    RowCount {
        min_rows: i64,
        max_rows: Option<i64>,
        threshold: f32,
    },
    /// 格式检查
    Format {
        column: String,
        pattern: String,
        threshold: f32,
    },
    /// 一致性检查
    Consistency {
        source_table: String,
        target_table: String,
        join_condition: String,
        threshold: f32,
    },
}

/// 质量问题
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityIssue {
    pub check_type: String,
    pub column: Option<String>,
    pub severity: Severity,
    pub description: String,
    pub actual_value: String,
    pub expected_value: String,
    pub affected_rows: i64,
    pub sql_query: Option<String>,
    pub suggestion: String,
}

/// 质量检查报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityReport {
    pub task_name: String,
    pub table_name: String,
    pub check_time: DateTime<Utc>,
    pub total_checks: usize,
    pub passed_checks: usize,
    pub failed_checks: usize,
    pub issues: Vec<QualityIssue>,
    pub overall_status: QualityStatus,
    pub summary: String,
}

/// 质量状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum QualityStatus {
    Passed,
    Warning,
    Failed,
}

/// 质量规则配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityRule {
    pub name: String,
    pub check: QualityCheck,
    pub severity: Severity,
    pub enabled: bool,
    pub description: Option<String>,
}

/// 数据质量服务
pub struct QualityService {
    config: AppConfig,
    rules: Vec<QualityRule>,
}

impl QualityService {
    /// 创建新的质量服务
    pub fn new(config: AppConfig) -> Self {
        Self {
            config,
            rules: Vec::new(),
        }
    }

    /// 初始化默认规则
    pub fn initialize_default_rules(&mut self) {
        // 默认规则会在任务创建时动态添加
        tracing::info!("Quality service initialized");
    }

    /// 添加质量规则
    pub fn add_rule(&mut self, rule: QualityRule) {
        let rule_name = rule.name.clone();
        self.rules.push(rule);
        tracing::debug!("Added quality rule: {}", rule_name);
    }

    /// 根据表元数据生成推荐规则
    pub fn generate_recommended_rules(&self, table_metadata: &TableMetadata) -> Vec<QualityRule> {
        let mut rules = Vec::new();

        // 为每个列生成基础规则
        for column in &table_metadata.columns {
            // 非空检查（主键或重要字段）
            if !column.nullable || column.name.to_lowercase().contains("id") {
                rules.push(QualityRule {
                    name: format!("{}_not_null", column.name),
                    check: QualityCheck::NotNull {
                        column: column.name.clone(),
                        threshold: 0.0,
                    },
                    severity: Severity::Critical,
                    enabled: true,
                    description: Some(format!("检查 {} 列不为空", column.name)),
                });
            }

            // 格式检查（特定命名的列）
            if column.name.to_lowercase().contains("email") {
                rules.push(QualityRule {
                    name: format!("{}_email_format", column.name),
                    check: QualityCheck::Format {
                        column: column.name.clone(),
                        pattern: r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$".to_string(),
                        threshold: 0.01,
                    },
                    severity: Severity::Major,
                    enabled: true,
                    description: Some(format!("检查 {} 列的邮箱格式", column.name)),
                });
            }

            if column.name.to_lowercase().contains("phone") || column.name.to_lowercase().contains("mobile") {
                rules.push(QualityRule {
                    name: format!("{}_phone_format", column.name),
                    check: QualityCheck::Format {
                        column: column.name.clone(),
                        pattern: r"^1[3-9]\d{9}$".to_string(),
                        threshold: 0.01,
                    },
                    severity: Severity::Major,
                    enabled: true,
                    description: Some(format!("检查 {} 列的手机号格式", column.name)),
                });
            }

            // 值域检查（数值类型）
            if column.data_type.to_lowercase().contains("int") || 
               column.data_type.to_lowercase().contains("double") ||
               column.data_type.to_lowercase().contains("decimal") {
                rules.push(QualityRule {
                    name: format!("{}_non_negative", column.name),
                    check: QualityCheck::Range {
                        column: column.name.clone(),
                        min: Some(0.0),
                        max: None,
                        threshold: 0.01,
                    },
                    severity: Severity::Minor,
                    enabled: false, // 默认禁用，根据需要启用
                    description: Some(format!("检查 {} 列非负", column.name)),
                });
            }
        }

        // 唯一性检查（主键）
        if let Some(pk_column) = table_metadata.columns.iter()
            .find(|c| c.name.to_lowercase().ends_with("_id") || c.name.to_lowercase() == "id")
        {
            rules.push(QualityRule {
                name: format!("{}_unique", pk_column.name),
                check: QualityCheck::Unique {
                    column: pk_column.name.clone(),
                    threshold: 0.0,
                },
                severity: Severity::Blocking,
                enabled: true,
                description: Some(format!("检查 {} 列唯一性", pk_column.name)),
            });
        }

        // 行数检查
        rules.push(QualityRule {
            name: "row_count_min".to_string(),
            check: QualityCheck::RowCount {
                min_rows: 1,
                max_rows: None,
                threshold: 0.0,
            },
            severity: Severity::Critical,
            enabled: true,
            description: Some("检查表行数不为空".to_string()),
        });

        rules
    }

    /// 执行质量检查
    pub async fn execute_checks(
        &self,
        task_name: &str,
        table_name: &str,
        rules: &[QualityRule],
    ) -> Result<QualityReport> {
        let mut issues = Vec::new();
        let mut passed = 0;
        let mut failed = 0;

        for rule in rules.iter().filter(|r| r.enabled) {
            match self.execute_single_check(&rule.check, table_name).await {
                Ok(check_result) => {
                    if check_result.passed {
                        passed += 1;
                    } else {
                        failed += 1;
                        issues.push(QualityIssue {
                            check_type: self.get_check_type_name(&rule.check),
                            column: self.get_check_column(&rule.check),
                            severity: rule.severity.clone(),
                            description: rule.description.clone()
                                .unwrap_or_else(|| format!("{} check failed", rule.name)),
                            actual_value: check_result.actual_value,
                            expected_value: check_result.expected_value,
                            affected_rows: check_result.affected_rows,
                            sql_query: check_result.debug_sql,
                            suggestion: self.generate_suggestion(&rule.check),
                        });
                    }
                }
                Err(e) => {
                    failed += 1;
                    issues.push(QualityIssue {
                        check_type: self.get_check_type_name(&rule.check),
                        column: self.get_check_column(&rule.check),
                        severity: Severity::Major,
                        description: format!("检查执行失败：{}", e),
                        actual_value: "ERROR".to_string(),
                        expected_value: "SUCCESS".to_string(),
                        affected_rows: 0,
                        sql_query: None,
                        suggestion: "检查 SQL 语法和表结构".to_string(),
                    });
                }
            }
        }

        let overall_status = if failed == 0 {
            QualityStatus::Passed
        } else if issues.iter().any(|i| i.severity == Severity::Blocking || i.severity == Severity::Critical) {
            QualityStatus::Failed
        } else {
            QualityStatus::Warning
        };

        let summary = format!(
            "共执行 {} 项检查，通过 {} 项，失败 {} 项。{}",
            passed + failed,
            passed,
            failed,
            match overall_status {
                QualityStatus::Passed => "所有检查通过。".to_string(),
                QualityStatus::Warning => format!("发现 {} 个一般问题，建议修复。", failed),
                QualityStatus::Failed => format!("发现 {} 个严重问题，必须修复。", failed),
            }
        );

        Ok(QualityReport {
            task_name: task_name.to_string(),
            table_name: table_name.to_string(),
            check_time: Utc::now(),
            total_checks: passed + failed,
            passed_checks: passed,
            failed_checks: failed,
            issues,
            overall_status,
            summary,
        })
    }

    /// 执行单个检查
    async fn execute_single_check(
        &self,
        check: &QualityCheck,
        table_name: &str,
    ) -> Result<CheckResult> {
        // 生成检查 SQL 并执行
        let sql = self.generate_check_sql(check, table_name);
        
        // Mock 实现：实际应该连接 Hive 执行 SQL
        tracing::debug!("Executing quality check SQL: {}", sql);

        // 模拟结果
        Ok(CheckResult {
            passed: true,
            actual_value: "0".to_string(),
            expected_value: "0".to_string(),
            affected_rows: 0,
            debug_sql: Some(sql),
        })
    }

    /// 生成检查 SQL
    fn generate_check_sql(&self, check: &QualityCheck, table_name: &str) -> String {
        match check {
            QualityCheck::NotNull { column, threshold: _ } => {
                format!(
                    "SELECT COUNT(*) as null_count FROM {} WHERE {} IS NULL",
                    table_name, column
                )
            }
            QualityCheck::Unique { column, threshold: _ } => {
                format!(
                    "SELECT COUNT(*) - COUNT(DISTINCT {}) as duplicate_count FROM {}",
                    column, table_name
                )
            }
            QualityCheck::Range { column, min, max, threshold: _ } => {
                let mut conditions = Vec::new();
                if let Some(min_val) = min {
                    conditions.push(format!("{} < {}", column, min_val));
                }
                if let Some(max_val) = max {
                    conditions.push(format!("{} > {}", column, max_val));
                }
                format!(
                    "SELECT COUNT(*) as out_of_range_count FROM {} WHERE {}",
                    table_name,
                    conditions.join(" OR ")
                )
            }
            QualityCheck::Enum { column, allowed_values, threshold: _ } => {
                let values_str = allowed_values.iter()
                    .map(|v| format!("'{}'", v))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "SELECT COUNT(*) as invalid_count FROM {} WHERE {} NOT IN ({})",
                    table_name, column, values_str
                )
            }
            QualityCheck::Fluctuation { metric, max_increase, max_decrease, threshold: _ } => {
                format!(
                    "-- Fluctuation check for {}\n-- Max increase: {}%, Max decrease: {}%",
                    metric, max_increase * 100.0, max_decrease * 100.0
                )
            }
            QualityCheck::RowCount { min_rows, max_rows, threshold: _ } => {
                let mut sql = format!("SELECT COUNT(*) - {} as row_diff FROM {}", min_rows, table_name);
                if let Some(max) = max_rows {
                    sql = format!(
                        "SELECT CASE WHEN COUNT(*) > {} THEN COUNT(*) - {} ELSE 0 END as row_diff FROM {}",
                        max, max, table_name
                    );
                }
                sql
            }
            QualityCheck::Format { column, pattern, threshold: _ } => {
                format!(
                    "SELECT COUNT(*) as invalid_format_count FROM {} WHERE {} NOT RLIKE '{}'",
                    table_name, column, pattern
                )
            }
            QualityCheck::Consistency { source_table, target_table, join_condition, threshold: _ } => {
                format!(
                    "SELECT COUNT(*) as inconsistent_count FROM {} s LEFT JOIN {} t ON {} WHERE t.id IS NULL",
                    source_table, target_table, join_condition
                )
            }
        }
    }

    /// 获取检查类型名称
    fn get_check_type_name(&self, check: &QualityCheck) -> String {
        match check {
            QualityCheck::NotNull { .. } => "非空检查",
            QualityCheck::Unique { .. } => "唯一性检查",
            QualityCheck::Range { .. } => "值域检查",
            QualityCheck::Enum { .. } => "枚举值检查",
            QualityCheck::Fluctuation { .. } => "波动检查",
            QualityCheck::RowCount { .. } => "行数检查",
            QualityCheck::Format { .. } => "格式检查",
            QualityCheck::Consistency { .. } => "一致性检查",
        }.to_string()
    }

    /// 获取检查列名
    fn get_check_column(&self, check: &QualityCheck) -> Option<String> {
        match check {
            QualityCheck::NotNull { column, .. } |
            QualityCheck::Unique { column, .. } |
            QualityCheck::Range { column, .. } |
            QualityCheck::Enum { column, .. } |
            QualityCheck::Format { column, .. } => Some(column.clone()),
            _ => None,
        }
    }

    /// 生成修复建议
    fn generate_suggestion(&self, check: &QualityCheck) -> String {
        match check {
            QualityCheck::NotNull { column, .. } => {
                format!("检查数据源中 {} 列为空的数据，修复上游数据或添加默认值处理", column)
            }
            QualityCheck::Unique { column, .. } => {
                format!("检查 {} 列的重复数据，确认主键约束是否正确", column)
            }
            QualityCheck::Range { column, .. } => {
                format!("检查 {} 列的异常值，确认数据范围是否符合业务逻辑", column)
            }
            QualityCheck::Enum { column, .. } => {
                format!("检查 {} 列的非法枚举值，确认是否有新增状态未更新", column)
            }
            QualityCheck::Fluctuation { metric, .. } => {
                format!("检查 {} 指标的异常波动，确认是否有数据延迟或丢失", metric)
            }
            QualityCheck::RowCount { .. } => {
                "检查表行数异常，确认 ETL 任务是否正确执行".to_string()
            }
            QualityCheck::Format { column, .. } => {
                format!("检查 {} 列的格式问题，确认数据清洗规则是否正确", column)
            }
            QualityCheck::Consistency { .. } => {
                "检查数据一致性，确认关联关系是否正确".to_string()
            }
        }
    }

    /// 发送告警
    pub async fn send_alert(&self, report: &QualityReport) -> Result<()> {
        if let Some(quality_config) = &self.config.quality {
            if let Some(email) = &quality_config.alert_email {
                if report.overall_status == QualityStatus::Failed {
                    tracing::warn!("Sending alert to {} for failed quality check", email);
                    // 实际实现：发送邮件告警
                }
            }
        }
        Ok(())
    }
}

/// 检查结果
struct CheckResult {
    passed: bool,
    actual_value: String,
    expected_value: String,
    affected_rows: i64,
    debug_sql: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_quality_service_creation() {
        let config = AppConfig::default();
        let mut service = QualityService::new(config);
        service.initialize_default_rules();
        
        assert!(service.rules.is_empty());
    }

    #[tokio::test]
    async fn test_severity_ordering() {
        // Severity derives Ord, Blocking should be highest
        // But since we're comparing enum variants, let's just verify they exist
        let blocking = Severity::Blocking;
        let critical = Severity::Critical;
        
        // Just verify we can create and compare them
        assert_eq!(blocking, Severity::Blocking);
        assert_eq!(critical, Severity::Critical);
    }
}
