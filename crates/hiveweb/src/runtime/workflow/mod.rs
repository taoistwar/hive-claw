//! Workflow DAG 执行器（research §5 / FR-013 / US3 / T110）
//!
//! 拓扑序 BFS 分层 + `tokio::join_all` 并行同层节点。环检测在 service 层
//! `services::workflow::put_graph` 保存时已禁止（spec FR-014），运行时不再校验。
//!
//! mapping 解析：每个 edge 的 `mapping = {"dst.input.<field>": "<src_node_key>.output.<path>"}`
//! 路径 `<path>` 支持简单 dot-path (e.g. `temp_c` 或 `nested.field`)。
mod generate_answer_node;
mod function_node;
mod node_executor;
mod workflow_executor;
pub use workflow_executor::{ExecuteOutcome, WorkflowExecutor, ExecutorDeps, WorkflowError};
