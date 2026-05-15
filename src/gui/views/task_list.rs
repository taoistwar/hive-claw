//! 任务列表视图

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use crate::models::task::Task;

/// 任务列表视图
pub struct TaskListView {
    pub tasks: Vec<Task>,
    pub selected_index: usize,
    pub scroll_offset: usize,
}

impl TaskListView {
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            selected_index: 0,
            scroll_offset: 0,
        }
    }

    pub fn set_tasks(&mut self, tasks: Vec<Task>) {
        self.tasks = tasks;
        if self.selected_index >= self.tasks.len() && !self.tasks.is_empty() {
            self.selected_index = self.tasks.len() - 1;
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(0),
            ])
            .split(area);

        // 标题
        let title = Paragraph::new(Line::from(Span::styled(
            format!("任务列表 (共 {} 个任务)", self.tasks.len()),
            Style::default().fg(Color::Yellow).add_modifier(ratatui::style::Modifier::BOLD),
        )))
        .block(Block::default().borders(Borders::BOTTOM));
        frame.render_widget(title, chunks[0]);

        // 任务列表
        let items: Vec<ListItem> = self.tasks.iter().enumerate().map(|(i, task)| {
            let status_color = match task.status.as_str() {
                "Success" => Color::Green,
                "Running" => Color::Yellow,
                "Failed" => Color::Red,
                "Pending" => Color::Gray,
                "Killed" => Color::Magenta,
                _ => Color::White,
            };

            let style = if i == self.selected_index {
                Style::default().bg(Color::DarkGray).add_modifier(ratatui::style::Modifier::REVERSED)
            } else {
                Style::default()
            };

            let content = vec![
                Line::from(Span::styled(
                    format!("{} [{}] - {}", task.name, task.template, task.description),
                    style,
                )),
                Line::from(Span::styled(
                    format!("状态：{} | 更新：{}", task.status, task.updated_at),
                    style.fg(status_color),
                )),
            ];

            ListItem::new(content)
        }).collect();

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title("任务列表"));
        frame.render_widget(list, chunks[1]);
    }

    pub fn select_next(&mut self) {
        if self.selected_index < self.tasks.len().saturating_sub(1) {
            self.selected_index += 1;
        }
    }

    pub fn select_previous(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn selected_task(&self) -> Option<&Task> {
        self.tasks.get(self.selected_index)
    }
}
