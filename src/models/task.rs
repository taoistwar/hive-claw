//! 任务模型

use serde::{Deserialize, Serialize};

/// 任务模板类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskTemplate {
    SingleTableAgg,   // 单表聚合
    MultiTableJoin,   // 多表关联
    IncrementalSync,  // 增量同步
    FullSync,         // 全量同步
    Deduplication,    // 去重清洗
    SCD,              // 缓慢变化维
    MetricCalc,       // 指标计算
}

impl std::fmt::Display for TaskTemplate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskTemplate::SingleTableAgg => write!(f, "单表聚合"),
            TaskTemplate::MultiTableJoin => write!(f, "多表关联"),
            TaskTemplate::IncrementalSync => write!(f, "增量同步"),
            TaskTemplate::FullSync => write!(f, "全量同步"),
            TaskTemplate::Deduplication => write!(f, "去重清洗"),
            TaskTemplate::SCD => write!(f, "SCD"),
            TaskTemplate::MetricCalc => write!(f, "指标计算"),
        }
    }
}

/// 任务状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    Running,
    Success,
    Failed,
    Killed,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Pending => write!(f, "等待中"),
            TaskStatus::Running => write!(f, "运行中"),
            TaskStatus::Success => write!(f, "成功"),
            TaskStatus::Failed => write!(f, "失败"),
            TaskStatus::Killed => write!(f, "已终止"),
        }
    }
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Pending => "Pending",
            TaskStatus::Running => "Running",
            TaskStatus::Success => "Success",
            TaskStatus::Failed => "Failed",
            TaskStatus::Killed => "Killed",
        }
    }
}

/// 任务模型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub description: String,
    pub template: String,
    pub status: TaskStatus,
    pub created_at: String,
    pub updated_at: String,
}
