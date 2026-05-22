/// Structured progress-event helpers shared by agent runtimes.

use serde_json::{Map, Value};

/// TODO: Hook context from the agent crate.
pub trait TODO_AgentHookContext: Send + Sync {
    fn tool_calls(&self) -> Vec<&dyn TODO_ToolCall>;
    fn tool_results(&self) -> Vec<Value>;
    fn tool_events(&self) -> Vec<Value>;
}

/// TODO: Tool call trait.
pub trait TODO_ToolCall: Send + Sync {
    fn id(&self) -> Option<&str>;
    fn name(&self) -> Option<&str>;
    fn arguments(&self) -> Option<Map<String, Value>>;
}

/// Progress callback type.
pub type ProgressCallback = Box<
    dyn Fn(
        String,
        bool,
        Option<Vec<Map<String, Value>>>,
        Option<Vec<Map<String, Value>>>,
        bool,
        bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    + Send,
>;

/// Check if an on_progress callback accepts `tool_events` parameter.
pub fn on_progress_accepts_tool_events() -> bool {
    true
}

/// Check if an on_progress callback accepts `file_edit_events` parameter.
pub fn on_progress_accepts_file_edit_events() -> bool {
    true
}

/// Invoke the on_progress callback with tool events if applicable.
pub async fn invoke_on_progress(
    on_progress: &dyn Fn(
        String,
        bool,
        Option<Vec<Map<String, Value>>>,
        Option<Vec<Map<String, Value>>>,
        bool,
        bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
    content: String,
    tool_hint: bool,
    tool_events: Option<Vec<Map<String, Value>>>,
) {
    if tool_events.is_some() && on_progress_accepts_tool_events() {
        on_progress(content, tool_hint, tool_events, None, false, false).await;
        return;
    }
    on_progress(content, tool_hint, None, None, false, false).await;
}

/// Invoke the on_progress callback with file edit events.
pub async fn invoke_file_edit_progress(
    on_progress: &dyn Fn(
        String,
        bool,
        Option<Vec<Map<String, Value>>>,
        Option<Vec<Map<String, Value>>>,
        bool,
        bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
    file_edit_events: Vec<Map<String, Value>>,
) {
    if file_edit_events.is_empty() || !on_progress_accepts_file_edit_events() {
        return;
    }
    on_progress(
        String::new(),
        false,
        None,
        Some(file_edit_events),
        false,
        false,
    )
    .await;
}

/// Build a tool event start payload.
pub fn build_tool_event_start_payload(tool_call: &dyn TODO_ToolCall) -> Map<String, Value> {
    [
        ("version".to_string(), Value::Number(1.into())),
        ("phase".to_string(), Value::String("start".to_string())),
        (
            "call_id".to_string(),
            Value::String(tool_call.id().unwrap_or("").to_string()),
        ),
        (
            "name".to_string(),
            Value::String(tool_call.name().unwrap_or("").to_string()),
        ),
        (
            "arguments".to_string(),
            Value::Object(tool_call.arguments().unwrap_or_default()),
        ),
        ("result".to_string(), Value::Null),
        ("error".to_string(), Value::Null),
        ("files".to_string(), Value::Array(vec![])),
        ("embeds".to_string(), Value::Array(vec![])),
    ]
    .into_iter()
    .collect()
}

/// Extract `files` and `embeds` from a tool result.
pub fn tool_event_result_extras(result: &Value) -> (Vec<Value>, Vec<Value>) {
    let Some(obj) = result.as_object() else {
        return (vec![], vec![]);
    };
    let files = obj
        .get("files")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let embeds = obj
        .get("embeds")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    (files, embeds)
}

/// Build tool event finish payloads from an agent hook context.
pub fn build_tool_event_finish_payloads(
    context: &dyn TODO_AgentHookContext,
) -> Vec<Map<String, Value>> {
    let mut payloads = Vec::new();
    let count = context.tool_calls().len().min(
        context.tool_results().len().min(context.tool_events().len()),
    );

    for idx in 0..count {
        let tool_call = context.tool_calls()[idx];
        let result = &context.tool_results()[idx];
        let event = &context.tool_events()[idx];

        let status = event.get("status").and_then(|v| v.as_str()).unwrap_or("");
        let phase = if status == "ok" { "end" } else { "error" };

        let (files, embeds) = tool_event_result_extras(result);

        let mut payload: Map<String, Value> = [
            ("version".to_string(), Value::Number(1.into())),
            ("phase".to_string(), Value::String(phase.to_string())),
            (
                "call_id".to_string(),
                Value::String(tool_call.id().unwrap_or("").to_string()),
            ),
            (
                "name".to_string(),
                Value::String(tool_call.name().unwrap_or("").to_string()),
            ),
            (
                "arguments".to_string(),
                Value::Object(tool_call.arguments().unwrap_or_default()),
            ),
            (
                "result".to_string(),
                if phase == "end" {
                    result.clone()
                } else {
                    Value::Null
                },
            ),
            ("error".to_string(), Value::Null),
            ("files".to_string(), Value::Array(files)),
            ("embeds".to_string(), Value::Array(embeds)),
        ]
        .into_iter()
        .collect();

        if phase == "error" {
            let error = if let Some(s) = result.as_str() {
                let s = s.trim();
                if !s.is_empty() {
                    s.to_string()
                } else {
                    event
                        .get("detail")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Tool execution failed")
                        .to_string()
                }
            } else {
                event
                    .get("detail")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Tool execution failed")
                    .to_string()
            };
            payload.insert("error".to_string(), Value::String(error));
        }

        payloads.push(payload);
    }

    payloads
}
