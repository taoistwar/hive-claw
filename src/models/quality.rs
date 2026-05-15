//! 数据质量模型

use serde::{Deserialize, Serialize};

/// 问题严重程度
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Severity {
    Blocker,
    Critical,
    Major,
    Minor,
}

/// 质量问题
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityIssue {
    pub table: String,
    pub rule_id: String,
    pub severity: Severity,
    pub message: String,
}
