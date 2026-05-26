//! Agent-facing interface exposed by the API server.
//!
//! The HTTP server never touches the concrete `AgentLoop`; instead it
//! drives any type that implements [`ApiAgent`]. The CLI crate supplies
//! a real adapter over `agent::AgentLoop`, but tests can hand-roll a
//! trivial mock.

use std::path::PathBuf;

use async_trait::async_trait;

/// Parsed request ready to be executed by an agent backend.
#[derive(Debug, Clone)]
pub struct ApiRequest {
    /// Raw user text, already flattened from JSON content blocks.
    pub content: String,
    /// Media files saved to disk during parsing (absolute paths).
    pub media: Vec<PathBuf>,
    /// Optional session identifier. Defaults to `default` when absent.
    pub session_id: Option<String>,
    /// Requested model. The server validates this against its configured
    /// `model_name` before delegating, so the agent can ignore it.
    pub model: Option<String>,
    /// Routing info for session-key scoping. Defaults mimic Python:
    /// `channel = "api"`, `chat_id = "default"`.
    pub channel: String,
    pub chat_id: String,
}

impl ApiRequest {
    pub fn session_key(&self) -> String {
        match self.session_id.as_deref() {
            Some(id) if !id.is_empty() => format!("api:{id}"),
            _ => "api:default".to_string(),
        }
    }
}

/// Terminal assistant answer. No tool events / usage here — those are
/// internal agent concerns.
#[derive(Debug, Clone, Default)]
pub struct ApiAnswer {
    pub content: String,
}

/// Asynchronous interface used by the HTTP handlers.
///
/// Implementations should be cheap to clone or wrap in `Arc` — the
/// server stores a single `Arc<dyn ApiAgent>` and reuses it across
/// requests.
#[async_trait]
pub trait ApiAgent: Send + Sync {
    /// Run a single non-streaming request.
    async fn generate(&self, req: ApiRequest) -> Result<ApiAnswer, String>;

    /// Optional streaming entry-point. The default implementation falls
    /// back to [`Self::generate`] and emits the full answer as one
    /// delta — matching the Python API shape when the underlying runner
    /// doesn't yet expose per-token callbacks.
    ///
    /// `on_delta` receives each token as it arrives. We pass it by
    /// `&mut` so implementations can mutate captured state.
    async fn generate_stream(
        &self,
        req: ApiRequest,
        on_delta: &mut dyn StreamSink,
    ) -> Result<ApiAnswer, String> {
        let answer = self.generate(req).await?;
        if !answer.content.is_empty() {
            on_delta.on_delta(&answer.content);
        }
        Ok(answer)
    }
}

/// Trait alias for streaming sinks. Use this instead of `FnMut(&str)`
/// in async trait methods to avoid HRTB lifetime inference gotchas.
pub trait StreamSink: Send {
    fn on_delta(&mut self, delta: &str);
}

impl<F: FnMut(&str) + Send> StreamSink for F {
    fn on_delta(&mut self, delta: &str) {
        self(delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    #[async_trait]
    impl ApiAgent for Echo {
        async fn generate(&self, req: ApiRequest) -> Result<ApiAnswer, String> {
            Ok(ApiAnswer {
                content: format!("echo: {}", req.content),
            })
        }
    }

    #[test]
    fn session_key_defaults_when_unset() {
        let req = ApiRequest {
            content: "".into(),
            media: vec![],
            session_id: None,
            model: None,
            channel: "api".into(),
            chat_id: "default".into(),
        };
        assert_eq!(req.session_key(), "api:default");
        let with_id = ApiRequest {
            session_id: Some("abc".into()),
            ..req
        };
        assert_eq!(with_id.session_key(), "api:abc");
    }

    #[tokio::test]
    async fn default_stream_replays_full_answer() {
        struct Collector(Vec<String>);
        impl StreamSink for Collector {
            fn on_delta(&mut self, d: &str) {
                self.0.push(d.to_string());
            }
        }
        let agent = Echo;
        let mut sink = Collector(Vec::new());
        let req = ApiRequest {
            content: "hi".into(),
            media: vec![],
            session_id: None,
            model: None,
            channel: "api".into(),
            chat_id: "default".into(),
        };
        let ans = agent.generate_stream(req, &mut sink).await.unwrap();
        assert_eq!(ans.content, "echo: hi");
        assert_eq!(sink.0, vec!["echo: hi".to_string()]);
    }
}
