//! 设置中心视图（Mock）

/// 设置视图结构
pub struct SettingsView;

impl SettingsView {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SettingsView {
    fn default() -> Self {
        Self::new()
    }
}
