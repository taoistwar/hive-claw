//! 模态框组件

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// 模态框类型
#[derive(Debug, Clone, PartialEq)]
pub enum ModalType {
    Info,
    Warning,
    Error,
    Confirm,
}

/// 模态框组件
pub struct Modal {
    pub title: String,
    pub message: String,
    pub modal_type: ModalType,
    pub confirmed: bool,
}

impl Modal {
    pub fn new(title: &str, message: &str, modal_type: ModalType) -> Self {
        Self {
            title: title.to_string(),
            message: message.to_string(),
            modal_type,
            confirmed: false,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        // 计算模态框大小（屏幕中央 60% 区域）
        let modal_area = centered_rect(60, 50, area);

        // 清空背景
        frame.render_widget(Clear, modal_area);

        // 根据类型设置样式
        let (border_color, title_color) = match self.modal_type {
            ModalType::Info => (Color::Blue, Color::Blue),
            ModalType::Warning => (Color::Yellow, Color::Yellow),
            ModalType::Error => (Color::Red, Color::Red),
            ModalType::Confirm => (Color::Green, Color::Green),
        };

        // 创建内容
        let mut lines = vec![Line::from("")];
        for line in self.message.lines() {
            lines.push(Line::from(Span::raw(line)));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "按 Enter 确认 | 按 Esc 取消",
            Style::default().fg(Color::Gray),
        )));

        let modal = Paragraph::new(lines)
            .block(
                Block::default()
                    .title(self.title.as_str())
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border_color))
                    .title_style(Style::default().fg(title_color).add_modifier(Modifier::BOLD)),
            )
            .style(Style::default().fg(Color::White));

        frame.render_widget(modal, modal_area);
    }
}

/// 创建居中矩形
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
