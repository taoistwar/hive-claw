/// Structured progress-event helpers shared by agent runtimes.
use serde_json::{Map, Value};

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
