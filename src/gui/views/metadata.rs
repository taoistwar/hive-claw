//! 元数据管理视图（Mock）

/// 元数据视图结构
pub struct MetadataView;

impl MetadataView {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MetadataView {
    fn default() -> Self {
        Self::new()
    }
}
