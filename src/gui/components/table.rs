//! 表格组件

use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Row, Table, TableState},
    Frame,
};

/// 表格组件
pub struct DataTable {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub state: TableState,
}

impl DataTable {
    pub fn new(headers: Vec<String>) -> Self {
        let mut state = TableState::default();
        state.select(Some(0));
        
        Self {
            headers,
            rows: Vec::new(),
            state,
        }
    }

    pub fn add_row(&mut self, row: Vec<String>) {
        self.rows.push(row);
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let header_row = Row::new(
            self.headers.iter().map(|h| Line::from(h.as_str()))
        )
        .style(Style::default().fg(Color::Yellow))
        .bottom_margin(1);

        let data_rows: Vec<Row> = self.rows.iter().map(|row| {
            Row::new(row.iter().map(|cell| Line::from(cell.as_str())))
        }).collect();

        let table = Table::new(data_rows)
            .header(header_row)
            .block(Block::default().borders(Borders::ALL))
            .highlight_style(Style::default().bg(Color::DarkGray))
            .highlight_symbol("▶ ")
            .widths(&[Constraint::Percentage(50), Constraint::Percentage(50)]);

        let mut state = self.state.clone();
        frame.render_stateful_widget(table, area, &mut state);
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.state.selected()
    }

    pub fn select_next(&mut self) {
        if let Some(selected) = self.state.selected() {
            if selected < self.rows.len().saturating_sub(1) {
                self.state.select(Some(selected + 1));
            }
        }
    }

    pub fn select_previous(&mut self) {
        if let Some(selected) = self.state.selected() {
            if selected > 0 {
                self.state.select(Some(selected - 1));
            }
        }
    }
}
