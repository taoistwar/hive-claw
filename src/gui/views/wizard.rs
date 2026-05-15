//! 创建向导视图

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, List, ListItem},
    Frame,
};

/// 向导步骤
#[derive(Debug, Clone, PartialEq)]
pub enum WizardStep {
    SelectTemplate,
    EnterDetails,
    SelectTables,
    ConfigureSchedule,
    QualityRules,
    Review,
    Complete,
}

/// 任务模板
#[derive(Debug, Clone)]
pub struct TaskTemplateOption {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
}

impl TaskTemplateOption {
    pub const ALL: [TaskTemplateOption; 7] = [
        TaskTemplateOption {
            id: "single_table_agg",
            name: "单表聚合",
            description: "对单表进行 GROUP BY 聚合计算",
        },
        TaskTemplateOption {
            id: "multi_table_join",
            name: "多表关联",
            description: "多表 JOIN 生成宽表",
        },
        TaskTemplateOption {
            id: "incremental_sync",
            name: "增量同步",
            description: "基于时间戳/水位的增量数据同步",
        },
        TaskTemplateOption {
            id: "full_sync",
            name: "全量同步",
            description: "全量数据覆盖同步",
        },
        TaskTemplateOption {
            id: "deduplication",
            name: "去重清洗",
            description: "数据去重和清洗",
        },
        TaskTemplateOption {
            id: "scd",
            name: "SCD",
            description: "缓慢变化维（Type 2）处理",
        },
        TaskTemplateOption {
            id: "metric_calc",
            name: "指标计算",
            description: "业务指标计算（DAU、留存、转化率等）",
        },
    ];
}

/// 创建向导视图
pub struct WizardView {
    pub current_step: WizardStep,
    pub selected_template: usize,
    pub task_name: String,
    pub task_description: String,
}

impl WizardView {
    pub fn new() -> Self {
        Self {
            current_step: WizardStep::SelectTemplate,
            selected_template: 0,
            task_name: String::new(),
            task_description: String::new(),
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // 进度条
                Constraint::Min(0),      // 内容区
                Constraint::Length(3),   // 操作提示
            ])
            .split(area);

        // 渲染进度条
        self.render_progress(frame, chunks[0]);

        // 渲染当前步骤内容
        match self.current_step {
            WizardStep::SelectTemplate => self.render_select_template(frame, chunks[1]),
            WizardStep::EnterDetails => self.render_enter_details(frame, chunks[1]),
            WizardStep::SelectTables => self.render_select_tables(frame, chunks[1]),
            WizardStep::ConfigureSchedule => self.render_configure_schedule(frame, chunks[1]),
            WizardStep::QualityRules => self.render_quality_rules(frame, chunks[1]),
            WizardStep::Review => self.render_review(frame, chunks[1]),
            WizardStep::Complete => self.render_complete(frame, chunks[1]),
        }

        // 渲染操作提示
        self.render_hints(frame, chunks[2]);
    }

    fn render_progress(&self, frame: &mut Frame, area: Rect) {
        let step_index = match self.current_step {
            WizardStep::SelectTemplate => 0,
            WizardStep::EnterDetails => 1,
            WizardStep::SelectTables => 2,
            WizardStep::ConfigureSchedule => 3,
            WizardStep::QualityRules => 4,
            WizardStep::Review => 5,
            WizardStep::Complete => 6,
        };

        let steps = vec!["模板", "详情", "表", "调度", "质量", "预览", "完成"];
        let progress_text = steps.iter().enumerate().map(|(i, &step)| {
            if i <= step_index {
                Span::styled(
                    format!("● {} ", step),
                    Style::default().fg(Color::Green),
                )
            } else {
                Span::styled(
                    format!("○ {} ", step),
                    Style::default().fg(Color::Gray),
                )
            }
        }).collect::<Vec<_>>();

        let progress = Paragraph::new(Line::from(progress_text))
            .block(Block::default().borders(Borders::ALL).title("创建向导"));
        frame.render_widget(progress, area);
    }

    fn render_select_template(&self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = TaskTemplateOption::ALL.iter().enumerate().map(|(i, tpl)| {
            let style = if i == self.selected_template {
                Style::default().bg(Color::DarkGray).add_modifier(ratatui::style::Modifier::REVERSED)
            } else {
                Style::default()
            };

            let content = vec![
                Line::from(Span::styled(tpl.name, style.add_modifier(ratatui::style::Modifier::BOLD))),
                Line::from(Span::styled(tpl.description, style.fg(Color::Gray))),
            ];

            ListItem::new(content)
        }).collect();

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title("选择任务模板"));
        frame.render_widget(list, area);
    }

    fn render_enter_details(&self, frame: &mut Frame, area: Rect) {
        let text = vec![
            Line::from(""),
            Line::from(Span::raw("请输入任务信息：")),
            Line::from(""),
            Line::from(format!("任务名称：{}", self.task_name)),
            Line::from(format!("任务描述：{}", self.task_description)),
            Line::from(""),
            Line::from(Span::styled(
                "提示：实际使用时在此处显示输入框",
                Style::default().fg(Color::Gray),
            )),
        ];

        let paragraph = Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title("输入任务详情"));
        frame.render_widget(paragraph, area);
    }

    fn render_select_tables(&self, frame: &mut Frame, area: Rect) {
        let text = vec![
            Line::from(""),
            Line::from(Span::raw("选择源表和目标表：")),
            Line::from(""),
            Line::from("源表：[待选择]"),
            Line::from("目标表：[待选择]"),
            Line::from(""),
            Line::from(Span::styled(
                "提示：实际使用时在此处显示表选择器",
                Style::default().fg(Color::Gray),
            )),
        ];

        let paragraph = Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title("选择数据表"));
        frame.render_widget(paragraph, area);
    }

    fn render_configure_schedule(&self, frame: &mut Frame, area: Rect) {
        let text = vec![
            Line::from(""),
            Line::from(Span::raw("配置调度策略：")),
            Line::from(""),
            Line::from("调度类型：每日调度"),
            Line::from("执行时间：02:00"),
            Line::from("依赖任务：无"),
            Line::from(""),
            Line::from(Span::styled(
                "提示：实际使用时在此处显示调度配置表单",
                Style::default().fg(Color::Gray),
            )),
        ];

        let paragraph = Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title("配置调度"));
        frame.render_widget(paragraph, area);
    }

    fn render_quality_rules(&self, frame: &mut Frame, area: Rect) {
        let text = vec![
            Line::from(""),
            Line::from(Span::raw("配置数据质量规则：")),
            Line::from(""),
            Line::from("☐ 非空检查"),
            Line::from("☐ 唯一性检查"),
            Line::from("☐ 值域检查"),
            Line::from("☐ 波动检查"),
            Line::from(""),
            Line::from(Span::styled(
                "提示：实际使用时在此处显示质量规则配置",
                Style::default().fg(Color::Gray),
            )),
        ];

        let paragraph = Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title("质量规则"));
        frame.render_widget(paragraph, area);
    }

    fn render_review(&self, frame: &mut Frame, area: Rect) {
        let text = vec![
            Line::from(""),
            Line::from(Span::raw("请确认任务配置：")),
            Line::from(""),
            Line::from(format!("模板：{}", TaskTemplateOption::ALL[self.selected_template].name)),
            Line::from(format!("名称：{}", self.task_name)),
            Line::from(format!("描述：{}", self.task_description)),
            Line::from(""),
            Line::from(Span::styled(
                "提示：实际使用时显示完整配置预览",
                Style::default().fg(Color::Gray),
            )),
        ];

        let paragraph = Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title("配置预览"));
        frame.render_widget(paragraph, area);
    }

    fn render_complete(&self, frame: &mut Frame, area: Rect) {
        let text = vec![
            Line::from(""),
            Line::from(Span::styled(
                "✓ 任务创建成功!",
                Style::default().fg(Color::Green).add_modifier(ratatui::style::Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("任务已提交到 Git 仓库"),
            Line::from("Pull Request 已创建"),
            Line::from(""),
            Line::from(Span::styled(
                "按 Enter 返回任务列表",
                Style::default().fg(Color::Yellow),
            )),
        ];

        let paragraph = Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title("完成"));
        frame.render_widget(paragraph, area);
    }

    fn render_hints(&self, frame: &mut Frame, area: Rect) {
        let hint_text = match self.current_step {
            WizardStep::SelectTemplate => "按 ↑/↓ 选择模板 | 按 Enter 继续",
            WizardStep::EnterDetails => "按 Enter 继续 | 按 Esc 返回",
            WizardStep::SelectTables => "按 Enter 继续 | 按 Esc 返回",
            WizardStep::ConfigureSchedule => "按 Enter 继续 | 按 Esc 返回",
            WizardStep::QualityRules => "按 Enter 继续 | 按 Esc 返回",
            WizardStep::Review => "按 Enter 确认 | 按 Esc 返回",
            WizardStep::Complete => "按 Enter 返回任务列表",
        };

        let hint = Paragraph::new(Line::from(Span::styled(
            hint_text,
            Style::default().fg(Color::Gray),
        )))
        .block(Block::default().borders(Borders::ALL));
        frame.render_widget(hint, area);
    }

    pub fn next_step(&mut self) {
        self.current_step = match self.current_step {
            WizardStep::SelectTemplate => WizardStep::EnterDetails,
            WizardStep::EnterDetails => WizardStep::SelectTables,
            WizardStep::SelectTables => WizardStep::ConfigureSchedule,
            WizardStep::ConfigureSchedule => WizardStep::QualityRules,
            WizardStep::QualityRules => WizardStep::Review,
            WizardStep::Review => WizardStep::Complete,
            WizardStep::Complete => WizardStep::SelectTemplate,
        };
    }

    pub fn previous_step(&mut self) {
        self.current_step = match self.current_step {
            WizardStep::SelectTemplate => WizardStep::SelectTemplate,
            WizardStep::EnterDetails => WizardStep::SelectTemplate,
            WizardStep::SelectTables => WizardStep::EnterDetails,
            WizardStep::ConfigureSchedule => WizardStep::SelectTables,
            WizardStep::QualityRules => WizardStep::ConfigureSchedule,
            WizardStep::Review => WizardStep::QualityRules,
            WizardStep::Complete => WizardStep::Review,
        };
    }

    pub fn select_template_next(&mut self) {
        if self.selected_template < TaskTemplateOption::ALL.len() - 1 {
            self.selected_template += 1;
        }
    }

    pub fn select_template_previous(&mut self) {
        if self.selected_template > 0 {
            self.selected_template -= 1;
        }
    }
}
