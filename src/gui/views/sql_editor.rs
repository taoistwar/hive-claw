//! SQL 编辑器视图（Mock）

/// SQL 编辑器结构
pub struct SqlEditor;

impl SqlEditor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SqlEditor {
    fn default() -> Self {
        Self::new()
    }
}
