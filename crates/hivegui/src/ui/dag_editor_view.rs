use crate::datasource::{
    Store,
    entity_store::{Function, Workflow, WorkflowEdge, WorkflowNode},
};
use crate::ui::management_style::{ActionRole, ActionSize, ManagementStyle, action_button};
use gpui::{
    Context, CursorStyle, Entity, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels,
    Point, ScrollHandle, Window, div, hsla, prelude::*, px,
};
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement;
use std::collections::HashMap;

/// DAG 节点类型
#[derive(Debug, Clone, PartialEq)]
pub enum DagNodeType {
    StartNode,
    EndNode,
    FunctionNode,
    GenerateAnswerNode,
}

impl DagNodeType {
    pub fn from_str(s: &str) -> Self {
        match s {
            "start_node" => DagNodeType::StartNode,
            "end_node" => DagNodeType::EndNode,
            "generate_answer_node" => DagNodeType::GenerateAnswerNode,
            _ => DagNodeType::FunctionNode,
        }
    }

    pub fn to_str(&self) -> &'static str {
        match self {
            DagNodeType::StartNode => "start_node",
            DagNodeType::EndNode => "end_node",
            DagNodeType::FunctionNode => "function_node",
            DagNodeType::GenerateAnswerNode => "generate_answer_node",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            DagNodeType::StartNode => "开始节点",
            DagNodeType::EndNode => "结束节点",
            DagNodeType::FunctionNode => "函数节点",
            DagNodeType::GenerateAnswerNode => "回答节点",
        }
    }
}

/// DAG 编辑器中的节点
#[derive(Debug, Clone)]
pub struct DagNode {
    pub node_key: String,
    pub node_type: DagNodeType,
    pub function_id: Option<i64>,
    pub function_name: Option<String>,
    pub position: Point<Pixels>,
    pub node_config: Option<serde_json::Value>,
}

/// DAG 编辑器中的连线
#[derive(Debug, Clone)]
pub struct DagEdge {
    pub src_node_key: String,
    pub dst_node_key: String,
    pub mapping: serde_json::Value,
}

pub struct DagEditorView {
    store: Entity<Store>,
    workflow_id: i64,
    workflow: Option<Workflow>,
    nodes: Vec<DagNode>,
    edges: Vec<DagEdge>,
    functions: Vec<Function>,
    loading: bool,
    error_message: Option<String>,

    // 拖拽状态
    dragging_node: Option<(String, Point<Pixels>)>,
    drag_offset: Point<Pixels>,

    // 连线创建状态
    creating_edge: Option<(String, String, Point<Pixels>)>,
    mouse_position: Point<Pixels>,

    // 选中状态
    selected_node: Option<String>,
    selected_edge: Option<(String, String)>,

    // 右键菜单
    context_menu: Option<(Point<Pixels>, String)>,

    // 添加函数节点弹窗
    show_add_function: bool,
    add_function_input: Option<Entity<InputState>>,
    add_function_search: String,

    // 节点配置面板
    show_node_config: bool,
    config_node_key: Option<String>,

    // 滚动
    scroll_handle: ScrollHandle,
}

impl DagEditorView {
    pub fn new(store: Entity<Store>, workflow_id: i64, cx: &mut Context<Self>) -> Self {
        let mut v = Self {
            store,
            workflow_id,
            workflow: None,
            nodes: Vec::new(),
            edges: Vec::new(),
            functions: Vec::new(),
            loading: true,
            error_message: None,
            dragging_node: None,
            drag_offset: Point::default(),
            creating_edge: None,
            mouse_position: Point::default(),
            selected_node: None,
            selected_edge: None,
            context_menu: None,
            show_add_function: false,
            add_function_input: None,
            add_function_search: String::new(),
            show_node_config: false,
            config_node_key: None,
            scroll_handle: ScrollHandle::default(),
        };
        v.load(cx);
        v
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let store = self.store.read(cx).clone();
        let workflow_id = self.workflow_id;

        cx.spawn(async move |this, cx| {
            let workflow = Workflow::get(store.pool(), workflow_id).await?;
            let db_nodes = WorkflowNode::list_by_workflow(store.pool(), workflow_id).await?;
            let db_edges = WorkflowEdge::list_by_workflow(store.pool(), workflow_id).await?;
            let functions = Function::list(store.pool(), None, 500, 0).await?;

            this.update(cx, |v, cx| {
                v.workflow = workflow;
                v.functions = functions;

                v.nodes = db_nodes
                    .into_iter()
                    .map(|n| {
                        let function_name = n.function_id.and_then(|fid| {
                            v.functions
                                .iter()
                                .find(|f| f.id == fid)
                                .map(|f| f.name.clone())
                        });
                        DagNode {
                            node_key: n.node_key,
                            node_type: DagNodeType::from_str(&n.node_type),
                            function_id: n.function_id,
                            function_name,
                            position: Point::new(px(n.position_x as f32), px(n.position_y as f32)),
                            node_config: n.node_config.and_then(|s| serde_json::from_str(&s).ok()),
                        }
                    })
                    .collect();

                v.edges = db_edges
                    .into_iter()
                    .map(|e| DagEdge {
                        src_node_key: e.src_node_key,
                        dst_node_key: e.dst_node_key,
                        mapping: serde_json::from_str(&e.mapping).unwrap_or_default(),
                    })
                    .collect();

                v.loading = false;
                cx.notify();
            })
        })
        .detach();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if let Some(cycle) = self.detect_cycle() {
            self.error_message = Some(format!("检测到环: {}", cycle.join(" → ")));
            cx.notify();
            return;
        }

        let store = self.store.read(cx).clone();
        let workflow_id = self.workflow_id;
        let nodes = self.nodes.clone();
        let edges = self.edges.clone();

        cx.spawn(async move |this, cx| {
            WorkflowNode::delete_by_workflow(store.pool(), workflow_id).await?;
            WorkflowEdge::delete_by_workflow(store.pool(), workflow_id).await?;

            for node in &nodes {
                let node_config = node.node_config.as_ref().map(|v| v.to_string());
                WorkflowNode::upsert(
                    store.pool(),
                    workflow_id,
                    node.node_key.clone(),
                    node.node_type.to_str().to_string(),
                    node.function_id,
                    f64::from(node.position.x),
                    f64::from(node.position.y),
                    node_config,
                )
                .await?;
            }

            for edge in &edges {
                WorkflowEdge::upsert(
                    store.pool(),
                    workflow_id,
                    edge.src_node_key.clone(),
                    edge.dst_node_key.clone(),
                    edge.mapping.to_string(),
                )
                .await?;
            }

            this.update(cx, |v, cx| {
                v.error_message = None;
                cx.notify();
            })
            .ok();

            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.load(cx);
    }

    fn show_add_function_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_add_function = true;
        self.add_function_search.clear();
        self.add_function_input =
            Some(cx.new(|cx| InputState::new(window, cx).placeholder("搜索函数...")));
        cx.notify();
    }

    fn hide_add_function_modal(&mut self, cx: &mut Context<Self>) {
        self.show_add_function = false;
        self.add_function_input = None;
        cx.notify();
    }

    fn add_function_node(&mut self, function_id: i64, cx: &mut Context<Self>) {
        let function = self.functions.iter().find(|f| f.id == function_id);
        if let Some(func) = function {
            let seq = self
                .nodes
                .iter()
                .filter(|n| n.node_type == DagNodeType::FunctionNode)
                .count()
                + 1;
            let node_key = format!("function_{}", seq);

            let start_pos = self
                .nodes
                .iter()
                .find(|n| n.node_type == DagNodeType::StartNode)
                .map(|n| n.position)
                .unwrap_or(Point::new(px(100.0), px(100.0)));

            let offset_x = (self
                .nodes
                .iter()
                .filter(|n| {
                    n.node_type == DagNodeType::FunctionNode
                        || n.node_type == DagNodeType::GenerateAnswerNode
                })
                .count()
                % 3) as f32
                * 200.0;

            self.nodes.push(DagNode {
                node_key,
                node_type: DagNodeType::FunctionNode,
                function_id: Some(function_id),
                function_name: Some(func.name.clone()),
                position: Point::new(start_pos.x + px(offset_x), start_pos.y + px(150.0)),
                node_config: None,
            });
        }
        self.hide_add_function_modal(cx);
    }

    fn add_answer_node(&mut self, _cx: &mut Context<Self>) {
        let seq = self
            .nodes
            .iter()
            .filter(|n| n.node_type == DagNodeType::GenerateAnswerNode)
            .count()
            + 1;
        let node_key = format!("answer_{}", seq);

        let start_pos = self
            .nodes
            .iter()
            .find(|n| n.node_type == DagNodeType::StartNode)
            .map(|n| n.position)
            .unwrap_or(Point::new(px(100.0), px(100.0)));

        self.nodes.push(DagNode {
            node_key,
            node_type: DagNodeType::GenerateAnswerNode,
            function_id: None,
            function_name: None,
            position: Point::new(start_pos.x + px(200.0), start_pos.y + px(200.0)),
            node_config: Some(serde_json::json!({
                "system_prompt": "你是一个智能助手，请根据以下内容回答用户问题：\n{query}",
                "history_window": 0,
                "input_mapping": {}
            })),
        });
    }

    fn delete_node(&mut self, node_key: &str, cx: &mut Context<Self>) {
        if node_key == "start" || node_key == "end" {
            self.error_message = Some(format!(
                "{}不能删除",
                if node_key == "start" {
                    "开始节点"
                } else {
                    "结束节点"
                }
            ));
            cx.notify();
            return;
        }

        self.nodes.retain(|n| n.node_key != node_key);
        self.edges
            .retain(|e| e.src_node_key != node_key && e.dst_node_key != node_key);
        self.selected_node = None;
        cx.notify();
    }

    fn show_node_config_panel(&mut self, node_key: &str, cx: &mut Context<Self>) {
        self.selected_node = Some(node_key.to_string());
        self.config_node_key = Some(node_key.to_string());
        self.show_node_config = true;
        cx.notify();
    }

    fn hide_node_config_panel(&mut self, cx: &mut Context<Self>) {
        self.show_node_config = false;
        self.config_node_key = None;
        cx.notify();
    }

    fn detect_cycle(&self) -> Option<Vec<String>> {
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();
        for edge in &self.edges {
            graph
                .entry(edge.src_node_key.clone())
                .or_default()
                .push(edge.dst_node_key.clone());
        }

        let mut visited = HashMap::new();
        let mut rec_stack = HashMap::new();
        let mut path = Vec::new();

        for node in &self.nodes {
            if !visited.get(&node.node_key).unwrap_or(&false) {
                if let Some(cycle) = self.dfs_cycle(
                    &node.node_key,
                    &graph,
                    &mut visited,
                    &mut rec_stack,
                    &mut path,
                ) {
                    return Some(cycle);
                }
            }
        }
        None
    }

    fn dfs_cycle(
        &self,
        node: &str,
        graph: &HashMap<String, Vec<String>>,
        visited: &mut HashMap<String, bool>,
        rec_stack: &mut HashMap<String, bool>,
        path: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        visited.insert(node.to_string(), true);
        rec_stack.insert(node.to_string(), true);
        path.push(node.to_string());

        if let Some(neighbors) = graph.get(node) {
            for neighbor in neighbors {
                if !visited.get(neighbor).unwrap_or(&false) {
                    if let Some(cycle) = self.dfs_cycle(neighbor, graph, visited, rec_stack, path) {
                        return Some(cycle);
                    }
                } else if *rec_stack.get(neighbor).unwrap_or(&false) {
                    path.push(neighbor.clone());
                    let cycle_start = path.iter().position(|n| n == neighbor).unwrap();
                    return Some(path[cycle_start..].to_vec());
                }
            }
        }

        path.pop();
        rec_stack.insert(node.to_string(), false);
        None
    }

    fn on_node_mouse_down(
        &mut self,
        node_key: &str,
        position: Point<Pixels>,
        event: &MouseDownEvent,
    ) {
        self.dragging_node = Some((node_key.to_string(), position));
        self.drag_offset = Point::new(event.position.x - position.x, event.position.y - position.y);
        self.selected_node = Some(node_key.to_string());
    }

    fn on_node_mouse_move(&mut self, position: Point<Pixels>) {
        if let Some((ref node_key, _)) = self.dragging_node {
            if let Some(node) = self.nodes.iter_mut().find(|n| &n.node_key == node_key) {
                node.position = Point::new(
                    position.x - self.drag_offset.x,
                    position.y - self.drag_offset.y,
                );
            }
        }
    }

    fn on_node_mouse_up(&mut self) {
        self.dragging_node = None;
    }

    fn on_handle_mouse_down(&mut self, node_key: &str, handle_type: &str, position: Point<Pixels>) {
        self.creating_edge = Some((node_key.to_string(), handle_type.to_string(), position));
    }

    fn on_handle_mouse_up(
        &mut self,
        target_node_key: &str,
        handle_type: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some((ref src_key, ref src_handle, _)) = self.creating_edge {
            // 确保从输出连到输入
            if src_handle == "output" && handle_type == "input" && src_key != target_node_key {
                // 检查是否已存在
                let exists = self
                    .edges
                    .iter()
                    .any(|e| e.src_node_key == *src_key && e.dst_node_key == target_node_key);
                if !exists {
                    self.edges.push(DagEdge {
                        src_node_key: src_key.clone(),
                        dst_node_key: target_node_key.to_string(),
                        mapping: serde_json::json!({}),
                    });
                    cx.notify();
                }
            }
        }
        self.creating_edge = None;
    }

    fn delete_edge(&mut self, src: &str, dst: &str, cx: &mut Context<Self>) {
        self.edges
            .retain(|e| !(e.src_node_key == src && e.dst_node_key == dst));
        self.selected_edge = None;
        cx.notify();
    }

    fn on_canvas_mouse_down(&mut self, _position: Point<Pixels>) {
        // 点击空白处取消选中
        self.selected_node = None;
        self.selected_edge = None;
        self.context_menu = None;
    }

    fn update_node_config(
        &mut self,
        node_key: &str,
        config: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        if let Some(node) = self.nodes.iter_mut().find(|n| n.node_key == node_key) {
            node.node_config = Some(config);
            cx.notify();
        }
    }
}

impl Render for DagEditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let style = ManagementStyle::current(cx);

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            // 工具栏
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p(px(16.0))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                action_button(
                                    "add-fn",
                                    "添加函数节点",
                                    ActionRole::Main,
                                    ActionSize::Page,
                                    style,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let t = cx.weak_entity();
                                        move |_, window, cx| {
                                            t.update(cx, |v, cx| {
                                                v.show_add_function_modal(window, cx)
                                            })
                                            .ok();
                                        }
                                    },
                                ),
                            )
                            .child(
                                action_button(
                                    "add-answer",
                                    "添加回答节点",
                                    ActionRole::Neutral,
                                    ActionSize::Page,
                                    style,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let t = cx.weak_entity();
                                        move |_, _, cx| {
                                            t.update(cx, |v, cx| v.add_answer_node(cx)).ok();
                                        }
                                    },
                                ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                action_button(
                                    "save",
                                    "保存",
                                    ActionRole::Main,
                                    ActionSize::Page,
                                    style,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let t = cx.weak_entity();
                                        move |_, _, cx| {
                                            t.update(cx, |v, cx| v.save(cx)).ok();
                                        }
                                    },
                                ),
                            )
                            .child(
                                action_button(
                                    "reset",
                                    "重置",
                                    ActionRole::Neutral,
                                    ActionSize::Page,
                                    style,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let t = cx.weak_entity();
                                        move |_, _, cx| {
                                            t.update(cx, |v, cx| v.reset(cx)).ok();
                                        }
                                    },
                                ),
                            ),
                    ),
            )
            // 错误提示
            .when_some(self.error_message.as_ref(), |this, err| {
                this.child(
                    div()
                        .p(px(12.0))
                        .bg(theme.warning.opacity(0.1))
                        .border_b_1()
                        .border_color(theme.warning)
                        .text_size(px(13.0))
                        .text_color(theme.warning)
                        .child(err.clone()),
                )
            })
            // 画布区域
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .overflow_hidden()
                    .bg(hsla(0.0, 0.0, 0.98, 1.0))
                    .on_mouse_down(MouseButton::Left, {
                        let t = cx.weak_entity();
                        move |event, _, cx| {
                            t.update(cx, |v, cx| {
                                v.on_canvas_mouse_down(event.position);
                                cx.notify();
                            })
                            .ok();
                        }
                    })
                    // 节点
                    .children(self.nodes.iter().map(|node| {
                        let node_key = node.node_key.clone();
                        let is_selected = self.selected_node.as_ref() == Some(&node_key);
                        let node_type = node.node_type.clone();
                        let function_name = node.function_name.clone();
                        let position = node.position;

                        let bg_color = match node_type {
                            DagNodeType::StartNode => hsla(0.33, 0.7, 0.9, 1.0),
                            DagNodeType::EndNode => hsla(0.0, 0.7, 0.9, 1.0),
                            DagNodeType::FunctionNode => hsla(0.58, 0.7, 0.9, 1.0),
                            DagNodeType::GenerateAnswerNode => hsla(0.75, 0.7, 0.9, 1.0),
                        };

                        div()
                            .absolute()
                            .top(position.y)
                            .left(position.x)
                            .w(px(180.0))
                            .bg(bg_color)
                            .border_1()
                            .border_color(if is_selected {
                                hsla(0.6, 0.8, 0.5, 1.0)
                            } else {
                                hsla(0.0, 0.0, 0.7, 1.0)
                            })
                            .rounded(px(8.0))
                            .shadow_md()
                            .p(px(12.0))
                            .cursor(CursorStyle::PointingHand)
                            .on_mouse_down(MouseButton::Left, {
                                let t = cx.weak_entity();
                                let key = node_key.clone();
                                move |event, _, cx| {
                                    t.update(cx, |v, cx| {
                                        v.on_node_mouse_down(&key, position, event);
                                        cx.notify();
                                    })
                                    .ok();
                                }
                            })
                            .on_mouse_up(MouseButton::Left, {
                                let t = cx.weak_entity();
                                move |_, _, cx| {
                                    t.update(cx, |v, cx| {
                                        v.on_node_mouse_up();
                                        cx.notify();
                                    })
                                    .ok();
                                }
                            })
                            .on_mouse_move({
                                let t = cx.weak_entity();
                                move |event: &MouseMoveEvent, _, cx| {
                                    t.update(cx, |v, cx| {
                                        v.on_node_mouse_move(event.position);
                                        cx.notify();
                                    })
                                    .ok();
                                }
                            })
                            .on_mouse_down(MouseButton::Right, {
                                let t = cx.weak_entity();
                                let key = node_key.clone();
                                move |event, _, cx| {
                                    t.update(cx, |v, cx| {
                                        v.context_menu = Some((event.position, key.clone()));
                                        cx.notify();
                                    })
                                    .ok();
                                }
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .w(px(24.0))
                                                    .h(px(24.0))
                                                    .rounded_full()
                                                    .bg(hsla(0.0, 0.0, 1.0, 1.0))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .text_size(px(14.0))
                                                    .child(match node_type {
                                                        DagNodeType::StartNode => "▶",
                                                        DagNodeType::EndNode => "■",
                                                        DagNodeType::FunctionNode => "⚙",
                                                        DagNodeType::GenerateAnswerNode => "",
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .text_size(px(13.0))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .child(function_name.unwrap_or_else(|| {
                                                        node_type.display_name().to_string()
                                                    })),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(hsla(0.0, 0.0, 0.5, 1.0))
                                            .child(node_key.clone()),
                                    ),
                            )
                            // 顶部连接点（输入）
                            .child(
                                div()
                                    .absolute()
                                    .top(px(-6.0))
                                    .left(px(84.0))
                                    .w(px(12.0))
                                    .h(px(12.0))
                                    .rounded_full()
                                    .bg(hsla(0.6, 0.7, 0.6, 1.0))
                                    .border_1()
                                    .border_color(hsla(0.0, 0.0, 1.0, 1.0))
                                    .cursor(CursorStyle::PointingHand)
                                    .on_mouse_down(MouseButton::Left, {
                                        let t = cx.weak_entity();
                                        let key = node_key.clone();
                                        let pos = Point::new(position.x + px(90.0), position.y);
                                        move |_, _, cx| {
                                            t.update(cx, |v, cx| {
                                                v.on_handle_mouse_down(&key, "input", pos);
                                                cx.notify();
                                            })
                                            .ok();
                                        }
                                    })
                                    .on_mouse_up(MouseButton::Left, {
                                        let t = cx.weak_entity();
                                        let key = node_key.clone();
                                        move |_, _, cx| {
                                            t.update(cx, |v, cx| {
                                                v.on_handle_mouse_up(&key, "input", cx);
                                            })
                                            .ok();
                                        }
                                    }),
                            )
                            // 底部连接点（输出）
                            .child(
                                div()
                                    .absolute()
                                    .bottom(px(-6.0))
                                    .left(px(84.0))
                                    .w(px(12.0))
                                    .h(px(12.0))
                                    .rounded_full()
                                    .bg(hsla(0.6, 0.7, 0.6, 1.0))
                                    .border_1()
                                    .border_color(hsla(0.0, 0.0, 1.0, 1.0))
                                    .cursor(CursorStyle::PointingHand)
                                    .on_mouse_down(MouseButton::Left, {
                                        let t = cx.weak_entity();
                                        let key = node_key.clone();
                                        let pos = Point::new(
                                            position.x + px(90.0),
                                            position.y + px(100.0),
                                        );
                                        move |_, _, cx| {
                                            t.update(cx, |v, cx| {
                                                v.on_handle_mouse_down(&key, "output", pos);
                                                cx.notify();
                                            })
                                            .ok();
                                        }
                                    })
                                    .on_mouse_up(MouseButton::Left, {
                                        let t = cx.weak_entity();
                                        let key = node_key.clone();
                                        move |_, _, cx| {
                                            t.update(cx, |v, cx| {
                                                v.on_handle_mouse_up(&key, "output", cx);
                                            })
                                            .ok();
                                        }
                                    }),
                            )
                    }))
                    // 连线渲染
                    .children(self.edges.iter().map(|edge| {
                        let src = self.nodes.iter().find(|n| n.node_key == edge.src_node_key);
                        let dst = self.nodes.iter().find(|n| n.node_key == edge.dst_node_key);

                        if let (Some(src_node), Some(dst_node)) = (src, dst) {
                            let src_pos = src_node.position;
                            let dst_pos = dst_node.position;
                            let is_selected = self
                                .selected_edge
                                .as_ref()
                                .map(|(s, d)| s == &edge.src_node_key && d == &edge.dst_node_key)
                                .unwrap_or(false);

                            // 计算连线起点和终点（节点底部中心到顶部中心）
                            let start_x = src_pos.x + px(90.0);
                            let start_y = src_pos.y + px(100.0);
                            let end_x = dst_pos.x + px(90.0);
                            let end_y = dst_pos.y;

                            div()
                                .absolute()
                                .top(start_y.min(end_y))
                                .left(start_x.min(end_x))
                                .w((start_x.max(end_x) - start_x.min(end_x)).max(px(20.0)))
                                .h((start_y.max(end_y) - start_y.min(end_y)).max(px(20.0)))
                                .bg(if is_selected {
                                    hsla(0.6, 0.8, 0.5, 0.2)
                                } else {
                                    hsla(0.0, 0.0, 0.4, 0.1)
                                })
                                .border_1()
                                .border_color(if is_selected {
                                    hsla(0.6, 0.8, 0.5, 1.0)
                                } else {
                                    hsla(0.0, 0.0, 0.4, 0.3)
                                })
                                .rounded(px(4.0))
                                .cursor(CursorStyle::PointingHand)
                                .on_mouse_down(MouseButton::Left, {
                                    let t = cx.weak_entity();
                                    let src_key = edge.src_node_key.clone();
                                    let dst_key = edge.dst_node_key.clone();
                                    move |_, _, cx| {
                                        t.update(cx, |v, cx| {
                                            v.selected_edge =
                                                Some((src_key.clone(), dst_key.clone()));
                                            v.selected_node = None;
                                            cx.notify();
                                        })
                                        .ok();
                                    }
                                })
                                .on_mouse_down(MouseButton::Right, {
                                    let t = cx.weak_entity();
                                    let src_key = edge.src_node_key.clone();
                                    let dst_key = edge.dst_node_key.clone();
                                    move |_event, _, cx| {
                                        t.update(cx, |v, cx| {
                                            v.delete_edge(&src_key, &dst_key, cx);
                                            v.context_menu = None;
                                        })
                                        .ok();
                                    }
                                })
                        } else {
                            div().flex_none()
                        }
                    }))
                    // 右键菜单
                    .when_some(self.context_menu.as_ref(), |this, (pos, node_key)| {
                        let key = node_key.clone();
                        this.child(
                            div()
                                .absolute()
                                .top(pos.y)
                                .left(pos.x)
                                .bg(theme.popover)
                                .border_1()
                                .border_color(theme.border)
                                .rounded(px(6.0))
                                .shadow_lg()
                                .p(px(8.0))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(4.0))
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .px(px(8.0))
                                                .py(px(4.0))
                                                .rounded(px(4.0))
                                                .cursor(CursorStyle::PointingHand)
                                                .hover(|this| this.opacity(0.8))
                                                .child("查看配置")
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    let k = key.clone();
                                                    move |_, _, cx| {
                                                        t.update(cx, |v, cx| {
                                                            v.show_node_config_panel(&k, cx);
                                                            v.context_menu = None;
                                                        })
                                                        .ok();
                                                    }
                                                }),
                                        )
                                        .when(key != "start" && key != "end", |this| {
                                            this.child(
                                                div()
                                                    .text_size(px(12.0))
                                                    .px(px(8.0))
                                                    .py(px(4.0))
                                                    .rounded(px(4.0))
                                                    .cursor(CursorStyle::PointingHand)
                                                    .hover(|this| this.opacity(0.8))
                                                    .text_color(theme.danger)
                                                    .child("删除节点")
                                                    .on_mouse_down(MouseButton::Left, {
                                                        let t = cx.weak_entity();
                                                        let k = key.clone();
                                                        move |_, _, cx| {
                                                            t.update(cx, |v, cx| {
                                                                v.delete_node(&k, cx);
                                                                v.context_menu = None;
                                                            })
                                                            .ok();
                                                        }
                                                    }),
                                            )
                                        }),
                                )
                                .on_mouse_down(MouseButton::Left, {
                                    let t = cx.weak_entity();
                                    move |_, _, cx| {
                                        t.update(cx, |v, cx| {
                                            v.context_menu = None;
                                            cx.notify();
                                        })
                                        .ok();
                                    }
                                }),
                        )
                    }),
            )
            // 状态栏
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p(px(12.0))
                    .border_t_1()
                    .border_color(theme.border)
                    .text_size(px(12.0))
                    .text_color(hsla(0.0, 0.0, 0.5, 1.0))
                    .child(format!(
                        "节点: {} | 连线: {}",
                        self.nodes.len(),
                        self.edges.len()
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .w(px(12.0))
                                    .h(px(12.0))
                                    .rounded_full()
                                    .bg(hsla(0.33, 0.7, 0.9, 1.0)),
                            )
                            .child("开始")
                            .child(
                                div()
                                    .ml(px(12.0))
                                    .w(px(12.0))
                                    .h(px(12.0))
                                    .rounded_full()
                                    .bg(hsla(0.0, 0.7, 0.9, 1.0)),
                            )
                            .child("结束")
                            .child(
                                div()
                                    .ml(px(12.0))
                                    .w(px(12.0))
                                    .h(px(12.0))
                                    .rounded_full()
                                    .bg(hsla(0.58, 0.7, 0.9, 1.0)),
                            )
                            .child("函数")
                            .child(
                                div()
                                    .ml(px(12.0))
                                    .w(px(12.0))
                                    .h(px(12.0))
                                    .rounded_full()
                                    .bg(hsla(0.75, 0.7, 0.9, 1.0)),
                            )
                            .child("回答"),
                    ),
            )
            // 添加函数节点弹窗
            .when(self.show_add_function, |this| {
                let input = self.add_function_input.clone().unwrap();
                let search = self.add_function_search.clone();
                let filtered_functions: Vec<_> = self
                    .functions
                    .iter()
                    .filter(|f| {
                        if search.is_empty() {
                            true
                        } else {
                            f.name.contains(&search) || f.identifier.contains(&search)
                        }
                    })
                    .cloned()
                    .collect();

                this.child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .bg(theme.overlay)
                        .opacity(0.3)
                        .on_mouse_down(MouseButton::Left, {
                            let t = cx.weak_entity();
                            move |_, _, cx| {
                                t.update(cx, |v, cx| v.hide_add_function_modal(cx)).ok();
                            }
                        }),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(100.0))
                        .left(px(0.0))
                        .right(px(0.0))
                        .mx_auto()
                        .w(px(500.0))
                        .bg(theme.popover)
                        .rounded(px(12.0))
                        .shadow_lg()
                        .border_1()
                        .border_color(theme.border)
                        .p(px(24.0))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(16.0))
                                .child(
                                    div()
                                        .text_size(px(18.0))
                                        .font_weight(FontWeight::BOLD)
                                        .child("添加函数节点"),
                                )
                                .child(div().child(Input::new(&input)))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.0))
                                        .max_h(px(300.0))
                                        .overflow_y_scrollbar()
                                        .children(filtered_functions.into_iter().map(|f| {
                                            let fid = f.id;
                                            div()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .p(px(12.0))
                                                .border_1()
                                                .border_color(theme.border)
                                                .rounded(px(6.0))
                                                .cursor(CursorStyle::PointingHand)
                                                .hover(|this| this.opacity(0.8))
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .child(
                                                            div()
                                                                .text_size(px(14.0))
                                                                .font_weight(FontWeight::MEDIUM)
                                                                .child(f.name.clone()),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(px(12.0))
                                                                .text_color(hsla(
                                                                    0.0, 0.0, 0.5, 1.0,
                                                                ))
                                                                .child(f.identifier.clone()),
                                                        ),
                                                )
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    move |_, _, cx| {
                                                        t.update(cx, |v, cx| {
                                                            v.add_function_node(fid, cx)
                                                        })
                                                        .ok();
                                                    }
                                                })
                                        })),
                                ),
                        ),
                )
            })
            // 节点配置面板
            .when(self.show_node_config, |this| {
                let config_node_key = self.config_node_key.clone().unwrap_or_default();
                let node = self.nodes.iter().find(|n| n.node_key == config_node_key);

                if let Some(node) = node {
                    let node_key = node.node_key.clone();
                    let node_type = node.node_type.clone();
                    let function_name = node.function_name.clone();
                    let node_config_str = node
                        .node_config
                        .as_ref()
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "{}".to_string());

                    this.child(
                        div()
                            .absolute()
                            .top(px(0.0))
                            .right(px(0.0))
                            .bottom(px(0.0))
                            .w(px(350.0))
                            .bg(theme.popover)
                            .border_l_1()
                            .border_color(theme.border)
                            .shadow_lg()
                            .p(px(20.0))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(16.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_size(px(16.0))
                                                    .font_weight(FontWeight::BOLD)
                                                    .child("节点配置"),
                                            )
                                            .child(
                                                action_button(
                                                    "close-config",
                                                    "关闭",
                                                    ActionRole::Neutral,
                                                    ActionSize::Page,
                                                    style,
                                                )
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    move |_, _, cx| {
                                                        t.update(cx, |v, cx| {
                                                            v.hide_node_config_panel(cx)
                                                        })
                                                        .ok();
                                                    }
                                                }),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(12.0))
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(4.0))
                                                    .child(
                                                        div()
                                                            .text_size(px(12.0))
                                                            .text_color(hsla(0.0, 0.0, 0.5, 1.0))
                                                            .child("节点标识"),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(14.0))
                                                            .child(node_key.clone()),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(4.0))
                                                    .child(
                                                        div()
                                                            .text_size(px(12.0))
                                                            .text_color(hsla(0.0, 0.0, 0.5, 1.0))
                                                            .child("节点类型"),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(14.0))
                                                            .child(node_type.display_name()),
                                                    ),
                                            )
                                            .when(node_type == DagNodeType::FunctionNode, |this| {
                                                this.child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .gap(px(4.0))
                                                        .child(
                                                            div()
                                                                .text_size(px(12.0))
                                                                .text_color(hsla(
                                                                    0.0, 0.0, 0.5, 1.0,
                                                                ))
                                                                .child("关联函数"),
                                                        )
                                                        .child(div().text_size(px(14.0)).child(
                                                            function_name.unwrap_or_else(|| {
                                                                "未选择".to_string()
                                                            }),
                                                        )),
                                                )
                                            })
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(4.0))
                                                    .child(
                                                        div()
                                                            .text_size(px(12.0))
                                                            .text_color(hsla(0.0, 0.0, 0.5, 1.0))
                                                            .child("节点配置 (JSON)"),
                                                    )
                                                    .child(
                                                        div()
                                                            .p(px(8.0))
                                                            .bg(theme.background)
                                                            .border_1()
                                                            .border_color(theme.border)
                                                            .rounded(px(4.0))
                                                            .text_size(px(12.0))
                                                            .font_family("monospace")
                                                            .child(node_config_str),
                                                    ),
                                            ),
                                    ),
                            ),
                    )
                } else {
                    this
                }
            })
    }
}
