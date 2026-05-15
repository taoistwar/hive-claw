//! 数据质量仪表板视图

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, List, ListItem, Gauge},
    Frame,
};

/// 质量检查结果
#[derive(Debug, Clone)]
pub struct QualityCheckResult {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub blocking: usize,
    pub critical: usize,
    pub major: usize,
    pub minor: usize,
}

/// 数据质量仪表板视图
pub struct QualityDashboardView {
    pub checks: Vec<QualityCheckResult>,
    pub selected_index: usize,
}

impl QualityDashboardView {
    pub fn new() -> Self {
        Self {
            checks: Vec::new(),
            selected_index: 0,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(10),  // 总体统计
                Constraint::Min(0),       // 检查列表
            ])
            .split(area);

        // 总体统计
        self.render_summary(frame, chunks[0]);

        // 检查列表
        self.render_checks_list(frame, chunks[1]);
    }

    fn render_summary(&self, frame: &mut Frame, area: Rect) {
        let total_checks: usize = self.checks.iter().map(|c| c.total).sum();
        let total_passed: usize = self.checks.iter().map(|c| c.passed).sum();
        let total_failed: usize = self.checks.iter().map(|c| c.failed).sum();

        let pass_rate = if total_checks > 0 {
            (total_passed as f64 / total_checks as f64) * 100.0
        } else {
            0.0
        };

        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(25),
                Constraint::Percentage(25),
                Constraint::Percentage(25),
                Constraint::Percentage(25),
            ])
            .split(area);

        // 通过率仪表板
        let gauge = Gauge::default()
            .percent(pass_rate as u16)
            .label(format!("{:.1}%", pass_rate))
            .gauge_style(Style::default().fg(Color::Green))
            .block(Block::default().title("通过率").borders(Borders::ALL));
        frame.render_widget(gauge, chunks[0]);

        // 统计信息
        let stats = vec![
            Line::from(Span::styled(
                format!("总检查：{}", total_checks),
                Style::default().fg(Color::White),
            )),
            Line::from(Span::styled(
                format!("通过：{}", total_passed),
                Style::default().fg(Color::Green),
            )),
            Line::from(Span::styled(
                format!("失败：{}", total_failed),
                Style::default().fg(Color::Red),
            )),
        ];

        let stats_para = Paragraph::new(stats)
            .block(Block::default().title("统计").borders(Borders::ALL));
        frame.render_widget(stats_para, chunks[1]);

        // 问题分级
        let blocking = self.checks.iter().map(|c| c.blocking).sum::<usize>();
        let critical = self.checks.iter().map(|c| c.critical).sum::<usize>();
        let major = self.checks.iter().map(|c| c.major).sum::<usize>();
        let minor = self.checks.iter().map(|c| c.minor).sum::<usize>();

        let issues = vec![
            Line::from(Span::styled(
                format!("阻塞：{}", blocking),
                Style::default().fg(Color::Red),
            )),
            Line::from(Span::styled(
                format!("严重：{}", critical),
                Style::default().fg(Color::Yellow),
            )),
            Line::from(Span::styled(
                format!("一般：{}", major),
                Style::default().fg(Color::Cyan),
            )),
            Line::from(Span::styled(
                format!("提示：{}", minor),
                Style::default().fg(Color::Gray),
            )),
        ];

        let issues_para = Paragraph::new(issues)
            .block(Block::default().title("问题分级").borders(Borders::ALL));
        frame.render_widget(issues_para, chunks[2]);

        // 8 类检查类型
        let check_types = vec![
            "非空检查",
            "唯一性检查",
            "值域检查",
            "枚举值检查",
            "波动检查",
            "行数检查",
            "格式检查",
            "一致性检查",
        ];

        let types_text = check_types.iter()
            .map(|t| Line::from(Span::raw(*t)))
            .collect::<Vec<_>>();

        let types_para = Paragraph::new(types_text)
            .block(Block::default().title("检查类型").borders(Borders::ALL));
        frame.render_widget(types_para, chunks[3]);
    }

    fn render_checks_list(&self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self.checks.iter().enumerate().map(|(i, check)| {
            let style = if i == self.selected_index {
                Style::default().bg(Color::DarkGray).add_modifier(ratatui::style::Modifier::REVERSED)
            } else {
                Style::default()
            };

            let status = if check.failed == 0 {
                Span::styled("✓", Style::default().fg(Color::Green))
            } else {
                Span::styled("✗", Style::default().fg(Color::Red))
            };

            let content = vec![
                Line::from(vec![
                    status,
                    Span::raw(" "),
                    Span::styled(
                        format!("任务 #{} (通过 {}/{})", i + 1, check.passed, check.total),
                        style,
                    ),
                ]),
            ];

            ListItem::new(content)
        }).collect();

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title("检查历史"));
        frame.render_widget(list, area);
    }

    pub fn select_next(&mut self) {
        if self.selected_index < self.checks.len().saturating_sub(1) {
            self.selected_index += 1;
        }
    }

    pub fn select_previous(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn add_check_result(&mut self, result: QualityCheckResult) {
        self.checks.push(result);
        self.selected_index = self.checks.len() - 1;
    }
}
