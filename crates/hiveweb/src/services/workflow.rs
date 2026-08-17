//! Workflow service (T108 / US3)
//!
//! Workflow = metadata + 节点 + 边。
//! - metadata CRUD (identifier / name / description / timeout_ms)
//! - graph GET/PUT：全量替换 nodes + edges
//! - 校验：
//!   * 所有 function_id 必须存在
//!   * 无环（DFS 标准 white/gray/black 算法）→ 5 4092 DagCycle
//!   * edge mapping JSON: "{dst.input.<field>": "<src_node_key>.output.<path>"}
//!     检查 src_node_key 在 workflow 内存在 + dst Function 的 required input 都有上游 mapping
//!     不满足 → 5005 WorkflowMappingInvalid

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::MySqlPool;
use std::collections::{HashMap, HashSet};

use crate::db::sql_safety::audit_sql;
use crate::models::{NodeType, Workflow, WorkflowEdge, WorkflowNode};
use crate::services::optimistic_lock::OptimisticLockTable;
use crate::utils::error::AppError;

#[derive(Debug, Deserialize)]
pub struct CreateMeta {
    pub identifier: String,
    pub name: String,
    pub description: Option<String>,
    pub timeout_ms: Option<i32>,
    pub category_id: Option<i64>,
    pub required_capabilities: Option<Vec<String>>,
    #[serde(default)]
    pub tag_ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMeta {
    pub name: Option<String>,
    pub description: Option<String>,
    pub timeout_ms: Option<i32>,
    pub category_id: Option<i64>,
    pub required_capabilities: Option<Vec<String>>,
    #[serde(default)]
    pub tag_ids: Option<Vec<i64>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Default)]
pub struct ListFilter {
    pub id: Option<i64>,
    pub identifier: Option<String>,
    pub name: Option<String>,
    pub search: Option<String>,
    pub category_id: Option<i64>,
    pub tag_id: Option<i64>,
    pub required_capabilities: Option<String>,
    pub timeout_ms_from: Option<i64>,
    pub timeout_ms_to: Option<i64>,
    pub created_at_from: Option<DateTime<Utc>>,
    pub created_at_to: Option<DateTime<Utc>>,
    pub updated_at_from: Option<DateTime<Utc>>,
    pub updated_at_to: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct TagSummary {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct WorkflowListItem {
    #[serde(flatten)]
    pub workflow: Workflow,
    #[serde(default)]
    pub tags: Vec<TagSummary>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowList {
    pub items: Vec<WorkflowListItem>,
    pub total: i64,
}

/// 客户端提交 / 服务端返回的图结构（无 id；由 server 重新分配）
#[derive(Debug, Deserialize, Serialize)]
pub struct GraphNode {
    /// 服务端返回时携带；客户端 PUT 时忽略
    #[serde(default)]
    pub id: Option<i64>,
    pub node_key: String,
    #[serde(default = "default_node_type")]
    pub node_type: NodeType,
    pub function_id: Option<i64>,
    #[serde(default)]
    pub position: Option<Value>,
    /// Answer node specific configuration
    #[serde(default)]
    pub node_config: Option<Value>,
}

fn default_node_type() -> NodeType {
    NodeType::FunctionNode
}

/// 将 NodeType 转换为数据库 ENUM 字符串
fn node_type_to_db_str(nt: NodeType) -> &'static str {
    match nt {
        NodeType::FunctionNode => "function_node",
        NodeType::StartNode => "start_node",
        NodeType::EndNode => "end_node",
        NodeType::GenerateAnswerNode => "generate_answer_node",
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GraphEdge {
    #[serde(default)]
    pub id: Option<i64>,
    /// 客户端用 node_key 引用节点；server 解析为 db id
    pub src_node_key: String,
    pub dst_node_key: String,
    pub mapping: Value,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WorkflowGraph {
    pub workflow: Workflow,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Deserialize)]
pub struct GraphPut {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

// ============================== CRUD metadata ==============================

pub async fn create(pool: &MySqlPool, meta: CreateMeta) -> Result<Workflow, AppError> {
    let default_schema = serde_json::json!({ "type": "object", "properties": {} });
    let res = sqlx::query(
        "INSERT INTO workflows (identifier, name, description, timeout_ms, category_id, input_schema, required_capabilities) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&meta.identifier)
    .bind(&meta.name)
    .bind(&meta.description)
    .bind(meta.timeout_ms.unwrap_or(300000))
    .bind(meta.category_id)
    .bind(&default_schema)
    .bind(meta.required_capabilities.as_ref().map(|c| serde_json::to_value(c).unwrap_or(Value::Array(vec![]))))
    .execute(pool)
    .await
    .map_err(|e| {
        let m = e.to_string();
        if m.contains("Duplicate") {
            AppError::Conflict(format!("workflow identifier 已存在：{}", meta.identifier))
        } else {
            AppError::Internal(format!("workflow insert: {e}"))
        }
    })?;
    let id = res.last_insert_id() as i64;
    apply_tags(pool, id, &meta.tag_ids).await?;
    fetch_by_id(pool, id).await
}

async fn fetch_tags(pool: &MySqlPool, entity_id: i64) -> Result<Vec<TagSummary>, AppError> {
    let tags = sqlx::query_as::<_, TagSummary>(
        r#"SELECT t.id, t.name FROM tags t
           JOIN taggings tg ON tg.tag_id = t.id
           WHERE tg.entity_type = 'workflow' AND tg.entity_id = ?"#,
    )
    .bind(entity_id)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("workflow tags: {e}")))?;
    Ok(tags)
}

async fn apply_tags(pool: &MySqlPool, entity_id: i64, tag_ids: &[i64]) -> Result<(), AppError> {
    sqlx::query("DELETE FROM taggings WHERE entity_type = 'workflow' AND entity_id = ?")
        .bind(entity_id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("taggings clear: {e}")))?;
    for &tid in tag_ids {
        sqlx::query(
            "INSERT INTO taggings (tag_id, entity_type, entity_id) VALUES (?, 'workflow', ?)",
        )
        .bind(tid)
        .bind(entity_id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("tagging insert: {e}")))?;
    }
    Ok(())
}

pub async fn fetch_by_id(pool: &MySqlPool, id: i64) -> Result<Workflow, AppError> {
    sqlx::query_as::<_, Workflow>("SELECT * FROM workflows WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("workflow fetch: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("workflow id={id} not found")))
}

pub async fn list(
    pool: &MySqlPool,
    offset: i64,
    limit: i64,
    filter: ListFilter,
) -> Result<WorkflowList, AppError> {
    let mut where_clauses: Vec<String> = Vec::new();
    let mut search_pattern: Option<String> = None;
    let mut params: Vec<String> = Vec::new();

    if let Some(id) = filter.id {
        where_clauses.push("w.id = ?".to_string());
        params.push(id.to_string());
    }
    if let Some(ref identifier) = filter.identifier {
        where_clauses.push("w.identifier LIKE ?".to_string());
        params.push(format!("%{}%", identifier));
    }
    if let Some(ref name) = filter.name {
        where_clauses.push("w.name LIKE ?".to_string());
        params.push(format!("%{}%", name));
    }
    if let Some(ref search) = filter.search {
        where_clauses.push("(w.identifier LIKE ? OR w.name LIKE ? OR w.description LIKE ? OR CAST(w.required_capabilities AS CHAR) LIKE ?)".to_string());
        search_pattern = Some(format!("%{}%", search));
    }
    if let Some(cid) = filter.category_id {
        where_clauses.push("w.category_id = ?".to_string());
        params.push(cid.to_string());
    }
    if let Some(tid) = filter.tag_id {
        where_clauses.push("EXISTS (SELECT 1 FROM taggings tg WHERE tg.entity_type = 'workflow' AND tg.entity_id = w.id AND tg.tag_id = ?)".to_string());
        params.push(tid.to_string());
    }
    if let Some(ref caps) = filter.required_capabilities {
        where_clauses.push("CAST(w.required_capabilities AS CHAR) LIKE ?".to_string());
        params.push(format!("%{}%", caps));
    }
    if let Some(from) = filter.timeout_ms_from {
        where_clauses.push("w.timeout_ms >= ?".to_string());
        params.push(from.to_string());
    }
    if let Some(to) = filter.timeout_ms_to {
        where_clauses.push("w.timeout_ms <= ?".to_string());
        params.push(to.to_string());
    }
    if let Some(from) = &filter.created_at_from {
        where_clauses.push("w.created_at >= ?".to_string());
        params.push(from.to_string());
    }
    if let Some(to) = &filter.created_at_to {
        where_clauses.push("w.created_at <= ?".to_string());
        params.push(to.to_string());
    }
    if let Some(from) = &filter.updated_at_from {
        where_clauses.push("w.updated_at >= ?".to_string());
        params.push(from.to_string());
    }
    if let Some(to) = &filter.updated_at_to {
        where_clauses.push("w.updated_at <= ?".to_string());
        params.push(to.to_string());
    }

    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };

    let count_sql = format!("SELECT COUNT(*) FROM workflows w {where_sql}");

    let count_sql = audit_sql(count_sql);
    let mut count_q = sqlx::query_as::<_, (i64,)>(count_sql);
    if let Some(ref pattern) = search_pattern {
        count_q = count_q
            .bind(pattern)
            .bind(pattern)
            .bind(pattern)
            .bind(pattern);
    }
    for p in &params {
        count_q = count_q.bind(p);
    }
    let total: (i64,) = count_q
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("workflow count: {e}")))?;

    let list_sql = format!(
        "SELECT w.* FROM workflows w {where_sql} ORDER BY w.created_at DESC LIMIT ? OFFSET ?"
    );

    let list_sql = audit_sql(list_sql);
    let mut q = sqlx::query_as::<_, Workflow>(list_sql);
    if let Some(ref pattern) = search_pattern {
        q = q.bind(pattern).bind(pattern).bind(pattern).bind(pattern);
    }
    for p in &params {
        q = q.bind(p);
    }
    q = q.bind(limit).bind(offset);

    let workflows: Vec<Workflow> = q
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("workflow list: {e}")))?;

    let mut items = Vec::with_capacity(workflows.len());
    for wf in workflows {
        let tags = fetch_tags(pool, wf.id).await?;
        items.push(WorkflowListItem { workflow: wf, tags });
    }

    Ok(WorkflowList {
        items,
        total: total.0,
    })
}

pub async fn update(pool: &MySqlPool, id: i64, meta: UpdateMeta) -> Result<Workflow, AppError> {
    crate::services::optimistic_lock::check_and_bump(
        pool,
        OptimisticLockTable::Workflows,
        id,
        meta.updated_at,
    )
    .await?;
    sqlx::query(
        r#"UPDATE workflows SET
              name = COALESCE(?, name),
              description = COALESCE(?, description),
              timeout_ms = COALESCE(?, timeout_ms),
              category_id = COALESCE(?, category_id),
              required_capabilities = COALESCE(?, required_capabilities)
           WHERE id = ?"#,
    )
    .bind(&meta.name)
    .bind(&meta.description)
    .bind(meta.timeout_ms)
    .bind(meta.category_id)
    .bind(
        meta.required_capabilities
            .as_ref()
            .map(|c| serde_json::to_value(c).unwrap_or(Value::Array(vec![]))),
    )
    .bind(id)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("workflow update: {e}")))?;

    if let Some(ref tag_ids) = meta.tag_ids {
        apply_tags(pool, id, tag_ids).await?;
    }

    fetch_by_id(pool, id).await
}

pub async fn delete(pool: &MySqlPool, id: i64) -> Result<(), AppError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("tx begin: {e}")))?;
    sqlx::query("SELECT id FROM workflows WHERE id = ? FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("workflow lock: {e}")))?;
    // 被 tool kind=2 引用 → 4093
    let cnt: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM tools WHERE workflow_id = ?")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("workflow ref count: {e}")))?;
    if cnt.0 > 0 {
        return Err(AppError::ResourceInUse(format!(
            "workflow 被 {} 个 tool 引用，无法删除",
            cnt.0
        )));
    }
    sqlx::query("DELETE FROM workflows WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("workflow delete: {e}")))?;
    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("tx commit: {e}")))?;
    Ok(())
}

// ============================== Graph GET/PUT ==============================

pub async fn fetch_graph(pool: &MySqlPool, id: i64) -> Result<WorkflowGraph, AppError> {
    let wf = fetch_by_id(pool, id).await?;
    let nodes: Vec<WorkflowNode> =
        sqlx::query_as("SELECT * FROM workflow_nodes WHERE workflow_id = ? ORDER BY id")
            .bind(id)
            .fetch_all(pool)
            .await
            .map_err(|e| AppError::Internal(format!("nodes list: {e}")))?;
    let edges: Vec<WorkflowEdge> =
        sqlx::query_as("SELECT * FROM workflow_edges WHERE workflow_id = ? ORDER BY id")
            .bind(id)
            .fetch_all(pool)
            .await
            .map_err(|e| AppError::Internal(format!("edges list: {e}")))?;

    // db id → node_key 反查
    let id_to_key: HashMap<i64, String> =
        nodes.iter().map(|n| (n.id, n.node_key.clone())).collect();

    // 构造起始节点
    let input_schema = wf
        .input_schema
        .clone()
        .unwrap_or_else(|| serde_json::json!({ "type": "object", "properties": {} }));
    let mut start_position = serde_json::json!({"x": 100, "y": 300});
    if let Some(obj) = start_position.as_object_mut() {
        obj.insert("input_schema".to_string(), input_schema);
    }

    let start_node = GraphNode {
        id: None,
        node_key: "start".to_string(),
        node_type: NodeType::StartNode,
        function_id: None,
        position: Some(start_position),
        node_config: None,
    };

    // 构造结束节点
    let mut end_position = serde_json::json!({"x": 100, "y": 600});
    if let Some(ref schema) = wf.output_schema
        && let Some(obj) = end_position.as_object_mut()
    {
        obj.insert("output_schema".to_string(), schema.clone());
    }

    let end_node = GraphNode {
        id: None,
        node_key: "end".to_string(),
        node_type: NodeType::EndNode,
        function_id: None,
        position: Some(end_position),
        node_config: None,
    };

    let db_nodes: Vec<GraphNode> = nodes
        .into_iter()
        .map(|n| GraphNode {
            id: Some(n.id),
            node_key: n.node_key,
            node_type: n.node_type,
            function_id: n.function_id,
            position: n.position,
            node_config: n.node_config,
        })
        .collect();

    // 起始节点在最前，结束节点在最后
    let mut all_nodes = vec![start_node];
    all_nodes.extend(db_nodes);
    all_nodes.push(end_node);

    // 构建 DB 边列表
    let mut graph_edges: Vec<GraphEdge> = edges
        .into_iter()
        .map(|e| GraphEdge {
            id: Some(e.id),
            src_node_key: id_to_key.get(&e.src_node_id).cloned().unwrap_or_default(),
            dst_node_key: id_to_key.get(&e.dst_node_id).cloned().unwrap_or_default(),
            mapping: e.mapping,
        })
        .collect();

    // 重建连接 start/end 虚拟节点的边
    // - 没有入边的 db 节点 → 从 start 连过来
    // - 没有出边的 db 节点 → 连到 end
    let has_inbound: HashSet<String> = graph_edges.iter().map(|e| e.dst_node_key.clone()).collect();
    let has_outbound: HashSet<String> =
        graph_edges.iter().map(|e| e.src_node_key.clone()).collect();
    let empty_mapping = serde_json::json!({});
    for n in &all_nodes {
        if n.node_key == "start" || n.node_key == "end" {
            continue;
        }
        if !has_inbound.contains(&n.node_key) {
            graph_edges.push(GraphEdge {
                id: None,
                src_node_key: "start".to_string(),
                dst_node_key: n.node_key.clone(),
                mapping: empty_mapping.clone(),
            });
        }
        if !has_outbound.contains(&n.node_key) {
            graph_edges.push(GraphEdge {
                id: None,
                src_node_key: n.node_key.clone(),
                dst_node_key: "end".to_string(),
                mapping: empty_mapping.clone(),
            });
        }
    }

    Ok(WorkflowGraph {
        workflow: wf,
        nodes: all_nodes,
        edges: graph_edges,
    })
}

/// 全量替换 nodes + edges。事务内：
///   1. 校验 node_key 全局唯一（workflow 内）
///   2. 校验 function_id 全部存在
///   3. 检查环 → 5 4092
///   4. 校验 edge mapping
///   5. 聚合 required_capabilities 并更新到 workflows 表
///   6. DELETE 旧 nodes + edges (CASCADE 自动清 edges)
///   7. INSERT 新 nodes + INSERT 新 edges
pub async fn put_graph(
    pool: &MySqlPool,
    id: i64,
    graph: GraphPut,
) -> Result<WorkflowGraph, AppError> {
    // 0. workflow 必须存在
    let wf = fetch_by_id(pool, id).await?;

    // 分离起始节点、结束节点和需要入库的节点
    let start_node = graph
        .nodes
        .iter()
        .find(|n| n.node_type == NodeType::StartNode || n.node_key == "start");
    let start_input_schema = start_node
        .and_then(|n| n.position.as_ref())
        .and_then(|p| p.get("input_schema"))
        .cloned()
        .or_else(|| wf.input_schema.clone())
        .or_else(|| Some(serde_json::json!({ "type": "object", "properties": {} })));
    let start_desc = start_node
        .and_then(|n| n.position.as_ref())
        .and_then(|p| p.get("start_description").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
        .or_else(|| wf.start_description.clone());

    let end_node = graph
        .nodes
        .iter()
        .find(|n| n.node_type == NodeType::EndNode || n.node_key == "end");
    let end_output_schema = end_node
        .and_then(|n| n.position.as_ref())
        .and_then(|p| p.get("output_schema"))
        .cloned()
        .or_else(|| wf.output_schema.clone());

    // 需要入库的节点：function_node + generate_answer_node（start/end 是虚拟节点不入库）
    let db_nodes: Vec<&GraphNode> = graph
        .nodes
        .iter()
        .filter(|n| {
            n.node_type != NodeType::StartNode
                && n.node_key != "start"
                && n.node_type != NodeType::EndNode
                && n.node_key != "end"
        })
        .collect();

    // 过滤掉起始节点和结束节点相关的边
    let db_edges: Vec<&GraphEdge> = graph
        .edges
        .iter()
        .filter(|e| {
            e.src_node_key != "start"
                && e.dst_node_key != "start"
                && e.src_node_key != "end"
                && e.dst_node_key != "end"
        })
        .collect();

    // 1. node_key 唯一性
    let mut keys_seen: HashSet<&str> = HashSet::with_capacity(db_nodes.len());
    for n in &db_nodes {
        if !keys_seen.insert(n.node_key.as_str()) {
            return Err(AppError::BadRequest(format!(
                "node_key 重复：{}",
                n.node_key
            )));
        }
    }

    // 2. function_id 存在性 + 拉取 input_schema/output_schema 供 mapping 校验
    //    只检查 function_node 类型的节点（answer 节点无 function_id）
    let function_ids: Vec<i64> = db_nodes
        .iter()
        .filter(|n| n.node_type == NodeType::FunctionNode)
        .filter_map(|n| n.function_id)
        .collect();
    let function_schemas: HashMap<i64, (Value, Value, Option<Value>)> = if function_ids.is_empty() {
        HashMap::new()
    } else {
        let placeholders = vec!["?"; function_ids.len()].join(",");
        let sql = format!(
            "SELECT id, input_schema, output_schema, required_capabilities FROM functions WHERE id IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (i64, Value, Value, Option<Value>)>(audit_sql(sql));
        for fid in &function_ids {
            q = q.bind(fid);
        }
        let rows = q
            .fetch_all(pool)
            .await
            .map_err(|e| AppError::Internal(format!("function schemas: {e}")))?;
        let found: HashMap<i64, (Value, Value, Option<Value>)> = rows
            .into_iter()
            .map(|(i, a, b, c)| (i, (a, b, c)))
            .collect();
        for fid in &function_ids {
            if !found.contains_key(fid) {
                return Err(AppError::NotFound(format!("function id={fid} not found")));
            }
        }
        found
    };

    // 3. 环检测：所有入库节点（function + answer）参与
    let db_node_keys: HashSet<&str> = db_nodes.iter().map(|n| n.node_key.as_str()).collect();
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for n in &db_nodes {
        adjacency.insert(n.node_key.as_str(), vec![]);
    }
    for e in &db_edges {
        if !db_node_keys.contains(e.src_node_key.as_str()) {
            return Err(AppError::BadRequest(format!(
                "edge src_node_key {} 不在 nodes 内",
                e.src_node_key
            )));
        }
        if !db_node_keys.contains(e.dst_node_key.as_str()) {
            return Err(AppError::BadRequest(format!(
                "edge dst_node_key {} 不在 nodes 内",
                e.dst_node_key
            )));
        }
        adjacency
            .get_mut(e.src_node_key.as_str())
            .unwrap()
            .push(e.dst_node_key.as_str());
    }

    if !db_nodes.is_empty()
        && let Some(cycle) = detect_cycle(&adjacency)
    {
        return Err(AppError::DagCycle(format!(
            "工作流中存在环，请检查节点 {cycle:?} 之间的连线"
        )));
    }

    // 4. mapping 校验：仅检查 function_node 的 required input
    //    answer 节点不引用 function schema，跳过
    //    required 字段是否提供，统一在 4b 走新的结构化 input_mapping

    // 4a. 校验每个节点的 input_mapping（结构化 InputSpec）形态合法
    //     function_node 与 generate_answer_node 都用 input_mapping
    use crate::runtime::input_source::{parse_input_spec, validate_input_spec};
    for n in &db_nodes {
        let spec_key = match n.node_type {
            NodeType::FunctionNode | NodeType::GenerateAnswerNode => "input_mapping",
            _ => continue,
        };
        if let Some(spec_value) = n.node_config.as_ref().and_then(|c| c.get(spec_key)) {
            let spec = parse_input_spec(spec_value).map_err(|e| {
                AppError::WorkflowMappingInvalid(format!(
                    "节点「{}」的 {} 格式错误：{}",
                    n.node_key, spec_key, e
                ))
            })?;
            validate_input_spec(&spec).map_err(|e| {
                AppError::WorkflowMappingInvalid(format!(
                    "节点「{}」的 {} 校验失败：{}",
                    n.node_key, spec_key, e
                ))
            })?;
        }
    }

    // 4b. 校验每个 function_node 的 required input 是否在 input_mapping 中提供
    //     (structured InputSpec 已经在 4a 校验过格式)
    for n in &db_nodes {
        let Some(fid) = n.function_id else {
            continue;
        };
        let Some((input_schema, _output_schema, _caps)) = function_schemas.get(&fid) else {
            continue;
        };
        let required: Vec<&str> = input_schema
            .get("required")
            .and_then(|r| r.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();

        if required.is_empty() {
            continue;
        }

        let spec_map = n
            .node_config
            .as_ref()
            .and_then(|c| c.get("input_mapping"))
            .and_then(|m| m.as_object());

        for req_field in &required {
            let satisfied = spec_map
                .map(|m| m.contains_key(*req_field))
                .unwrap_or(false);
            if !satisfied {
                return Err(AppError::WorkflowMappingInvalid(format!(
                    "节点「{}」的输入映射「{}」无效",
                    n.node_key, req_field
                )));
            }
        }
    }

    // 5. 聚合 required_capabilities（仅 function_node）
    let mut all_caps: HashSet<String> = HashSet::new();
    for n in &db_nodes {
        if let Some(fid) = n.function_id
            && let Some((_, _, Some(caps))) = function_schemas.get(&fid)
            && let Some(arr) = caps.as_array()
        {
            for v in arr {
                if let Some(s) = v.as_str() {
                    all_caps.insert(s.to_string());
                }
            }
        }
    }
    let workflow_caps: Option<Value> = if all_caps.is_empty() {
        None
    } else {
        let mut caps_vec: Vec<String> = all_caps.into_iter().collect();
        caps_vec.sort();
        Some(Value::Array(
            caps_vec.into_iter().map(Value::String).collect(),
        ))
    };

    // 6+7. 事务内重建
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("tx begin: {e}")))?;

    // 更新 workflow 的 required_capabilities, input_schema, output_schema
    sqlx::query(
        "UPDATE workflows SET required_capabilities = ?, input_schema = ?, start_description = ?, output_schema = ? WHERE id = ?"
    )
    .bind(&workflow_caps)
    .bind(&start_input_schema)
    .bind(&start_desc)
    .bind(&end_output_schema)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(|e| AppError::Internal(format!("workflow capabilities update: {e}")))?;

    sqlx::query("DELETE FROM workflow_edges WHERE workflow_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("edges clear: {e}")))?;
    sqlx::query("DELETE FROM workflow_nodes WHERE workflow_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("nodes clear: {e}")))?;

    let mut key_to_id: HashMap<&str, i64> = HashMap::with_capacity(db_nodes.len());
    for n in &db_nodes {
        let nt_str = node_type_to_db_str(n.node_type);
        let res = sqlx::query(
            "INSERT INTO workflow_nodes (workflow_id, node_key, function_id, position, node_type, node_config) VALUES (?, ?, ?, ?, ?, ?)"
        )
        .bind(id)
        .bind(&n.node_key)
        .bind(n.function_id)
        .bind(&n.position)
        .bind(nt_str)
        .bind(&n.node_config)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("node insert {}: {e}", n.node_key)))?;
        key_to_id.insert(n.node_key.as_str(), res.last_insert_id() as i64);
    }
    for e in &db_edges {
        let src = *key_to_id.get(e.src_node_key.as_str()).unwrap();
        let dst = *key_to_id.get(e.dst_node_key.as_str()).unwrap();
        sqlx::query(
            "INSERT INTO workflow_edges (workflow_id, src_node_id, dst_node_id, mapping) VALUES (?, ?, ?, ?)"
        )
        .bind(id)
        .bind(src)
        .bind(dst)
        .bind(&e.mapping)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("edge insert: {e}")))?;
    }

    tx.commit()
        .await
        .map_err(|e| AppError::Internal(format!("tx commit: {e}")))?;

    fetch_graph(pool, id).await
}

/// DFS 环检测。返回环上节点 key 列表（任一）；无环则返回 None。
fn detect_cycle<'a>(adj: &HashMap<&'a str, Vec<&'a str>>) -> Option<Vec<&'a str>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Color {
        White,
        Gray,
        Black,
    }
    let mut color: HashMap<&str, Color> = adj.keys().map(|&k| (k, Color::White)).collect();
    let mut stack: Vec<(&str, Vec<&str>)> = Vec::new();

    for &start in adj.keys() {
        if color[start] != Color::White {
            continue;
        }
        stack.push((start, vec![start]));
        color.insert(start, Color::Gray);
        while let Some((node, path)) = stack.last().cloned() {
            let Some(succs) = adj.get(node) else {
                color.insert(node, Color::Black);
                stack.pop();
                continue;
            };
            let mut advanced = false;
            for &next in succs {
                match color.get(next).copied().unwrap_or(Color::White) {
                    Color::White => {
                        color.insert(next, Color::Gray);
                        let mut np = path.clone();
                        np.push(next);
                        stack.push((next, np));
                        advanced = true;
                        break;
                    }
                    Color::Gray => {
                        // 找到环；返回从 next 开始的环段
                        let mut cycle: Vec<&str> =
                            path.iter().skip_while(|&&p| p != next).copied().collect();
                        cycle.push(next);
                        return Some(cycle);
                    }
                    Color::Black => {}
                }
            }
            if !advanced {
                color.insert(node, Color::Black);
                stack.pop();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_to_str(
        adj: &[(&'static str, &[&'static str])],
    ) -> HashMap<&'static str, Vec<&'static str>> {
        adj.iter().map(|(k, v)| (*k, v.to_vec())).collect()
    }

    #[test]
    fn no_cycle() {
        // A → B → D, A → C → D
        let adj = key_to_str(&[("A", &["B", "C"]), ("B", &["D"]), ("C", &["D"]), ("D", &[])]);
        assert!(detect_cycle(&adj).is_none());
    }

    #[test]
    fn simple_cycle() {
        // A → B → C → A
        let adj = key_to_str(&[("A", &["B"]), ("B", &["C"]), ("C", &["A"])]);
        let cycle = detect_cycle(&adj).expect("cycle should be detected");
        assert!(cycle.contains(&"A"));
        assert!(cycle.contains(&"B"));
        assert!(cycle.contains(&"C"));
    }

    #[test]
    fn self_loop() {
        let adj = key_to_str(&[("X", &["X"])]);
        assert!(detect_cycle(&adj).is_some());
    }
}
