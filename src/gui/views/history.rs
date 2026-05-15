//! 执行历史视图（Mock）

/// 历史视图结构
pub struct HistoryView;

impl HistoryView {
    pub fn new() -> Self {
        Self
    }
}

impl Default for HistoryView {
    fn default() -> Self {
        Self::new()
    }
}
