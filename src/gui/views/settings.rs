//! 设置视图

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, List, ListItem},
    Frame,
};

/// 设置项
#[derive(Debug, Clone)]
pub struct SettingItem {
    pub category: &'static str,
    pub name: &'static str,
    pub value: String,
    pub editable: bool,
}

/// 设置视图
pub struct SettingsView {
    pub categories: Vec<SettingItem>,
    pub selected_index: usize,
}

impl SettingsView {
    pub fn new() -> Self {
        let categories = vec![
            // 外观设置
            SettingItem {
                category: "外观",
                name: "主题",
                value: String::from("深色"),
                editable: true,
            },
            SettingItem {
                category: "外观",
                name: "字体大小",
                value: String::from("14"),
                editable: true,
            },
            SettingItem {
                category: "外观",
                name: "语言",
                value: String::from("简体中文"),
                editable: true,
            },
            // 编辑器设置
            SettingItem {
                category: "编辑器",
                name: "缩进",
                value: String::from("2 空格"),
                editable: true,
            },
            SettingItem {
                category: "编辑器",
                name: "自动换行",
                value: String::from("开启"),
                editable: true,
            },
            SettingItem {
                category: "编辑器",
                name: "语法检查",
                value: String::from("开启"),
                editable: true,
            },
            // 连接设置
            SettingItem {
                category: "连接",
                name: "Azkaban 地址",
                value: String::from("http://azkaban.example.com"),
                editable: true,
            },
            SettingItem {
                category: "连接",
                name: "Hive Metastore",
                value: String::from("mysql://mysql.example.com:3306/hive"),
                editable: true,
            },
            SettingItem {
                category: "连接",
                name: "LDAP 服务器",
                value: String::from("ldap://ldap.example.com"),
                editable: true,
            },
            // AI 设置
            SettingItem {
                category: "AI",
                name: "提供商",
                value: String::from("OpenAI"),
                editable: true,
            },
            SettingItem {
                category: "AI",
                name: "模型",
                value: String::from("gpt-4o"),
                editable: true,
            },
            // 缓存设置
            SettingItem {
                category: "缓存",
                name: "元数据 TTL",
                value: String::from("3600 秒"),
                editable: true,
            },
        ];

        Self {
            categories,
            selected_index: 0,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // 标题
                Constraint::Min(0),      // 设置列表
                Constraint::Length(3),   // 提示
            ])
            .split(area);

        // 标题
        let title = Paragraph::new(Line::from(Span::styled(
            "系统设置",
            Style::default().fg(Color::Yellow).add_modifier(ratatui::style::Modifier::BOLD),
        )))
        .block(Block::default().borders(Borders::BOTTOM));
        frame.render_widget(title, chunks[0]);

        // 设置列表
        let mut current_category = "";
        let mut items = Vec::new();

        for (i, item) in self.categories.iter().enumerate() {
            // 添加分类标题
            if item.category != current_category {
                current_category = item.category;
                items.push(ListItem::new(Line::from(Span::styled(
                    format!("═══ {} ═══", current_category),
                    Style::default().fg(Color::Cyan).add_modifier(ratatui::style::Modifier::BOLD),
                ))));
            }

            // 添加设置项
            let style = if i == self.selected_index {
                Style::default().bg(Color::DarkGray).add_modifier(ratatui::style::Modifier::REVERSED)
            } else {
                Style::default()
            };

            let editable_marker = if item.editable { "✎" } else { "🔒" };

            items.push(ListItem::new(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("{} {}", editable_marker, item.name),
                    style,
                ),
                Span::raw(" "),
                Span::styled(
                    format!("= {}", item.value),
                    style.fg(Color::Gray),
                ),
            ])));
        }

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title("设置项"));
        frame.render_widget(list, chunks[1]);

        // 提示
        let hint = Paragraph::new(Line::from(Span::styled(
            "按 ↑/↓ 选择 | 按 Enter 编辑 | 按 Esc 返回",
            Style::default().fg(Color::Gray),
        )))
        .block(Block::default().borders(Borders::ALL));
        frame.render_widget(hint, chunks[2]);
    }

    pub fn select_next(&mut self) {
        if self.selected_index < self.categories.len().saturating_sub(1) {
            self.selected_index += 1;
        }
    }

    pub fn select_previous(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn selected_item(&self) -> Option<&SettingItem> {
        self.categories.get(self.selected_index)
    }
}
