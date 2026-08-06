use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Workflow {
    pub id: i64,
    pub identifier: String,
    pub name: String,
    pub description: Option<String>,
    pub timeout_ms: i32,
    pub category_id: Option<i64>,
    pub input_schema: Option<serde_json::Value>,
    pub start_description: Option<String>,
    pub output_schema: Option<serde_json::Value>,
    pub end_description: Option<String>,
    pub required_capabilities: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    FunctionNode,
    StartNode,
    EndNode,
    GenerateAnswerNode,
}

// 手动实现 sqlx::Encode for NodeType
impl<'q> sqlx::Encode<'q, sqlx::MySql> for NodeType {
    fn encode_by_ref(
        &self,
        buf: &mut <sqlx::MySql as sqlx::Database>::ArgumentBuffer<'q>,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        let s = match self {
            NodeType::FunctionNode => "function_node",
            NodeType::StartNode => "start_node",
            NodeType::EndNode => "end_node",
            NodeType::GenerateAnswerNode => "generate_answer_node",
        };
        <&str as sqlx::Encode<sqlx::MySql>>::encode(s, buf)
    }
}

// 手动实现 sqlx::Decode for NodeType
impl<'r> sqlx::Decode<'r, sqlx::MySql> for NodeType {
    fn decode(
        value: <sqlx::MySql as sqlx::Database>::ValueRef<'r>,
    ) -> Result<Self, sqlx::error::BoxDynError> {
        let s = <&str as sqlx::Decode<sqlx::MySql>>::decode(value)?;
        match s {
            "function_node" => Ok(NodeType::FunctionNode),
            "start_node" => Ok(NodeType::StartNode),
            "end_node" => Ok(NodeType::EndNode),
            "generate_answer_node" => Ok(NodeType::GenerateAnswerNode),
            _ => Err(format!("invalid node_type: {s}").into()),
        }
    }
}

// 手动实现 sqlx::Type for NodeType
impl sqlx::Type<sqlx::MySql> for NodeType {
    fn type_info() -> sqlx::mysql::MySqlTypeInfo {
        <&str as sqlx::Type<sqlx::MySql>>::type_info()
    }

    fn compatible(ty: &sqlx::mysql::MySqlTypeInfo) -> bool {
        <&str as sqlx::Type<sqlx::MySql>>::compatible(ty)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct WorkflowNode {
    pub id: i64,
    pub workflow_id: i64,
    pub node_key: String,
    pub function_id: Option<i64>,
    pub node_type: NodeType,
    pub position: Option<serde_json::Value>,
    pub node_config: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct WorkflowEdge {
    pub id: i64,
    pub workflow_id: i64,
    pub src_node_id: i64,
    pub dst_node_id: i64,
    pub mapping: serde_json::Value,
}
