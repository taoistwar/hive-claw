//! 元数据浏览器视图

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Table, Row, TableState},
    Frame,
};

/// 元数据浏览器视图
pub struct MetadataBrowserView {
    pub databases: Vec<String>,
    pub tables: Vec<String>,
    pub selected_db_index: usize,
    pub selected_table_index: usize,
}

impl MetadataBrowserView {
    pub fn new() -> Self {
        Self {
            databases: Vec::new(),
            tables: Vec::new(),
            selected_db_index: 0,
            selected_table_index: 0,
        }
    }

    pub fn set_databases(&mut self, databases: Vec<String>) {
        self.databases = databases;
        if self.selected_db_index >= self.databases.len() && !self.databases.is_empty() {
            self.selected_db_index = self.databases.len() - 1;
        }
    }

    pub fn set_tables(&mut self, tables: Vec<String>) {
        self.tables = tables;
        if self.selected_table_index >= self.tables.len() && !self.tables.is_empty() {
            self.selected_table_index = self.tables.len() - 1;
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(30),
                Constraint::Percentage(70),
            ])
            .split(area);

        // 数据库列表
        let db_items: Vec<ListItem> = self.databases.iter().enumerate().map(|(i, db)| {
            let style = if i == self.selected_db_index {
                Style::default().bg(Color::DarkGray).add_modifier(ratatui::style::Modifier::REVERSED)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(db.as_str(), style)))
        }).collect();

        let db_list = List::new(db_items)
            .block(Block::default().borders(Borders::ALL).title("数据库"));
        frame.render_widget(db_list, chunks[0]);

        // 表列表
        let table_items: Vec<ListItem> = self.tables.iter().enumerate().map(|(i, table)| {
            let style = if i == self.selected_table_index {
                Style::default().bg(Color::DarkGray).add_modifier(ratatui::style::Modifier::REVERSED)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(table.as_str(), style)))
        }).collect();

        let table_list = List::new(table_items)
            .block(Block::default().borders(Borders::ALL).title("表"));
        frame.render_widget(table_list, chunks[1]);
    }

    pub fn select_db_next(&mut self) {
        if self.selected_db_index < self.databases.len().saturating_sub(1) {
            self.selected_db_index += 1;
        }
    }

    pub fn select_db_previous(&mut self) {
        if self.selected_db_index > 0 {
            self.selected_db_index -= 1;
        }
    }

    pub fn select_table_next(&mut self) {
        if self.selected_table_index < self.tables.len().saturating_sub(1) {
            self.selected_table_index += 1;
        }
    }

    pub fn select_table_previous(&mut self) {
        if self.selected_table_index > 0 {
            self.selected_table_index -= 1;
        }
    }

    pub fn selected_database(&self) -> Option<&String> {
        self.databases.get(self.selected_db_index)
    }

    pub fn selected_table(&self) -> Option<&String> {
        self.tables.get(self.selected_table_index)
    }
}
