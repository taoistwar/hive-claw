//! 按钮组件

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
    Frame,
};

/// 按钮状态
#[derive(Debug, Clone, PartialEq)]
pub enum ButtonState {
    Normal,
    Hover,
    Disabled,
}

/// 按钮组件
pub struct Button {
    pub label: String,
    pub state: ButtonState,
}

impl Button {
    pub fn new(label: &str) -> Self {
        Self {
            label: label.to_string(),
            state: ButtonState::Normal,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let style = match self.state {
            ButtonState::Normal => Style::default()
                .fg(Color::White)
                .bg(Color::Blue),
            ButtonState::Hover => Style::default()
                .fg(Color::White)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
            ButtonState::Disabled => Style::default()
                .fg(Color::DarkGray)
                .bg(Color::Black),
        };

        let button = Paragraph::new(Line::from(self.label.as_str()))
            .style(style)
            .block(Block::default().borders(Borders::ALL));

        frame.render_widget(button, area);
    }
}
