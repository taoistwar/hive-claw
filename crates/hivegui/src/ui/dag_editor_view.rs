use crate::datasource::{
    Store,
    entity_store::{Function, Workflow, WorkflowEdge, WorkflowNode},
};
use crate::ui::management_style::{ActionRole, ActionSize, ManagementStyle, action_button};
use gpui::{
    Bounds, Context, CursorStyle, Entity, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent,
    Pixels, Point, ScrollWheelEvent, Window, div, hsla, point, prelude::*, px,
};
use gpui_component::ActiveTheme as _;
use gpui_component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
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
    pub fn from_node_str(s: &str) -> Self {
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
    panning_canvas: Option<(Point<Pixels>, Point<Pixels>)>,
    canvas_bounds: Bounds<Pixels>,
    canvas_offset: Point<Pixels>,
    canvas_zoom: f32,

    // 连线创建状态
    creating_edge: Option<(String, String, Point<Pixels>)>,

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
    config_input: Option<Entity<TextareaState>>,
    config_model_preset_input: Option<Entity<InputState>>,
    config_history_window_input: Option<Entity<InputState>>,
    config_system_prompt_input: Option<Entity<TextareaState>>,
    config_error: Option<String>,
}

impl DagEditorView {
    pub fn new(store: Entity<Store>, workflow_id: i64, cx: &mut Context<Self>) -> Self {
        let mut v = Self {
            store,
            workflow_id,
            workflow: None,
            nodes: Self::boundary_nodes(),
            edges: Vec::new(),
            functions: Vec::new(),
            loading: true,
            error_message: None,
            dragging_node: None,
            drag_offset: Point::default(),
            panning_canvas: None,
            canvas_bounds: Bounds::default(),
            canvas_offset: Point::default(),
            canvas_zoom: 1.0,
            creating_edge: None,
            selected_node: None,
            selected_edge: None,
            context_menu: None,
            show_add_function: false,
            add_function_input: None,
            add_function_search: String::new(),
            show_node_config: false,
            config_node_key: None,
            config_input: None,
            config_model_preset_input: None,
            config_history_window_input: None,
            config_system_prompt_input: None,
            config_error: None,
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
                            node_type: DagNodeType::from_node_str(&n.node_type),
                            function_id: n.function_id,
                            function_name,
                            position: Point::new(px(n.position_x as f32), px(n.position_y as f32)),
                            node_config: n.node_config.and_then(|s| serde_json::from_str(&s).ok()),
                        }
                    })
                    .collect();
                v.ensure_boundary_nodes();

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
        let workflow = self.workflow.clone();
        let nodes = self.nodes.clone();
        let edges = self.edges.clone();

        cx.spawn(async move |this, cx| {
            if let Some(workflow) = workflow {
                Workflow::update(
                    store.pool(),
                    workflow.id,
                    workflow.identifier,
                    workflow.name,
                    workflow.description,
                    workflow.timeout_ms,
                    workflow.category_id,
                    workflow.input_schema,
                    workflow.start_description,
                    workflow.output_schema,
                    workflow.required_capabilities,
                )
                .await?;
            }

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

    fn boundary_nodes() -> Vec<DagNode> {
        vec![
            DagNode {
                node_key: "start".to_string(),
                node_type: DagNodeType::StartNode,
                function_id: None,
                function_name: None,
                position: point(px(100.0), px(80.0)),
                node_config: None,
            },
            DagNode {
                node_key: "end".to_string(),
                node_type: DagNodeType::EndNode,
                function_id: None,
                function_name: None,
                position: point(px(500.0), px(320.0)),
                node_config: None,
            },
        ]
    }

    fn ensure_boundary_nodes(&mut self) {
        for node in Self::boundary_nodes() {
            if !self
                .nodes
                .iter()
                .any(|existing| existing.node_type == node.node_type)
            {
                self.nodes.push(node);
            }
        }
    }

    fn show_add_function_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_add_function = true;
        self.add_function_search.clear();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("搜索函数..."));
        cx.subscribe_in(&input, window, |this, input, event, _, cx| {
            if let InputEvent::Change = event {
                this.add_function_search = input.read(cx).value().to_string();
                cx.notify();
            }
        })
        .detach();
        input.update(cx, |input, cx| input.focus(window, cx));
        self.add_function_input = Some(input);
        cx.notify();
    }

    fn hide_add_function_modal(&mut self, cx: &mut Context<Self>) {
        self.show_add_function = false;
        self.add_function_input = None;
        cx.notify();
    }

    fn function_matches_search(function: &Function, search: &str) -> bool {
        let search = search.trim().to_lowercase();
        if search.is_empty() {
            return true;
        }

        function.name.to_lowercase().contains(&search)
            || function.identifier.to_lowercase().contains(&search)
            || function
                .description
                .as_deref()
                .unwrap_or_default()
                .to_lowercase()
                .contains(&search)
            || function.input_schema.to_lowercase().contains(&search)
            || function.output_schema.to_lowercase().contains(&search)
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
                node_config: Some(serde_json::json!({"input_mapping": {}})),
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

    fn show_node_config_panel(
        &mut self,
        node_key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(node) = self.nodes.iter().find(|node| node.node_key == node_key) else {
            return;
        };
        let empty_schema = serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        });
        let config = match node.node_type {
            DagNodeType::StartNode => self
                .workflow
                .as_ref()
                .and_then(|workflow| workflow.input_schema.as_deref())
                .and_then(|schema| serde_json::from_str(schema).ok())
                .unwrap_or_else(|| empty_schema.clone()),
            DagNodeType::EndNode => self
                .workflow
                .as_ref()
                .and_then(|workflow| workflow.output_schema.as_deref())
                .and_then(|schema| serde_json::from_str(schema).ok())
                .unwrap_or(empty_schema),
            DagNodeType::FunctionNode | DagNodeType::GenerateAnswerNode => node
                .node_config
                .as_ref()
                .and_then(|config| config.get("input_mapping"))
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
        };
        let config = serde_json::to_string_pretty(&config).unwrap_or_else(|_| "{}".to_string());
        self.config_input = Some(cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(10)
                .default_value(config)
        }));

        if node.node_type == DagNodeType::GenerateAnswerNode {
            let node_config = node.node_config.as_ref();
            let model_preset = node_config
                .and_then(|config| config.get("model_preset"))
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let history_window = node_config
                .and_then(|config| config.get("history_window"))
                .and_then(|value| value.as_i64())
                .unwrap_or(0);
            let system_prompt = node_config
                .and_then(|config| config.get("system_prompt"))
                .and_then(|value| value.as_str())
                .unwrap_or("你是一个智能助手，请根据以下内容回答用户问题：\n{query}");
            self.config_model_preset_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("留空使用默认模型")
                    .default_value(model_preset)
            }));
            self.config_history_window_input = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("0-50")
                    .default_value(history_window.to_string())
            }));
            self.config_system_prompt_input = Some(cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(8)
                    .default_value(system_prompt)
            }));
        } else {
            self.config_model_preset_input = None;
            self.config_history_window_input = None;
            self.config_system_prompt_input = None;
        }
        self.selected_node = Some(node_key.to_string());
        self.config_node_key = Some(node_key.to_string());
        self.show_node_config = true;
        self.config_error = None;
        cx.notify();
    }

    fn hide_node_config_panel(&mut self, cx: &mut Context<Self>) {
        self.show_node_config = false;
        self.config_node_key = None;
        self.config_input = None;
        self.config_model_preset_input = None;
        self.config_history_window_input = None;
        self.config_system_prompt_input = None;
        self.config_error = None;
        cx.notify();
    }

    fn apply_node_config(&mut self, cx: &mut Context<Self>) {
        let Some(node_key) = self.config_node_key.clone() else {
            return;
        };
        let Some(input) = self.config_input.as_ref() else {
            return;
        };
        let value = input.read(cx).value().to_string();
        let config: serde_json::Value = match serde_json::from_str::<serde_json::Value>(&value) {
            Ok(config) if config.is_object() => config,
            Ok(_) => {
                self.config_error = Some("配置必须是 JSON 对象".to_string());
                cx.notify();
                return;
            }
            Err(error) => {
                self.config_error = Some(format!("JSON 格式错误：{error}"));
                cx.notify();
                return;
            }
        };
        let Some(node_type) = self
            .nodes
            .iter()
            .find(|node| node.node_key == node_key)
            .map(|node| node.node_type.clone())
        else {
            return;
        };

        match node_type {
            DagNodeType::StartNode => {
                let Some(workflow) = self.workflow.as_mut() else {
                    self.config_error = Some("工作流尚未加载完成".to_string());
                    cx.notify();
                    return;
                };
                workflow.input_schema = Some(config.to_string());
            }
            DagNodeType::EndNode => {
                let Some(workflow) = self.workflow.as_mut() else {
                    self.config_error = Some("工作流尚未加载完成".to_string());
                    cx.notify();
                    return;
                };
                workflow.output_schema = Some(config.to_string());
            }
            DagNodeType::FunctionNode | DagNodeType::GenerateAnswerNode => {
                if let Err(error) = Self::validate_input_mapping(&config) {
                    self.config_error = Some(error);
                    cx.notify();
                    return;
                }
                let mut node_config = self
                    .nodes
                    .iter()
                    .find(|node| node.node_key == node_key)
                    .and_then(|node| node.node_config.clone())
                    .filter(|config| config.is_object())
                    .unwrap_or_else(|| serde_json::json!({}));
                node_config["input_mapping"] = config;

                if node_type == DagNodeType::GenerateAnswerNode {
                    let model_preset = self
                        .config_model_preset_input
                        .as_ref()
                        .map(|input| input.read(cx).value().trim().to_string())
                        .unwrap_or_default();
                    let history_window = self
                        .config_history_window_input
                        .as_ref()
                        .map(|input| input.read(cx).value().trim().parse::<i64>())
                        .transpose();
                    let history_window = match history_window {
                        Ok(Some(value @ 0..=50)) => value,
                        Ok(None) => 0,
                        _ => {
                            self.config_error = Some("历史消息窗口必须是 0 到 50 的整数".into());
                            cx.notify();
                            return;
                        }
                    };
                    let system_prompt = self
                        .config_system_prompt_input
                        .as_ref()
                        .map(|input| input.read(cx).value().to_string())
                        .unwrap_or_default();
                    node_config["system_prompt"] = serde_json::Value::String(system_prompt);
                    node_config["history_window"] = history_window.into();
                    if model_preset.is_empty() {
                        node_config
                            .as_object_mut()
                            .expect("node config object")
                            .remove("model_preset");
                    } else {
                        node_config["model_preset"] = serde_json::Value::String(model_preset);
                    }
                }
                self.update_node_config(&node_key, node_config, cx);
            }
        }

        self.config_error = None;
        cx.notify();
    }

    fn validate_input_mapping(mapping: &serde_json::Value) -> Result<(), String> {
        let mapping = mapping
            .as_object()
            .ok_or_else(|| "输入变量映射必须是 JSON 对象".to_string())?;
        for (field, source) in mapping {
            if field.is_empty() {
                return Err("输入变量名不能为空".to_string());
            }
            if matches!(field.as_str(), "_agent_context" | "_agent_context_updates") {
                return Err(format!("输入变量 {field} 是系统保留字段"));
            }
            let source = source
                .as_object()
                .ok_or_else(|| format!("输入变量 {field} 的来源必须是 JSON 对象"))?;
            match source.get("kind").and_then(|kind| kind.as_str()) {
                Some("upstream") => {
                    if source
                        .get("node_key")
                        .and_then(|value| value.as_str())
                        .is_none_or(str::is_empty)
                    {
                        return Err(format!("输入变量 {field} 必须指定上游节点"));
                    }
                }
                Some("custom") => {
                    if !source.contains_key("value") {
                        return Err(format!("输入变量 {field} 必须指定自定义值"));
                    }
                }
                Some("agent_context") => {
                    let category = source.get("category").and_then(|value| value.as_str());
                    if !matches!(
                        category,
                        Some(
                            "user_input"
                                | "entities"
                                | "tool_results"
                                | "state_changes"
                                | "extensions"
                        )
                    ) {
                        return Err(format!("输入变量 {field} 的 AgentContext 分类无效"));
                    }
                    if source
                        .get("key")
                        .and_then(|value| value.as_str())
                        .is_none_or(str::is_empty)
                    {
                        return Err(format!("输入变量 {field} 必须指定 AgentContext key"));
                    }
                    let key = source
                        .get("key")
                        .and_then(|value| value.as_str())
                        .expect("validated AgentContext key");
                    let sub_key = source.get("sub_key").and_then(|value| value.as_str());
                    if category == Some("user_input") {
                        if !matches!(
                            key,
                            "raw_text" | "session_id" | "message_id" | "timestamp" | "metadata"
                        ) {
                            return Err(format!("输入变量 {field} 的 user_input key 无效"));
                        }
                        if sub_key.is_some_and(str::is_empty) {
                            return Err(format!("输入变量 {field} 的 sub_key 不能为空"));
                        }
                        if sub_key.is_some() && key != "metadata" {
                            return Err(format!("输入变量 {field} 只有 metadata 支持 sub_key"));
                        }
                    }
                    if category == Some("extensions") && sub_key.is_none_or(str::is_empty) {
                        return Err(format!(
                            "输入变量 {field} 的 extensions 来源必须指定 sub_key"
                        ));
                    }
                }
                _ => {
                    return Err(format!(
                        "输入变量 {field} 的来源类型必须是 upstream、custom 或 agent_context"
                    ));
                }
            }
        }
        Ok(())
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
            if !visited.get(&node.node_key).unwrap_or(&false)
                && let Some(cycle) = self.dfs_cycle(
                    &node.node_key,
                    &graph,
                    &mut visited,
                    &mut rec_stack,
                    &mut path,
                )
            {
                return Some(cycle);
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
        if let Some((ref node_key, _)) = self.dragging_node
            && let Some(node) = self.nodes.iter_mut().find(|n| &n.node_key == node_key)
        {
            node.position = Point::new(
                px(
                    f32::from(position.x - self.drag_offset.x - self.canvas_offset.x)
                        / self.canvas_zoom,
                ),
                px(
                    f32::from(position.y - self.drag_offset.y - self.canvas_offset.y)
                        / self.canvas_zoom,
                ),
            );
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

    fn on_canvas_mouse_down(&mut self, position: Point<Pixels>) {
        // 点击空白处取消选中
        self.selected_node = None;
        self.selected_edge = None;
        self.context_menu = None;
        self.panning_canvas = Some((position, self.canvas_offset));
    }

    fn on_canvas_mouse_move(&mut self, position: Point<Pixels>) {
        if self.dragging_node.is_some() {
            self.on_node_mouse_move(position);
            return;
        }
        if let Some((start, initial_offset)) = self.panning_canvas {
            self.canvas_offset = Point::new(
                initial_offset.x + position.x - start.x,
                initial_offset.y + position.y - start.y,
            );
        }
    }

    fn on_canvas_mouse_up(&mut self) {
        self.panning_canvas = None;
        self.on_node_mouse_up();
    }

    fn on_canvas_scroll(&mut self, event: &ScrollWheelEvent) {
        let delta = event.delta.pixel_delta(px(16.0));
        let step = if f32::from(delta.y) >= 0.0 { 0.1 } else { -0.1 };
        self.zoom_canvas_at(event.position, step);
    }

    fn zoom_canvas_at(&mut self, anchor: Point<Pixels>, step: f32) {
        let old_zoom = self.canvas_zoom;
        let new_zoom = (old_zoom + step).clamp(0.4, 2.0);
        if (new_zoom - old_zoom).abs() < f32::EPSILON {
            return;
        }

        let anchor_in_canvas = Point::new(
            anchor.x - self.canvas_bounds.origin.x,
            anchor.y - self.canvas_bounds.origin.y,
        );
        let scale_change = new_zoom / old_zoom;
        self.canvas_offset = Point::new(
            anchor_in_canvas.x
                - px(f32::from(anchor_in_canvas.x - self.canvas_offset.x) * scale_change),
            anchor_in_canvas.y
                - px(f32::from(anchor_in_canvas.y - self.canvas_offset.y) * scale_change),
        );
        self.canvas_zoom = new_zoom;
    }

    fn zoom_canvas(&mut self, step: f32, cx: &mut Context<Self>) {
        self.canvas_zoom = (self.canvas_zoom + step).clamp(0.4, 2.0);
        cx.notify();
    }

    fn reset_canvas_view(&mut self, cx: &mut Context<Self>) {
        self.canvas_offset = Point::default();
        self.canvas_zoom = 1.0;
        cx.notify();
    }

    fn canvas_position(&self, position: Point<Pixels>) -> Point<Pixels> {
        Point::new(
            self.canvas_offset.x + px(f32::from(position.x) * self.canvas_zoom),
            self.canvas_offset.y + px(f32::from(position.y) * self.canvas_zoom),
        )
    }

    fn canvas_pixels(&self, value: f32) -> Pixels {
        px(value * self.canvas_zoom)
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
                                    "zoom-out",
                                    "−",
                                    ActionRole::Neutral,
                                    ActionSize::Page,
                                    style,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let t = cx.weak_entity();
                                        move |_, _, cx| {
                                            t.update(cx, |v, cx| v.zoom_canvas(-0.1, cx)).ok();
                                        }
                                    },
                                ),
                            )
                            .child(
                                div()
                                    .min_w(px(52.0))
                                    .text_center()
                                    .text_size(px(12.0))
                                    .child(format!("{:.0}%", self.canvas_zoom * 100.0)),
                            )
                            .child(
                                action_button(
                                    "zoom-in",
                                    "+",
                                    ActionRole::Neutral,
                                    ActionSize::Page,
                                    style,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let t = cx.weak_entity();
                                        move |_, _, cx| {
                                            t.update(cx, |v, cx| v.zoom_canvas(0.1, cx)).ok();
                                        }
                                    },
                                ),
                            )
                            .child(
                                action_button(
                                    "reset-view",
                                    "重置视图",
                                    ActionRole::Neutral,
                                    ActionSize::Page,
                                    style,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let t = cx.weak_entity();
                                        move |_, _, cx| {
                                            t.update(cx, |v, cx| v.reset_canvas_view(cx)).ok();
                                        }
                                    },
                                ),
                            )
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
                    .on_children_prepainted({
                        let t = cx.weak_entity();
                        move |bounds, _, cx| {
                            if let Some(canvas_bounds) = bounds.first().copied() {
                                t.update(cx, |v, _| v.canvas_bounds = canvas_bounds).ok();
                            }
                        }
                    })
                    .id("DAG_CANVAS")
                    .debug_selector(|| "DAG_CANVAS".to_owned())
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
                    .on_mouse_move({
                        let t = cx.weak_entity();
                        move |event: &MouseMoveEvent, _, cx| {
                            if event.pressed_button == Some(MouseButton::Left) {
                                t.update(cx, |v, cx| {
                                    v.on_canvas_mouse_move(event.position);
                                    cx.notify();
                                })
                                .ok();
                            }
                        }
                    })
                    .on_mouse_up(MouseButton::Left, {
                        let t = cx.weak_entity();
                        move |_, _, cx| {
                            t.update(cx, |v, cx| {
                                v.on_canvas_mouse_up();
                                cx.notify();
                            })
                            .ok();
                        }
                    })
                    .on_scroll_wheel({
                        let t = cx.weak_entity();
                        move |event, _, cx| {
                            t.update(cx, |v, cx| {
                                v.on_canvas_scroll(event);
                                cx.notify();
                            })
                            .ok();
                            cx.stop_propagation();
                        }
                    })
                    // 用一个不参与交互的绝对定位子元素记录画布在窗口中的实际边界。
                    .child(
                        div()
                            .absolute()
                            .top(px(0.0))
                            .right(px(0.0))
                            .bottom(px(0.0))
                            .left(px(0.0)),
                    )
                    // 节点
                    .children(self.nodes.iter().map(|node| {
                        let node_key = node.node_key.clone();
                        let node_selector = format!("DAG_NODE-{node_key}");
                        let is_selected = self.selected_node.as_ref() == Some(&node_key);
                        let node_type = node.node_type.clone();
                        let function_name = node.function_name.clone();
                        let position = self.canvas_position(node.position);
                        let node_width = self.canvas_pixels(180.0);
                        let node_height = self.canvas_pixels(100.0);
                        let handle_size = self.canvas_pixels(12.0);
                        let handle_left = self.canvas_pixels(84.0);

                        let bg_color = match node_type {
                            DagNodeType::StartNode => hsla(0.33, 0.7, 0.9, 1.0),
                            DagNodeType::EndNode => hsla(0.0, 0.7, 0.9, 1.0),
                            DagNodeType::FunctionNode => hsla(0.58, 0.7, 0.9, 1.0),
                            DagNodeType::GenerateAnswerNode => hsla(0.75, 0.7, 0.9, 1.0),
                        };

                        div()
                            .id(format!("DAG_NODE-{node_key}"))
                            .debug_selector(move || node_selector.clone())
                            .absolute()
                            .top(position.y)
                            .left(position.x)
                            .w(node_width)
                            .h(node_height)
                            .bg(bg_color)
                            .border_1()
                            .border_color(if is_selected {
                                hsla(0.6, 0.8, 0.5, 1.0)
                            } else {
                                hsla(0.0, 0.0, 0.7, 1.0)
                            })
                            .rounded(self.canvas_pixels(8.0))
                            .shadow_md()
                            .p(self.canvas_pixels(12.0))
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
                                    cx.stop_propagation();
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
                                    cx.stop_propagation();
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
                                        v.selected_node = Some(key.clone());
                                        v.selected_edge = None;
                                        v.context_menu = Some((
                                            Point::new(
                                                event.position.x - v.canvas_bounds.origin.x,
                                                event.position.y - v.canvas_bounds.origin.y,
                                            ),
                                            key.clone(),
                                        ));
                                        cx.notify();
                                    })
                                    .ok();
                                    cx.stop_propagation();
                                }
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(self.canvas_pixels(6.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(self.canvas_pixels(8.0))
                                            .child(
                                                div()
                                                    .w(self.canvas_pixels(24.0))
                                                    .h(self.canvas_pixels(24.0))
                                                    .rounded_full()
                                                    .bg(hsla(0.0, 0.0, 1.0, 1.0))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .text_size(self.canvas_pixels(14.0))
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
                                                    .text_size(self.canvas_pixels(13.0))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .child(function_name.unwrap_or_else(|| {
                                                        node_type.display_name().to_string()
                                                    })),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_size(self.canvas_pixels(11.0))
                                            .text_color(hsla(0.0, 0.0, 0.5, 1.0))
                                            .child(node_key.clone()),
                                    ),
                            )
                            // 顶部连接点（输入）
                            .child(
                                div()
                                    .absolute()
                                    .top(-self.canvas_pixels(6.0))
                                    .left(handle_left)
                                    .w(handle_size)
                                    .h(handle_size)
                                    .rounded_full()
                                    .bg(hsla(0.6, 0.7, 0.6, 1.0))
                                    .border_1()
                                    .border_color(hsla(0.0, 0.0, 1.0, 1.0))
                                    .cursor(CursorStyle::PointingHand)
                                    .on_mouse_down(MouseButton::Left, {
                                        let t = cx.weak_entity();
                                        let key = node_key.clone();
                                        let pos = Point::new(
                                            position.x + self.canvas_pixels(90.0),
                                            position.y,
                                        );
                                        move |_, _, cx| {
                                            t.update(cx, |v, cx| {
                                                v.on_handle_mouse_down(&key, "input", pos);
                                                cx.notify();
                                            })
                                            .ok();
                                            cx.stop_propagation();
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
                                            cx.stop_propagation();
                                        }
                                    }),
                            )
                            // 底部连接点（输出）
                            .child(
                                div()
                                    .absolute()
                                    .bottom(-self.canvas_pixels(6.0))
                                    .left(handle_left)
                                    .w(handle_size)
                                    .h(handle_size)
                                    .rounded_full()
                                    .bg(hsla(0.6, 0.7, 0.6, 1.0))
                                    .border_1()
                                    .border_color(hsla(0.0, 0.0, 1.0, 1.0))
                                    .cursor(CursorStyle::PointingHand)
                                    .on_mouse_down(MouseButton::Left, {
                                        let t = cx.weak_entity();
                                        let key = node_key.clone();
                                        let pos = Point::new(
                                            position.x + self.canvas_pixels(90.0),
                                            position.y + node_height,
                                        );
                                        move |_, _, cx| {
                                            t.update(cx, |v, cx| {
                                                v.on_handle_mouse_down(&key, "output", pos);
                                                cx.notify();
                                            })
                                            .ok();
                                            cx.stop_propagation();
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
                                            cx.stop_propagation();
                                        }
                                    }),
                            )
                    }))
                    // 连线渲染
                    .children(self.edges.iter().map(|edge| {
                        let src = self.nodes.iter().find(|n| n.node_key == edge.src_node_key);
                        let dst = self.nodes.iter().find(|n| n.node_key == edge.dst_node_key);

                        if let (Some(src_node), Some(dst_node)) = (src, dst) {
                            let src_pos = self.canvas_position(src_node.position);
                            let dst_pos = self.canvas_position(dst_node.position);
                            let is_selected = self
                                .selected_edge
                                .as_ref()
                                .map(|(s, d)| s == &edge.src_node_key && d == &edge.dst_node_key)
                                .unwrap_or(false);

                            // 计算连线起点和终点（节点底部中心到顶部中心）
                            let start_x = src_pos.x + self.canvas_pixels(90.0);
                            let start_y = src_pos.y + self.canvas_pixels(100.0);
                            let end_x = dst_pos.x + self.canvas_pixels(90.0);
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
                                                .id("DAG_NODE_MENU_CONFIG")
                                                .debug_selector(|| {
                                                    "DAG_NODE_MENU_CONFIG".to_owned()
                                                })
                                                .text_size(px(12.0))
                                                .px(px(8.0))
                                                .py(px(4.0))
                                                .rounded(px(4.0))
                                                .cursor(CursorStyle::PointingHand)
                                                .hover(|this| this.opacity(0.8))
                                                .child("配置节点")
                                                .on_mouse_down(MouseButton::Left, {
                                                    let t = cx.weak_entity();
                                                    let k = key.clone();
                                                    move |_, window, cx| {
                                                        t.update(cx, |v, cx| {
                                                            v.show_node_config_panel(
                                                                &k, window, cx,
                                                            );
                                                            v.context_menu = None;
                                                        })
                                                        .ok();
                                                    }
                                                }),
                                        )
                                        .when(key != "start" && key != "end", |this| {
                                            this.child(
                                                div()
                                                    .id("DAG_NODE_MENU_DELETE")
                                                    .debug_selector(|| {
                                                        "DAG_NODE_MENU_DELETE".to_owned()
                                                    })
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
                    .filter(|function| Self::function_matches_search(function, &search))
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
                                        .id("DAG_FUNCTION_SUGGESTIONS")
                                        .debug_selector(|| "DAG_FUNCTION_SUGGESTIONS".to_owned())
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.0))
                                        .max_h(px(300.0))
                                        .overflow_y_scrollbar()
                                        .children(filtered_functions.into_iter().map(|f| {
                                            let fid = f.id;
                                            div()
                                                .id(format!("DAG_FUNCTION_OPTION-{fid}"))
                                                .debug_selector(move || {
                                                    format!("DAG_FUNCTION_OPTION-{fid}")
                                                })
                                                .flex()
                                                .flex_col()
                                                .items_start()
                                                .gap(px(4.0))
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
                                                .when_some(f.description.clone(), |this, desc| {
                                                    this.child(
                                                        div()
                                                            .max_w(px(390.0))
                                                            .text_ellipsis()
                                                            .text_size(px(12.0))
                                                            .text_color(hsla(0.0, 0.0, 0.5, 1.0))
                                                            .child(desc),
                                                    )
                                                })
                                                .child(
                                                    div()
                                                        .text_size(px(11.0))
                                                        .max_w(px(420.0))
                                                        .text_ellipsis()
                                                        .text_color(hsla(0.0, 0.0, 0.5, 1.0))
                                                        .child(format!(
                                                            "输入: {}  输出: {}",
                                                            f.input_schema, f.output_schema
                                                        )),
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
                    let config_input = self.config_input.clone();
                    let model_preset_input = self.config_model_preset_input.clone();
                    let history_window_input = self.config_history_window_input.clone();
                    let system_prompt_input = self.config_system_prompt_input.clone();
                    let config_error = self.config_error.clone();
                    let function = node
                        .function_id
                        .and_then(|id| self.functions.iter().find(|function| function.id == id))
                        .cloned();
                    let (config_label, config_help, config_selector) = match &node_type {
                        DagNodeType::StartNode => (
                            "工作流输入 Schema (JSON)",
                            "定义工作流接收的输入变量，对应 Hiveweb 的 input_schema。",
                            "DAG_CONFIG_WORKFLOW_SCHEMA",
                        ),
                        DagNodeType::EndNode => (
                            "工作流输出 Schema (JSON)",
                            "定义工作流最终输出的变量，对应 Hiveweb 的 output_schema。",
                            "DAG_CONFIG_WORKFLOW_SCHEMA",
                        ),
                        DagNodeType::FunctionNode | DagNodeType::GenerateAnswerNode => (
                            "输入变量映射 (JSON)",
                            "每个字段的来源支持 upstream、custom 或 agent_context。",
                            "DAG_CONFIG_INPUT_MAPPING",
                        ),
                    };
                    let config_editor = div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(div().text_size(px(12.0)).child(config_label))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(hsla(0.0, 0.0, 0.5, 1.0))
                                .child(config_help),
                        )
                        .child(
                            div()
                                .id(config_selector)
                                .debug_selector(move || config_selector.to_owned())
                                .h(px(220.0))
                                .p(px(8.0))
                                .bg(theme.background)
                                .border_1()
                                .border_color(theme.border)
                                .rounded(px(4.0))
                                .child(
                                    div()
                                        .id("DAG_NODE_CONFIG_EDITOR")
                                        .debug_selector(|| "DAG_NODE_CONFIG_EDITOR".to_owned())
                                        .size_full()
                                        .when_some(config_input, |this, input| {
                                            this.child(
                                                Textarea::new(&input)
                                                    .size_full()
                                                    .border_0()
                                                    .font_family("monospace"),
                                            )
                                        }),
                                ),
                        );

                    let body = div()
                        .flex()
                        .flex_col()
                        .gap(px(14.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(3.0))
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(hsla(0.0, 0.0, 0.5, 1.0))
                                        .child("节点标识"),
                                )
                                .child(div().text_size(px(14.0)).child(node_key.clone())),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(3.0))
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(hsla(0.0, 0.0, 0.5, 1.0))
                                        .child("节点类型"),
                                )
                                .child(div().text_size(px(14.0)).child(node_type.display_name())),
                        );
                    let body = match &node_type {
                        DagNodeType::StartNode => body
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(hsla(0.0, 0.0, 0.5, 1.0))
                                    .child("配置工作流的输入变量。开始节点不能删除。"),
                            )
                            .child(config_editor),
                        DagNodeType::EndNode => body
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(hsla(0.0, 0.0, 0.5, 1.0))
                                    .child("配置工作流的输出变量。结束节点不能删除。"),
                            )
                            .child(config_editor),
                        DagNodeType::FunctionNode => body
                            .when_some(function, |this, function| {
                                this.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(6.0))
                                        .p(px(10.0))
                                        .bg(theme.background)
                                        .border_1()
                                        .border_color(theme.border)
                                        .rounded(px(6.0))
                                        .child(
                                            div()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child(function.name),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .text_color(hsla(0.0, 0.0, 0.5, 1.0))
                                                .child(function.identifier),
                                        )
                                        .when_some(function.description, |this, description| {
                                            this.child(div().text_size(px(12.0)).child(description))
                                        })
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .text_color(hsla(0.0, 0.0, 0.5, 1.0))
                                                .child(format!(
                                                    "输入 Schema: {}",
                                                    function.input_schema
                                                )),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .text_color(hsla(0.0, 0.0, 0.5, 1.0))
                                                .child(format!(
                                                    "输出 Schema: {}",
                                                    function.output_schema
                                                )),
                                        ),
                                )
                            })
                            .child(config_editor),
                        DagNodeType::GenerateAnswerNode => body
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(hsla(0.0, 0.0, 0.5, 1.0))
                                    .child(
                                        "变量名可在系统提示词中通过 {变量名} 引用；{context} 表示历史消息。",
                                    ),
                            )
                            .child(config_editor)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.0))
                                    .child(div().text_size(px(12.0)).child("模型预设"))
                                    .child(
                                        div()
                                            .id("DAG_CONFIG_MODEL_PRESET")
                                            .debug_selector(|| {
                                                "DAG_CONFIG_MODEL_PRESET".to_owned()
                                            })
                                            .when_some(model_preset_input, |this, input| {
                                                this.child(Input::new(&input))
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(hsla(0.0, 0.0, 0.5, 1.0))
                                            .child("留空时使用本地默认模型预设。"),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.0))
                                    .child(div().text_size(px(12.0)).child("历史消息窗口"))
                                    .child(
                                        div()
                                            .id("DAG_CONFIG_HISTORY_WINDOW")
                                            .debug_selector(|| {
                                                "DAG_CONFIG_HISTORY_WINDOW".to_owned()
                                            })
                                            .when_some(history_window_input, |this, input| {
                                                this.child(Input::new(&input))
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(hsla(0.0, 0.0, 0.5, 1.0))
                                            .child("允许 0 到 50；0 表示不传递历史消息。"),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.0))
                                    .child(div().text_size(px(12.0)).child("系统提示词"))
                                    .child(
                                        div()
                                            .id("DAG_CONFIG_SYSTEM_PROMPT")
                                            .debug_selector(|| {
                                                "DAG_CONFIG_SYSTEM_PROMPT".to_owned()
                                            })
                                            .h(px(180.0))
                                            .when_some(system_prompt_input, |this, input| {
                                                this.child(Textarea::new(&input).size_full())
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .p(px(10.0))
                                    .rounded(px(6.0))
                                    .bg(theme.background)
                                    .text_size(px(11.0))
                                    .child("固定输出：answer、model_preset"),
                            ),
                    };

                    this.child(
                        div()
                            .absolute()
                            .top(px(0.0))
                            .right(px(0.0))
                            .bottom(px(0.0))
                            .w(px(420.0))
                            .flex()
                            .flex_col()
                            .min_h_0()
                            .bg(theme.popover)
                            .border_l_1()
                            .border_color(theme.border)
                            .shadow_lg()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .flex_none()
                                    .p(px(20.0))
                                    .border_b_1()
                                    .border_color(theme.border)
                                    .child(
                                        div()
                                            .text_size(px(16.0))
                                            .font_weight(FontWeight::BOLD)
                                            .child(format!("{}配置", node_type.display_name())),
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
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scrollbar()
                                    .gap(px(14.0))
                                    .p(px(20.0))
                                    .child(body)
                                    .when_some(config_error, |this, error| {
                                        this.child(
                                            div()
                                                .text_size(px(12.0))
                                                .text_color(theme.danger)
                                                .child(error),
                                        )
                                    })
                                    .child(
                                        div().flex().justify_end().child(
                                            action_button(
                                                "DAG_NODE_CONFIG_APPLY",
                                                "应用配置",
                                                ActionRole::Main,
                                                ActionSize::Page,
                                                style,
                                            )
                                            .debug_selector(|| {
                                                "DAG_NODE_CONFIG_APPLY".to_owned()
                                            })
                                            .on_mouse_down(MouseButton::Left, {
                                                let t = cx.weak_entity();
                                                move |_, _, cx| {
                                                    t.update(cx, |v, cx| {
                                                        v.apply_node_config(cx)
                                                    })
                                                    .ok();
                                                }
                                            }),
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

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use gpui::{
        AppContext, Focusable, Modifiers, MouseButton, ScrollDelta, ScrollWheelEvent,
        TestAppContext, TouchPhase, VisualTestContext, WindowHandle, point, px, size,
    };

    use super::{DagEditorView, DagNode, DagNodeType};
    use crate::datasource::{
        Store,
        entity_store::{Function, Workflow},
    };

    fn init_gpui(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::theme::init(cx);
            gpui_component::init(cx);
        });
    }

    fn test_store(cx: &mut TestAppContext) -> (tempfile::TempDir, gpui::Entity<Store>) {
        let temp_dir = tempfile::tempdir().expect("create temporary data directory");
        let runtime = tokio::runtime::Runtime::new().expect("create Tokio runtime");
        let store = runtime
            .block_on(Store::new(temp_dir.path()))
            .expect("create test store");
        (temp_dir, cx.new(|_| store))
    }

    fn test_editor(store: gpui::Entity<Store>) -> DagEditorView {
        DagEditorView {
            store,
            workflow_id: 1,
            workflow: None,
            nodes: vec![
                DagNode {
                    node_key: "start".into(),
                    node_type: DagNodeType::StartNode,
                    function_id: None,
                    function_name: None,
                    position: point(px(100.0), px(80.0)),
                    node_config: None,
                },
                DagNode {
                    node_key: "end".into(),
                    node_type: DagNodeType::EndNode,
                    function_id: None,
                    function_name: None,
                    position: point(px(500.0), px(320.0)),
                    node_config: None,
                },
            ],
            edges: Vec::new(),
            functions: Vec::new(),
            loading: false,
            error_message: None,
            dragging_node: None,
            drag_offset: Default::default(),
            panning_canvas: None,
            canvas_bounds: Default::default(),
            canvas_offset: Default::default(),
            canvas_zoom: 1.0,
            creating_edge: None,
            selected_node: None,
            selected_edge: None,
            context_menu: None,
            show_add_function: false,
            add_function_input: None,
            add_function_search: String::new(),
            show_node_config: false,
            config_node_key: None,
            config_input: None,
            config_model_preset_input: None,
            config_history_window_input: None,
            config_system_prompt_input: None,
            config_error: None,
        }
    }

    fn open_editor_window(
        cx: &mut TestAppContext,
        store: gpui::Entity<Store>,
    ) -> (
        WindowHandle<gpui_component::Root>,
        gpui::Entity<DagEditorView>,
    ) {
        let editor = Rc::new(RefCell::new(None));
        let editor_for_window = editor.clone();
        let window = cx.open_window(size(px(900.0), px(600.0)), move |window, cx| {
            let inner = cx.new(|_| test_editor(store));
            *editor_for_window.borrow_mut() = Some(inner.clone());
            gpui_component::Root::new(inner, window, cx).bordered(false)
        });
        let editor = editor.borrow_mut().take().expect("DAG editor entity");
        (window, editor)
    }

    fn function(
        id: i64,
        name: &str,
        description: &str,
        input_schema: &str,
        output_schema: &str,
    ) -> Function {
        Function {
            id,
            identifier: format!("function_{id}"),
            name: name.into(),
            description: Some(description.into()),
            kind: 1,
            input_schema: input_schema.into(),
            output_schema: output_schema.into(),
            plugin_id: None,
            plugin_export: None,
            category_id: None,
            required_capabilities: None,
            created_at: "2026-07-21T00:00:00Z".into(),
            updated_at: "2026-07-21T00:00:00Z".into(),
        }
    }

    fn workflow() -> Workflow {
        Workflow {
            id: 1,
            identifier: "test-workflow".into(),
            name: "测试工作流".into(),
            description: None,
            timeout_ms: 30_000,
            category_id: None,
            input_schema: None,
            start_description: None,
            output_schema: None,
            required_capabilities: None,
            created_at: "2026-07-21T00:00:00Z".into(),
            updated_at: "2026-07-21T00:00:00Z".into(),
        }
    }

    #[gpui::test]
    fn empty_workflow_loads_required_start_and_end_nodes(cx: &mut TestAppContext) {
        let (_temp_dir, store) = test_store(cx);
        let mut editor = test_editor(store);
        editor.nodes.clear();
        editor.ensure_boundary_nodes();
        assert_eq!(
            editor
                .nodes
                .iter()
                .filter(|node| node.node_type == DagNodeType::StartNode)
                .count(),
            1,
            "an empty workflow must contain exactly one start node"
        );
        assert_eq!(
            editor
                .nodes
                .iter()
                .filter(|node| node.node_type == DagNodeType::EndNode)
                .count(),
            1,
            "an empty workflow must contain exactly one end node"
        );
    }

    #[gpui::test]
    fn function_picker_focuses_search_and_opens_suggestions(cx: &mut TestAppContext) {
        init_gpui(cx);
        let (_temp_dir, store) = test_store(cx);
        let (window, editor) = open_editor_window(cx, store);
        cx.run_until_parked();

        window
            .update(cx, |_, window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.functions = vec![function(
                        1,
                        "查询余额",
                        "查询本地账号余额",
                        r#"{"type":"object"}"#,
                        r#"{"type":"object"}"#,
                    )];
                    editor.show_add_function_modal(window, cx);
                    let input = editor
                        .add_function_input
                        .as_ref()
                        .expect("function search input");
                    assert!(
                        input.read(cx).focus_handle(cx).is_focused(window),
                        "opening the function picker must focus its search input"
                    );
                });
            })
            .expect("update DAG editor");
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);
        assert!(
            cx.debug_bounds("DAG_FUNCTION_SUGGESTIONS").is_some(),
            "function suggestions must open as soon as the picker gains focus"
        );
        assert!(
            cx.debug_bounds("DAG_FUNCTION_OPTION-1").is_some(),
            "the default suggestion list must contain available functions"
        );
    }

    #[gpui::test]
    fn function_picker_filters_name_description_input_and_output(cx: &mut TestAppContext) {
        init_gpui(cx);
        let (_temp_dir, store) = test_store(cx);
        let (window, editor) = open_editor_window(cx, store);
        cx.run_until_parked();

        window
            .update(cx, |_, window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.functions = vec![
                        function(1, "needle name", "ordinary", "{}", "{}"),
                        function(2, "ordinary", "needle description", "{}", "{}"),
                        function(3, "ordinary", "ordinary", r#"{"needle_input":1}"#, "{}"),
                        function(4, "ordinary", "ordinary", "{}", r#"{"needle_output":1}"#),
                        function(5, "unrelated", "unrelated", "{}", "{}"),
                    ];
                    editor.show_add_function_modal(window, cx);
                });
            })
            .expect("open function picker");
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);
        cx.simulate_input("needle");
        cx.run_until_parked();

        for selector in [
            "DAG_FUNCTION_OPTION-1",
            "DAG_FUNCTION_OPTION-2",
            "DAG_FUNCTION_OPTION-3",
            "DAG_FUNCTION_OPTION-4",
        ] {
            assert!(
                cx.debug_bounds(selector).is_some(),
                "query should match option {selector}"
            );
        }
        assert!(
            cx.debug_bounds("DAG_FUNCTION_OPTION-5").is_none(),
            "an unrelated function must be filtered out"
        );
    }

    #[gpui::test]
    fn right_click_selects_node_and_shows_config_and_delete_actions(cx: &mut TestAppContext) {
        init_gpui(cx);
        let (_temp_dir, store) = test_store(cx);
        let (window, editor) = open_editor_window(cx, store);
        window
            .update(cx, |_, _, cx| {
                editor.update(cx, |editor, cx| {
                    editor.nodes.push(DagNode {
                        node_key: "function_1".into(),
                        node_type: DagNodeType::FunctionNode,
                        function_id: Some(1),
                        function_name: Some("测试函数".into()),
                        position: point(px(300.0), px(160.0)),
                        node_config: Some(serde_json::json!({"input_mapping": {}})),
                    });
                    cx.notify();
                });
            })
            .unwrap();
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let node = cx
            .debug_bounds("DAG_NODE-function_1")
            .expect("function node bounds");
        cx.simulate_mouse_down(node.center(), MouseButton::Right, Modifiers::default());
        cx.run_until_parked();

        cx.read(|cx| {
            assert_eq!(
                editor.read(cx).selected_node.as_deref(),
                Some("function_1"),
                "right-clicking a node must select it"
            );
        });
        assert!(
            cx.debug_bounds("DAG_NODE_MENU_CONFIG").is_some(),
            "node context menu must contain the configure action"
        );
        assert!(
            cx.debug_bounds("DAG_NODE_MENU_DELETE").is_some(),
            "deletable nodes must expose the delete action"
        );
        let delete = cx
            .debug_bounds("DAG_NODE_MENU_DELETE")
            .expect("delete action bounds");
        cx.simulate_click(delete.center(), Modifiers::default());
        cx.run_until_parked();
        cx.read(|cx| {
            assert!(
                editor
                    .read(cx)
                    .nodes
                    .iter()
                    .all(|node| node.node_key != "function_1"),
                "delete action must remove the selected node"
            );
        });

        let start = cx
            .debug_bounds("DAG_NODE-start")
            .expect("start node bounds");
        cx.simulate_mouse_down(start.center(), MouseButton::Right, Modifiers::default());
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("DAG_NODE_MENU_CONFIG").is_some(),
            "start node must remain configurable"
        );
        assert!(
            cx.debug_bounds("DAG_NODE_MENU_DELETE").is_none(),
            "start and end nodes must not expose the delete action"
        );
    }

    #[gpui::test]
    fn start_node_config_updates_workflow_input_schema(cx: &mut TestAppContext) {
        init_gpui(cx);
        let (_temp_dir, store) = test_store(cx);
        let (window, editor) = open_editor_window(cx, store);
        window
            .update(cx, |window_root, window, cx| {
                let _ = window_root;
                editor.update(cx, |editor, cx| {
                    editor.workflow = Some(workflow());
                    editor.show_node_config_panel("start", window, cx);
                    let input = editor.config_input.clone().expect("schema input");
                    input.update(cx, |input, cx| {
                        input.set_value(
                            r#"{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}"#,
                            window,
                            cx,
                        );
                    });
                    editor.apply_node_config(cx);
                });
            })
            .unwrap();

        cx.read(|cx| {
            let workflow = editor.read(cx).workflow.as_ref().expect("workflow");
            let schema: serde_json::Value =
                serde_json::from_str(workflow.input_schema.as_deref().expect("input schema"))
                    .unwrap();
            assert_eq!(schema["properties"]["query"]["type"], "string");
        });
    }

    #[gpui::test]
    fn answer_node_config_exposes_hiveweb_workflow_properties(cx: &mut TestAppContext) {
        init_gpui(cx);
        let (_temp_dir, store) = test_store(cx);
        let (window, editor) = open_editor_window(cx, store);
        window
            .update(cx, |_, window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.nodes.push(DagNode {
                        node_key: "answer_1".into(),
                        node_type: DagNodeType::GenerateAnswerNode,
                        function_id: None,
                        function_name: None,
                        position: point(px(300.0), px(160.0)),
                        node_config: Some(serde_json::json!({
                            "system_prompt": "回答 {query}",
                            "model_preset": "default",
                            "history_window": 4,
                            "input_mapping": {}
                        })),
                    });
                    editor.show_node_config_panel("answer_1", window, cx);
                });
            })
            .unwrap();
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);
        for selector in [
            "DAG_CONFIG_INPUT_MAPPING",
            "DAG_CONFIG_MODEL_PRESET",
            "DAG_CONFIG_HISTORY_WINDOW",
            "DAG_CONFIG_SYSTEM_PROMPT",
        ] {
            assert!(
                cx.debug_bounds(selector).is_some(),
                "answer node config must expose {selector}"
            );
        }

        cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                let mapping = editor.config_input.clone().unwrap();
                let model = editor.config_model_preset_input.clone().unwrap();
                let history = editor.config_history_window_input.clone().unwrap();
                let prompt = editor.config_system_prompt_input.clone().unwrap();
                mapping.update(cx, |input, cx| {
                    input.set_value(
                        r#"{"query":{"kind":"upstream","node_key":"start","field":"query"}}"#,
                        window,
                        cx,
                    )
                });
                model.update(cx, |input, cx| input.set_value("local-default", window, cx));
                history.update(cx, |input, cx| input.set_value("6", window, cx));
                prompt.update(cx, |input, cx| {
                    input.set_value("请回答 {query}，参考 {context}", window, cx)
                });
                editor.apply_node_config(cx);
            });
        });
        cx.read(|cx| {
            let config = editor
                .read(cx)
                .nodes
                .iter()
                .find(|node| node.node_key == "answer_1")
                .and_then(|node| node.node_config.as_ref())
                .cloned()
                .expect("answer node config");
            assert_eq!(config["model_preset"], "local-default");
            assert_eq!(config["history_window"], 6);
            assert_eq!(config["system_prompt"], "请回答 {query}，参考 {context}");
            assert_eq!(config["input_mapping"]["query"]["node_key"], "start");
        });
    }

    #[gpui::test]
    fn canvas_wheel_zooms_and_blank_drag_pans_nodes(cx: &mut TestAppContext) {
        init_gpui(cx);
        let (_temp_dir, store) = test_store(cx);
        let (window, _editor) = open_editor_window(cx, store);
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let canvas = cx.debug_bounds("DAG_CANVAS").expect("DAG canvas bounds");
        let node_before = cx
            .debug_bounds("DAG_NODE-start")
            .expect("start node bounds before canvas transforms");

        cx.simulate_event(ScrollWheelEvent {
            position: node_before.center(),
            delta: ScrollDelta::Pixels(point(px(0.0), px(160.0))),
            modifiers: Default::default(),
            touch_phase: TouchPhase::Moved,
        });
        cx.run_until_parked();
        let node_after_zoom = cx
            .debug_bounds("DAG_NODE-start")
            .expect("start node bounds after zoom");
        assert_ne!(
            node_after_zoom.size, node_before.size,
            "mouse wheel over the canvas must change the zoom level"
        );
        assert!(
            (f32::from(node_after_zoom.center().x - node_before.center().x)).abs() <= 1.0
                && (f32::from(node_after_zoom.center().y - node_before.center().y)).abs() <= 1.0,
            "the canvas coordinate under the pointer must stay under the pointer: before={node_before:?}, after={node_after_zoom:?}"
        );

        cx.simulate_event(ScrollWheelEvent {
            position: node_after_zoom.center(),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-160.0))),
            modifiers: Default::default(),
            touch_phase: TouchPhase::Moved,
        });
        cx.run_until_parked();
        let node_after_zoom_out = cx
            .debug_bounds("DAG_NODE-start")
            .expect("start node bounds after zooming out");
        assert_ne!(
            node_after_zoom_out.size, node_after_zoom.size,
            "reverse mouse wheel movement must zoom the canvas out"
        );
        assert!(
            (f32::from(node_after_zoom_out.center().x - node_after_zoom.center().x)).abs() <= 1.0
                && (f32::from(node_after_zoom_out.center().y - node_after_zoom.center().y)).abs()
                    <= 1.0,
            "zooming out must keep the canvas coordinate under the pointer fixed: before={node_after_zoom:?}, after={node_after_zoom_out:?}"
        );

        let drag_from = point(canvas.right() - px(40.0), canvas.bottom() - px(40.0));
        let drag_to = point(drag_from.x - px(70.0), drag_from.y - px(50.0));
        cx.simulate_mouse_down(drag_from, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(drag_to, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(drag_to, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();

        let node_after_pan = cx
            .debug_bounds("DAG_NODE-start")
            .expect("start node bounds after pan");
        assert_ne!(
            node_after_pan.origin, node_after_zoom_out.origin,
            "dragging a blank part of the canvas must pan every node"
        );
    }

    #[gpui::test]
    fn function_node_config_edits_and_applies_structured_input_mapping(cx: &mut TestAppContext) {
        init_gpui(cx);
        let (_temp_dir, store) = test_store(cx);
        let (window, editor_entity) = open_editor_window(cx, store);
        cx.run_until_parked();

        window
            .update(cx, |_, window, cx| {
                editor_entity.update(cx, |editor, cx| {
                    editor.nodes.push(DagNode {
                        node_key: "function_1".into(),
                        node_type: DagNodeType::FunctionNode,
                        function_id: Some(1),
                        function_name: Some("测试函数".into()),
                        position: point(px(300.0), px(160.0)),
                        node_config: Some(serde_json::json!({"input_mapping": {}})),
                    });
                    editor.show_node_config_panel("function_1", window, cx);
                });
            })
            .expect("open node config panel");
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let editor_bounds = cx
            .debug_bounds("DAG_NODE_CONFIG_EDITOR")
            .expect("editable node config JSON field");
        let apply_bounds = cx
            .debug_bounds("DAG_NODE_CONFIG_APPLY")
            .expect("apply node config button");

        cx.simulate_click(editor_bounds.center(), Modifiers::default());
        cx.simulate_keystrokes("ctrl-a");
        cx.simulate_input(r#"{"query":{"kind":"upstream","node_key":"start","field":"query"}}"#);
        cx.simulate_click(apply_bounds.center(), Modifiers::default());
        cx.run_until_parked();

        cx.read(|cx| {
            let editor = editor_entity.read(cx);
            let config = editor
                .nodes
                .iter()
                .find(|node| node.node_key == "function_1")
                .and_then(|node| node.node_config.as_ref())
                .expect("updated node config");
            assert_eq!(config["input_mapping"]["query"]["node_key"], "start");
        });
    }
}
