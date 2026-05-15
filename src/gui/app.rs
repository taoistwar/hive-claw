//! GUI 应用入口 (TUI 版本)

use crate::config::AppConfig;
use crate::models::task::{Task, TaskStatus};
use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Tabs},
    Frame, Terminal,
};
use std::io;

/// GUI 应用状态
struct App {
    should_quit: bool,
    active_tab: usize,
    tab_titles: Vec<String>,
    tasks: Vec<Task>,
    databases: Vec<String>,
    tables: Vec<String>,
}

impl App {
    fn new() -> Self {
        Self {
            should_quit: false,
            active_tab: 0,
            tab_titles: vec![
                "任务管理".to_string(),
                "元数据".to_string(),
                "创建向导".to_string(),
                "数据质量".to_string(),
                "设置".to_string(),
            ],
            tasks: vec![
                Task {
                    id: "1".to_string(),
                    name: "sync_user_daily".to_string(),
                    description: "每日用户数据同步".to_string(),
                    template: "增量同步".to_string(),
                    status: TaskStatus::Success,
                    created_at: "2026-05-15 02:00:00".to_string(),
                    updated_at: "2026-05-15 02:05:00".to_string(),
                },
                Task {
                    id: "2".to_string(),
                    name: "agg_order_hourly".to_string(),
                    description: "每小时订单聚合".to_string(),
                    template: "单表聚合".to_string(),
                    status: TaskStatus::Running,
                    created_at: "2026-05-15 03:00:00".to_string(),
                    updated_at: "2026-05-15 03:30:00".to_string(),
                },
            ],
            databases: vec!["default".to_string(), "ods".to_string(), "dwd".to_string()],
            tables: vec!["users".to_string(), "orders".to_string()],
        }
    }

    fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        while !self.should_quit {
            terminal.draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Min(0),
                        Constraint::Length(3),
                    ])
                    .split(f.size());

                let title = Tabs::new(self.tab_titles.iter().map(|t| Line::from(t.as_str())).collect())
                    .block(Block::default().borders(Borders::ALL).title("离线分析 AI Agent v0.1.0"))
                    .select(self.active_tab)
                    .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
                f.render_widget(title, chunks[0]);

                match self.active_tab {
                    0 => self.render_tasks(f, chunks[1]),
                    1 => self.render_metadata(f, chunks[1]),
                    2 => self.render_placeholder(f, chunks[1], "创建向导"),
                    3 => self.render_placeholder(f, chunks[1], "数据质量"),
                    4 => self.render_placeholder(f, chunks[1], "设置"),
                    _ => {}
                }

                let status = Paragraph::new(Line::from("q:退出 | ←/→:切换 | Enter:操作"))
                    .style(Style::default().fg(Color::Gray))
                    .block(Block::default().borders(Borders::ALL));
                f.render_widget(status, chunks[2]);
            })?;

            if event::poll(std::time::Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    match key.code {
                        KeyCode::Char('q') => self.should_quit = true,
                        KeyCode::Left if self.active_tab > 0 => self.active_tab -= 1,
                        KeyCode::Right if self.active_tab < self.tab_titles.len() - 1 => self.active_tab += 1,
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }

    fn render_tasks(&self, f: &mut Frame, area: ratatui::layout::Rect) {
        let items: Vec<ListItem> = self.tasks.iter().map(|t| {
            let color = match t.status {
                TaskStatus::Success => Color::Green,
                TaskStatus::Running => Color::Yellow,
                TaskStatus::Pending => Color::Gray,
                _ => Color::White,
            };
            ListItem::new(vec![
                Line::from(format!("{} [{}] - {}", t.name, t.template, t.description)),
                Line::from(Span::styled(format!("状态：{:?}", t.status), Style::default().fg(color))),
            ])
        }).collect();
        f.render_widget(List::new(items).block(Block::default().borders(Borders::ALL).title("任务列表")), area);
    }

    fn render_metadata(&self, f: &mut Frame, area: ratatui::layout::Rect) {
        let text = format!("数据库:\n{}\n\n表:\n{}", 
            self.databases.join("\n"), 
            self.tables.join("\n"));
        f.render_widget(Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("元数据")), area);
    }

    fn render_placeholder(&self, f: &mut Frame, area: ratatui::layout::Rect, title: &str) {
        f.render_widget(Paragraph::new(format!("{} - 功能开发中...", title))
            .block(Block::default().borders(Borders::ALL).title(title)), area);
    }
}

pub fn run_app(_config: AppConfig) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let res = app.run(&mut terminal);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    res
}
