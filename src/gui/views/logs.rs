//! 日志查看视图（Mock）

/// 日志视图结构
pub struct LogsView;

impl LogsView {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LogsView {
    fn default() -> Self {
        Self::new()
    }
}
