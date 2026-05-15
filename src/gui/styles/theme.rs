//! 主题样式

use ratatui::style::{Color, Modifier, Style};

use super::colors::Theme;

/// 默认文本样式
pub fn default_text() -> Style {
    Style::default().fg(Theme::TEXT_PRIMARY)
}

/// 次要文本样式
pub fn secondary_text() -> Style {
    Style::default().fg(Theme::TEXT_SECONDARY)
}

/// 禁用文本样式
pub fn disabled_text() -> Style {
    Style::default().fg(Theme::TEXT_DISABLED)
}

/// 标题样式
pub fn title() -> Style {
    Style::default()
        .fg(Theme::PRIMARY)
        .add_modifier(Modifier::BOLD)
}

/// 成功样式
pub fn success() -> Style {
    Style::default().fg(Theme::SUCCESS)
}

/// 警告样式
pub fn warning() -> Style {
    Style::default().fg(Theme::WARNING)
}

/// 错误样式
pub fn error() -> Style {
    Style::default().fg(Theme::ERROR)
}

/// 高亮样式（选中项）
pub fn highlight() -> Style {
    Style::default()
        .bg(Theme::BACKGROUND_LIGHT)
        .add_modifier(Modifier::REVERSED)
}

/// 边框样式
pub fn border() -> Style {
    Style::default().fg(Theme::BORDER)
}

/// 聚焦边框样式
pub fn border_focus() -> Style {
    Style::default().fg(Theme::BORDER_FOCUS)
}
