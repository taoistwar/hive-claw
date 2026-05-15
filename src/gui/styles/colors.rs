//! 颜色主题

use ratatui::style::Color;

/// 应用颜色主题
pub struct Theme;

impl Theme {
    // 主色调
    pub const PRIMARY: Color = Color::Blue;
    pub const PRIMARY_DARK: Color = Color::Blue;
    pub const SECONDARY: Color = Color::Green;
    
    // 状态颜色
    pub const SUCCESS: Color = Color::Green;
    pub const WARNING: Color = Color::Yellow;
    pub const ERROR: Color = Color::Red;
    pub const INFO: Color = Color::Cyan;
    
    // 文本颜色
    pub const TEXT_PRIMARY: Color = Color::White;
    pub const TEXT_SECONDARY: Color = Color::Gray;
    pub const TEXT_DISABLED: Color = Color::DarkGray;
    
    // 背景颜色
    pub const BACKGROUND: Color = Color::Black;
    pub const BACKGROUND_LIGHT: Color = Color::DarkGray;
    
    // 边框颜色
    pub const BORDER: Color = Color::White;
    pub const BORDER_FOCUS: Color = Color::Yellow;
}

/// 获取状态对应的颜色
pub fn status_color(status: &str) -> Color {
    match status.to_lowercase().as_str() {
        "success" | "succeeded" | "completed" => Theme::SUCCESS,
        "running" | "processing" => Theme::WARNING,
        "failed" | "error" => Theme::ERROR,
        "pending" | "waiting" => Theme::INFO,
        _ => Theme::TEXT_SECONDARY,
    }
}
