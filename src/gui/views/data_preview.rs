//! 数据预览视图（Mock）

/// 数据预览视图结构
pub struct DataPreview;

impl DataPreview {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DataPreview {
    fn default() -> Self {
        Self::new()
    }
}
