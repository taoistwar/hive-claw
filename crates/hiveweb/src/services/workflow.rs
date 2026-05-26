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

use crate::models::{Workflow, WorkflowEdge, WorkflowNode};
use crate::utils::error::AppError;

#[derive(Debug, Deserialize)]
pub struct CreateMeta {
    pub identifier: String,
    pub name: String,
    pub description: Option<String>,
    pub timeout_ms: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMeta {
    pub name: Option<String>,
    pub description: Option<String>,
    pub timeout_ms: Option<i32>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowList {
    pub items: Vec<Workflow>,
    pub total: i64,
}

/// 客户端提交 / 服务端返回的图结构（无 id；由 server 重新分配）
#[derive(Debug, Deserialize, Serialize)]
pub struct GraphNode {
    /// 服务端返回时携带；客户端 PUT 时忽略
    #[serde(default)]
    pub id: Option<i64>,
    pub node_key: String,
    pub function_id: i64,
    #[serde(default)]
    pub position: Option<Value>,
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
    let res = sqlx::query(
        "INSERT INTO workflows (identifier, name, description, timeout_ms) VALUES (?, ?, ?, ?)",
    )
    .bind(&meta.identifier)
    .bind(&meta.name)
    .bind(&meta.description)
    .bind(meta.timeout_ms.unwrap_or(30000))
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
    fetch_by_id(pool, res.last_insert_id() as i64).await
}

pub async fn fetch_by_id(pool: &MySqlPool, id: i64) -> Result<Workflow, AppError> {
    sqlx::query_as::<_, Workflow>("SELECT * FROM workflows WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("workflow fetch: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("workflow id={id} not found")))
}

pub async fn list(pool: &MySqlPool, offset: i64, limit: i64) -> Result<WorkflowList, AppError> {
    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM workflows")
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("workflow count: {e}")))?;
    let items: Vec<Workflow> =
        sqlx::query_as("SELECT * FROM workflows ORDER BY created_at DESC LIMIT ? OFFSET ?")
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await
            .map_err(|e| AppError::Internal(format!("workflow list: {e}")))?;
    Ok(WorkflowList { items, total: total.0 })
}

pub async fn update(pool: &MySqlPool, id: i64, meta: UpdateMeta) -> Result<Workflow, AppError> {
    crate::services::optimistic_lock::check_and_bump(pool, "workflows", id, meta.updated_at).await?;
    sqlx::query(
        r#"UPDATE workflows SET
              name = COALESCE(?, name),
              description = COALESCE(?, description),
              timeout_ms = COALESCE(?, timeout_ms)
           WHERE id = ?"#,
    )
    .bind(&meta.name)
    .bind(&meta.description)
    .bind(meta.timeout_ms)
    .bind(id)
    .execute(pool)
    .await
    .map_err(|e| AppError::Internal(format!("workflow update: {e}")))?;
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
    let cnt: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM tools WHERE workflow_id = ?")
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
    let nodes: Vec<WorkflowNode> = sqlx::query_as(
        "SELECT * FROM workflow_nodes WHERE workflow_id = ? ORDER BY id",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("nodes list: {e}")))?;
    let edges: Vec<WorkflowEdge> = sqlx::query_as(
        "SELECT * FROM workflow_edges WHERE workflow_id = ? ORDER BY id",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("edges list: {e}")))?;

    // db id → node_key 反查
    let id_to_key: HashMap<i64, String> =
        nodes.iter().map(|n| (n.id, n.node_key.clone())).collect();

    Ok(WorkflowGraph {
        workflow: wf,
        nodes: nodes
            .into_iter()
            .map(|n| GraphNode {
                id: Some(n.id),
                node_key: n.node_key,
                function_id: n.function_id,
                position: n.position,
            })
            .collect(),
        edges: edges
            .into_iter()
            .map(|e| GraphEdge {
                id: Some(e.id),
                src_node_key: id_to_key.get(&e.src_node_id).cloned().unwrap_or_default(),
                dst_node_key: id_to_key.get(&e.dst_node_id).cloned().unwrap_or_default(),
                mapping: e.mapping,
            })
            .collect(),
    })
}

/// 全量替换 nodes + edges。事务内：
///   1. 校验 node_key 全局唯一（workflow 内）
///   2. 校验 function_id 全部存在
///   3. 检查环 → 5 4092
///   4. 校验 edge mapping
///   5. DELETE 旧 nodes + edges (CASCADE 自动清 edges)
///   6. INSERT 新 nodes + INSERT 新 edges
pub async fn put_graph(
    pool: &MySqlPool,
    id: i64,
    graph: GraphPut,
) -> Result<WorkflowGraph, AppError> {
    // 0. workflow 必须存在
    fetch_by_id(pool, id).await?;

    // 1. node_key 唯一性
    let mut keys_seen: HashSet<&str> = HashSet::with_capacity(graph.nodes.len());
    for n in &graph.nodes {
        if !keys_seen.insert(n.node_key.as_str()) {
            return Err(AppError::BadRequest(format!(
                "node_key 重复：{}",
                n.node_key
            )));
        }
    }

    // 2. function_id 存在性 + 拉取 input_schema/output_schema 供 mapping 校验
    let function_ids: Vec<i64> = graph.nodes.iter().map(|n| n.function_id).collect();
    if function_ids.is_empty() && !graph.edges.is_empty() {
        return Err(AppError::BadRequest("空 graph 不能有 edges".into()));
    }
    let function_schemas: HashMap<i64, (Value, Value)> = if function_ids.is_empty() {
        HashMap::new()
    } else {
        let placeholders = vec!["?"; function_ids.len()].join(",");
        let sql = format!(
            "SELECT id, input_schema, output_schema FROM functions WHERE id IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (i64, Value, Value)>(&sql);
        for fid in &function_ids {
            q = q.bind(fid);
        }
        let rows = q
            .fetch_all(pool)
            .await
            .map_err(|e| AppError::Internal(format!("function schemas: {e}")))?;
        let found: HashMap<i64, (Value, Value)> =
            rows.into_iter().map(|(i, a, b)| (i, (a, b))).collect();
        for fid in &function_ids {
            if !found.contains_key(fid) {
                return Err(AppError::NotFound(format!("function id={fid} not found")));
            }
        }
        found
    };

    // 3. 环检测：DFS white/gray/black
    let key_to_function: HashMap<&str, i64> = graph
        .nodes
        .iter()
        .map(|n| (n.node_key.as_str(), n.function_id))
        .collect();
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for n in &graph.nodes {
        adjacency.insert(n.node_key.as_str(), vec![]);
    }
    for e in &graph.edges {
        if !key_to_function.contains_key(e.src_node_key.as_str()) {
            return Err(AppError::BadRequest(format!(
                "edge src_node_key {} 不在 nodes 内",
                e.src_node_key
            )));
        }
        if !key_to_function.contains_key(e.dst_node_key.as_str()) {
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

    if let Some(cycle) = detect_cycle(&adjacency) {
        return Err(AppError::DagCycle(format!(
            "工作流中存在环，请检查节点 {cycle:?} 之间的连线"
        )));
    }

    // 4. mapping 校验：dst Function 的 required input 都有 mapping
    //    mapping: { "dst.input.<field>": "<src_node_key>.output.<path>" }
    let mut inbound_by_node: HashMap<&str, Vec<&serde_json::Map<String, Value>>> = HashMap::new();
    for e in &graph.edges {
        if let Some(m) = e.mapping.as_object() {
            inbound_by_node.entry(e.dst_node_key.as_str()).or_default().push(m);
        }
    }
    for n in &graph.nodes {
        let Some((input_schema, _output_schema)) = function_schemas.get(&n.function_id) else {
            continue;
        };
        let required: Vec<&str> = input_schema
            .get("required")
            .and_then(|r| r.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        let inbound = inbound_by_node.get(n.node_key.as_str());
        for req_field in &required {
            let key = format!("dst.input.{req_field}");
            let satisfied = inbound
                .map(|maps| {
                    maps.iter()
                        .any(|m| m.contains_key(&key) || m.contains_key(*req_field))
                })
                .unwrap_or(false);
            // 入口节点（无上游）可由外部 input 提供；只对有上游的节点检
            if !satisfied && inbound.is_some() {
                return Err(AppError::WorkflowMappingInvalid(format!(
                    "节点「{}」的输入映射「{}」无效",
                    n.node_key, req_field
                )));
            }
        }
    }

    // 5+6. 事务内重建
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::Internal(format!("tx begin: {e}")))?;
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

    let mut key_to_id: HashMap<&str, i64> = HashMap::with_capacity(graph.nodes.len());
    for n in &graph.nodes {
        let res = sqlx::query(
            "INSERT INTO workflow_nodes (workflow_id, node_key, function_id, position) VALUES (?, ?, ?, ?)"
        )
        .bind(id)
        .bind(&n.node_key)
        .bind(n.function_id)
        .bind(&n.position)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Internal(format!("node insert {}: {e}", n.node_key)))?;
        key_to_id.insert(n.node_key.as_str(), res.last_insert_id() as i64);
    }
    for e in &graph.edges {
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
                        let mut cycle: Vec<&str> = path
                            .iter()
                            .skip_while(|&&p| p != next)
                            .copied()
                            .collect();
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

    fn key_to_str(adj: &[(&'static str, &[&'static str])]) -> HashMap<&'static str, Vec<&'static str>> {
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
