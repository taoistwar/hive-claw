//! TUI 应用入口 - 完整交互版本

use crate::config::AppConfig;
use crate::models::task::{Task, TaskStatus};
use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs, Wrap},
    Frame, Terminal,
};
use std::io;

/// 输入框组件
#[derive(Clone)]
struct InputField {
    label: String,
    value: String,
    cursor_position: usize,
}

impl InputField {
    fn new(label: &str) -> Self {
        Self {
            label: label.to_string(),
            value: String::new(),
            cursor_position: 0,
        }
    }

    fn insert_char(&mut self, c: char) {
        self.value.insert(self.cursor_position, c);
        self.cursor_position += 1;
    }

    fn delete_char(&mut self) {
        if self.cursor_position > 0 {
            self.cursor_position -= 1;
            self.value.remove(self.cursor_position);
        }
    }

    fn move_left(&mut self) {
        if self.cursor_position > 0 {
            self.cursor_position -= 1;
        }
    }

    fn move_right(&mut self) {
        if self.cursor_position < self.value.len() {
            self.cursor_position += 1;
        }
    }
}

/// 确认对话框
struct ConfirmDialog {
    title: String,
    message: String,
    confirmed: Option<bool>,
}

impl ConfirmDialog {
    fn new(title: &str, message: &str) -> Self {
        Self {
            title: title.to_string(),
            message: message.to_string(),
            confirmed: None,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect) {
        let dialog_area = centered_rect(60, 40, area);
        
        frame.render_widget(Clear, dialog_area);
        
        let block = Block::default()
            .title(self.title.as_str())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));
        
        frame.render_widget(block, dialog_area);
        
        let inner = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(3),
            ])
            .margin(1)
            .split(dialog_area);
        
        let message = Paragraph::new(self.message.as_str())
            .wrap(Wrap { trim: true });
        frame.render_widget(message, inner[1]);
        
        let buttons = Paragraph::new(Line::from(vec![
            Span::styled(" [Y] 确认 ", Style::default().fg(Color::Green)),
            Span::raw(" | "),
            Span::styled(" [N] 取消 ", Style::default().fg(Color::Red)),
        ]));
        frame.render_widget(buttons, inner[2]);
    }
}

/// 消息弹窗
struct MessagePopup {
    title: String,
    message: String,
    popup_type: PopupType,
}

enum PopupType {
    Info,
    Success,
    Error,
}

impl MessagePopup {
    fn info(title: &str, message: &str) -> Self {
        Self {
            title: title.to_string(),
            message: message.to_string(),
            popup_type: PopupType::Info,
        }
    }

    fn success(title: &str, message: &str) -> Self {
        Self {
            title: title.to_string(),
            message: message.to_string(),
            popup_type: PopupType::Success,
        }
    }

    fn error(title: &str, message: &str) -> Self {
        Self {
            title: title.to_string(),
            message: message.to_string(),
            popup_type: PopupType::Error,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect) {
        let popup_area = centered_rect(50, 30, area);
        
        frame.render_widget(Clear, popup_area);
        
        let color = match self.popup_type {
            PopupType::Info => Color::Blue,
            PopupType::Success => Color::Green,
            PopupType::Error => Color::Red,
        };
        
        let block = Block::default()
            .title(self.title.as_str())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(color));
        
        frame.render_widget(block, popup_area);
        
        let message = Paragraph::new(self.message.as_str())
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(Color::White));
        
        let inner = Layout::default()
            .margin(1)
            .split(popup_area);
        
        frame.render_widget(message, inner[0]);
    }
}

/// 应用状态
struct App {
    should_quit: bool,
    active_tab: usize,
    tab_titles: Vec<String>,
    tasks: Vec<Task>,
    selected_task: usize,
    databases: Vec<String>,
    selected_database: usize,
    tables: Vec<String>,
    selected_table: usize,
    show_confirm: bool,
    confirm_dialog: Option<ConfirmDialog>,
    show_message: bool,
    message_popup: Option<MessagePopup>,
    input_mode: bool,
    input_field: Option<InputField>,
    input_target: InputTarget,
}

enum InputTarget {
    TaskName,
    TaskDescription,
    None,
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
                Task {
                    id: "3".to_string(),
                    name: "dwd_user_profile".to_string(),
                    description: "用户画像全量同步".to_string(),
                    template: "全量同步".to_string(),
                    status: TaskStatus::Pending,
                    created_at: "2026-05-15 04:00:00".to_string(),
                    updated_at: "2026-05-15 04:00:00".to_string(),
                },
            ],
            selected_task: 0,
            databases: vec!["default".to_string(), "ods".to_string(), "dwd".to_string(), "dwm".to_string()],
            selected_database: 0,
            tables: vec!["users".to_string(), "orders".to_string(), "products".to_string()],
            selected_table: 0,
            show_confirm: false,
            confirm_dialog: None,
            show_message: false,
            message_popup: None,
            input_mode: false,
            input_field: None,
            input_target: InputTarget::None,
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

                // 标签页
                let title = Tabs::new(
                    self.tab_titles.iter().map(|t| Line::from(t.as_str())).collect()
                )
                .block(Block::default().borders(Borders::ALL).title("离线分析 AI Agent v0.1.0"))
                .select(self.active_tab)
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
                f.render_widget(title, chunks[0]);

                // 内容区域
                match self.active_tab {
                    0 => self.render_tasks(f, chunks[1]),
                    1 => self.render_metadata(f, chunks[1]),
                    2 => self.render_wizard(f, chunks[1]),
                    3 => self.render_quality(f, chunks[1]),
                    4 => self.render_settings(f, chunks[1]),
                    _ => {}
                }

                // 状态栏
                let status = self.render_statusbar();
                f.render_widget(status, chunks[2]);

                // 弹窗
                if self.show_confirm {
                    if let Some(ref dialog) = self.confirm_dialog {
                        dialog.render(f, f.size());
                    }
                }

                if self.show_message {
                    if let Some(ref popup) = self.message_popup {
                        popup.render(f, f.size());
                    }
                }

                // 输入模式
                if self.input_mode {
                    if let Some(ref field) = self.input_field {
                        self.render_input(f, f.size(), field);
                    }
                }
            })?;

            self.handle_input()?;
        }
        Ok(())
    }

    fn handle_input(&mut self) -> Result<()> {
        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    return Ok(());
                }

                // 输入模式处理
                if self.input_mode {
                    match key.code {
                        KeyCode::Esc => {
                            self.input_mode = false;
                            self.input_field = None;
                        }
                        KeyCode::Enter => {
                            self.input_mode = false;
                            if let Some(field) = &self.input_field {
                                match self.input_target {
                                    InputTarget::TaskName => {
                                        if let Some(task) = self.tasks.get_mut(self.selected_task) {
                                            task.name = field.value.clone();
                                        }
                                        self.show_message = true;
                                        self.message_popup = Some(MessagePopup::success(
                                            "成功",
                                            &format!("任务名称已更新为：{}", field.value)
                                        ));
                                    }
                                    InputTarget::TaskDescription => {
                                        if let Some(task) = self.tasks.get_mut(self.selected_task) {
                                            task.description = field.value.clone();
                                        }
                                        self.show_message = true;
                                        self.message_popup = Some(MessagePopup::success(
                                            "成功",
                                            &format!("任务描述已更新为：{}", field.value)
                                        ));
                                    }
                                    InputTarget::None => {}
                                }
                            }
                            self.input_field = None;
                        }
                        KeyCode::Char(c) => {
                            if let Some(ref mut field) = self.input_field {
                                field.insert_char(c);
                            }
                        }
                        KeyCode::Backspace => {
                            if let Some(ref mut field) = self.input_field {
                                field.delete_char();
                            }
                        }
                        KeyCode::Left => {
                            if let Some(ref mut field) = self.input_field {
                                field.move_left();
                            }
                        }
                        KeyCode::Right => {
                            if let Some(ref mut field) = self.input_field {
                                field.move_right();
                            }
                        }
                        _ => {}
                    }
                    return Ok(());
                }

                // 确认对话框处理
                if self.show_confirm {
                    match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            self.show_confirm = false;
                            if let Some(dialog) = self.confirm_dialog.take() {
                                if dialog.title.contains("删除") {
                                    self.tasks.remove(self.selected_task);
                                    if self.selected_task >= self.tasks.len() && self.selected_task > 0 {
                                        self.selected_task -= 1;
                                    }
                                    self.show_message = true;
                                    self.message_popup = Some(MessagePopup::info("已删除", "任务已成功删除"));
                                } else if dialog.title.contains("运行") {
                                    if let Some(task) = self.tasks.get_mut(self.selected_task) {
                                        task.status = TaskStatus::Running;
                                    }
                                    self.show_message = true;
                                    self.message_popup = Some(MessagePopup::info("运行中", "任务已开始执行"));
                                }
                            }
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                            self.show_confirm = false;
                            self.confirm_dialog = None;
                        }
                        _ => {}
                    }
                    return Ok(());
                }

                // 消息弹窗处理
                if self.show_message {
                    match key.code {
                        KeyCode::Enter | KeyCode::Esc => {
                            self.show_message = false;
                            self.message_popup = None;
                        }
                        _ => {}
                    }
                    return Ok(());
                }

                // 全局快捷键
                match key.code {
                    KeyCode::Char('q') => self.should_quit = true,
                    KeyCode::Tab => {
                        self.active_tab = (self.active_tab + 1) % self.tab_titles.len();
                    }
                    KeyCode::BackTab => {
                        self.active_tab = if self.active_tab == 0 {
                            self.tab_titles.len() - 1
                        } else {
                            self.active_tab - 1
                        };
                    }
                    KeyCode::Left if self.active_tab > 0 => self.active_tab -= 1,
                    KeyCode::Right if self.active_tab < self.tab_titles.len() - 1 => self.active_tab += 1,
                    
                    // 任务列表操作
                    KeyCode::Up if self.active_tab == 0 && self.selected_task > 0 => {
                        self.selected_task -= 1;
                    }
                    KeyCode::Down if self.active_tab == 0 && self.selected_task < self.tasks.len().saturating_sub(1) => {
                        self.selected_task += 1;
                    }
                    KeyCode::Enter if self.active_tab == 0 && !self.tasks.is_empty() => {
                        self.show_confirm = true;
                        self.confirm_dialog = Some(ConfirmDialog::new(
                            "运行任务",
                            &format!("确定要运行任务 '{}' 吗？", self.tasks[self.selected_task].name)
                        ));
                    }
                    KeyCode::Delete if self.active_tab == 0 && !self.tasks.is_empty() => {
                        self.show_confirm = true;
                        self.confirm_dialog = Some(ConfirmDialog::new(
                            "删除任务",
                            &format!("确定要删除任务 '{}' 吗？此操作不可恢复。", self.tasks[self.selected_task].name)
                        ));
                    }
                    KeyCode::Char('e') if self.active_tab == 0 && !self.tasks.is_empty() => {
                        self.input_mode = true;
                        self.input_target = InputTarget::TaskName;
                        self.input_field = Some(InputField::new("任务名称"));
                        if let Some(task) = self.tasks.get(self.selected_task) {
                            let mut field = InputField::new("任务名称");
                            field.value = task.name.clone();
                            field.cursor_position = field.value.len();
                            self.input_field = Some(field);
                        }
                    }
                    
                    // 元数据浏览操作
                    KeyCode::Up if self.active_tab == 1 && self.selected_database > 0 => {
                        self.selected_database -= 1;
                        self.selected_table = 0;
                    }
                    KeyCode::Down if self.active_tab == 1 && self.selected_database < self.databases.len().saturating_sub(1) => {
                        self.selected_database += 1;
                        self.selected_table = 0;
                    }
                    
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn render_tasks(&self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self.tasks.iter().enumerate().map(|(i, t)| {
            let color = match t.status {
                TaskStatus::Success => Color::Green,
                TaskStatus::Running => Color::Yellow,
                TaskStatus::Pending => Color::Gray,
                TaskStatus::Failed => Color::Red,
                TaskStatus::Killed => Color::Magenta,
            };
            
            let style = if i == self.selected_task {
                Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            
            ListItem::new(vec![
                Line::from(Span::styled(
                    format!(" {} [{}] - {}", t.name, t.template, t.description),
                    style
                )),
                Line::from(Span::styled(
                    format!("   状态：{:?} | 更新：{}", t.status, t.updated_at),
                    Style::default().fg(color)
                )),
            ])
        }).collect();

        let help = if !self.tasks.is_empty() {
            "↑/↓:选择 | Enter:运行 | e:编辑 | Del:删除"
        } else {
            "无任务"
        };

        let list = List::new(items)
            .block(Block::default()
                .borders(Borders::ALL)
                .title(format!("任务列表 (共 {} 个) | {}", self.tasks.len(), help)));
        
        f.render_widget(list, area);
    }

    fn render_metadata(&self, f: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(area);

        // 数据库列表
        let db_items: Vec<ListItem> = self.databases.iter().enumerate().map(|(i, db)| {
            let style = if i == self.selected_database {
                Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(db.as_str(), style)))
        }).collect();

        let db_list = List::new(db_items)
            .block(Block::default()
                .borders(Borders::ALL)
                .title("数据库 | ↑/↓:选择"));
        
        f.render_widget(db_list, chunks[0]);

        // 表列表
        let table_items: Vec<ListItem> = self.tables.iter().enumerate().map(|(i, table)| {
            let style = if i == self.selected_table {
                Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(
                format!("{} (表)", table),
                style
            )))
        }).collect();

        let table_list = List::new(table_items)
            .block(Block::default()
                .borders(Borders::ALL)
                .title(format!("表 ({}.{})", self.databases[self.selected_database], self.tables[self.selected_table])));
        
        f.render_widget(table_list, chunks[1]);
    }

    fn render_wizard(&self, f: &mut Frame, area: Rect) {
        let steps = vec![
            "1. 选择任务模板",
            "2. 填写任务信息",
            "3. 选择数据表",
            "4. 配置调度",
            "5. 质量规则",
            "6. 预览配置",
            "7. 提交创建",
        ];

        let content: Vec<Line> = steps.iter().map(|s| {
            Line::from(Span::styled(*s, Style::default().fg(Color::Cyan)))
        }).collect();

        let wizard = Paragraph::new(content)
            .block(Block::default()
                .borders(Borders::ALL)
                .title("创建向导"))
            .wrap(Wrap { trim: true });
        
        f.render_widget(wizard, area);
    }

    fn render_quality(&self, f: &mut Frame, area: Rect) {
        let checks = vec![
            ("非空检查", "验证字段不为 NULL"),
            ("唯一性检查", "验证字段值唯一"),
            ("值域检查", "验证数值在范围内"),
            ("枚举值检查", "验证值在枚举列表中"),
            ("波动检查", "验证数据波动在阈值内"),
            ("行数检查", "验证表行数在预期范围"),
            ("格式检查", "验证数据格式 (日期/邮箱等)"),
            ("一致性检查", "验证跨表数据一致性"),
        ];

        let content: Vec<Line> = checks.iter().flat_map(|(name, desc)| {
            vec![
                Line::from(Span::styled(*name, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))),
                Line::from(Span::raw(*desc)),
                Line::from(""),
            ]
        }).collect();

        let quality = Paragraph::new(content)
            .block(Block::default()
                .borders(Borders::ALL)
                .title("数据质量检查 (8 类)"));
        
        f.render_widget(quality, area);
    }

    fn render_settings(&self, f: &mut Frame, area: Rect) {
        let settings = vec![
            "Azkaban 连接配置",
            "Hive Metastore 配置",
            "Git 仓库配置",
            "AI 服务配置",
            "缓存策略",
        ];

        let content: Vec<Line> = settings.iter().map(|s| {
            Line::from(Span::styled(
                format!("  ○ {}", s),
                Style::default().fg(Color::Yellow)
            ))
        }).collect();

        let settings_view = Paragraph::new(content)
            .block(Block::default()
                .borders(Borders::ALL)
                .title("系统设置"))
            .wrap(Wrap { trim: true });
        
        f.render_widget(settings_view, area);
    }

    fn render_input(&self, f: &mut Frame, area: Rect, field: &InputField) {
        let popup_area = centered_rect(50, 20, area);
        
        f.render_widget(Clear, popup_area);
        
        let display_value = format!("{}▌", field.value);
        
        let input = Paragraph::new(Line::from(display_value))
            .style(Style::default().fg(Color::White))
            .block(Block::default()
                .title(field.label.as_str())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)));
        
        let inner = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(3),
            ])
            .margin(1)
            .split(popup_area);
        
        f.render_widget(input, popup_area);
        
        let help = Paragraph::new(Line::from("Enter:确认 | Esc:取消"))
            .style(Style::default().fg(Color::Gray));
        f.render_widget(help, inner[1]);
    }

    fn render_statusbar(&self) -> Paragraph<'static> {
        let help = if self.input_mode {
            "输入模式 | Enter:确认 | Esc:取消"
        } else if self.show_confirm {
            "确认对话框 | Y:确认 | N/ESC:取消"
        } else if self.show_message {
            "Enter/ESC:关闭"
        } else {
            match self.active_tab {
                0 => "q:退出 | Tab:切换 | ↑/↓:选择 | Enter:运行 | e:编辑 | Del:删除",
                1 => "q:退出 | Tab:切换 | ↑/↓:选择数据库",
                _ => "q:退出 | Tab:切换 | ←/→:切换标签",
            }
        };

        Paragraph::new(Line::from(help))
            .style(Style::default().fg(Color::Gray))
            .block(Block::default().borders(Borders::ALL))
    }
}

/// 计算居中矩形
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
