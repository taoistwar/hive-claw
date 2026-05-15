//! 输入框组件

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
    Frame,
};

/// 输入框组件
pub struct Input {
    pub label: String,
    pub value: String,
    pub cursor_position: usize,
    pub is_focused: bool,
}

impl Input {
    pub fn new(label: &str) -> Self {
        Self {
            label: label.to_string(),
            value: String::new(),
            cursor_position: 0,
            is_focused: false,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let border_style = if self.is_focused {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::White)
        };

        let display_value = if self.is_focused {
            format!("{}▌", self.value)
        } else {
            self.value.clone()
        };

        let input = Paragraph::new(Line::from(display_value))
            .style(Style::default().fg(Color::White))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(self.label.as_str())
                    .border_style(border_style),
            );

        frame.render_widget(input, area);
    }

    pub fn insert_char(&mut self, ch: char) {
        self.value.insert(self.cursor_position, ch);
        self.cursor_position += 1;
    }

    pub fn delete_char(&mut self) {
        if self.cursor_position > 0 {
            self.cursor_position -= 1;
            self.value.remove(self.cursor_position);
        }
    }

    pub fn move_cursor_left(&mut self) {
        if self.cursor_position > 0 {
            self.cursor_position -= 1;
        }
    }

    pub fn move_cursor_right(&mut self) {
        if self.cursor_position < self.value.len() {
            self.cursor_position += 1;
        }
    }
}
