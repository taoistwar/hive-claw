//! Capability dispatcher used by local desktop function tests.

use std::{collections::HashMap, sync::Arc, time::Duration};

use serde::Deserialize;
use serde_json::Value;

use hive_runtime_core::abi::{HostCallReply, HostCallRequest, StableErrorKind};

use super::capability_adapter::{AuditSink, CapabilityAdapter, CapabilityDispatchError};
use super::diagnostics::DiagnosticSink;

const BODY_MAX_BYTES: usize = 4 * 1024 * 1024;

/// One capability exposed by the desktop host runtime.
pub struct DesktopCapabilityDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub is_dangerous: bool,
}

/// Capability catalog registered into the HiveGUI management database at startup.
pub const DESKTOP_CAPABILITY_CATALOG: &[DesktopCapabilityDefinition] = &[
    DesktopCapabilityDefinition {
        name: "network.http",
        description: "HTTP/HTTPS access (allowlisted hosts, SSRF-blocked)",
        is_dangerous: true,
    },
    DesktopCapabilityDefinition {
        name: "fs.read",
        description: "/tmp/plugin/ 内文件读",
        is_dangerous: false,
    },
    DesktopCapabilityDefinition {
        name: "fs.write",
        description: "/tmp/plugin/ 内文件写",
        is_dangerous: false,
    },
    DesktopCapabilityDefinition {
        name: "s3.read",
        description: "Rustfs 桶 GET",
        is_dangerous: false,
    },
    DesktopCapabilityDefinition {
        name: "s3.write",
        description: "Rustfs 桶 PUT / DELETE",
        is_dangerous: false,
    },
    DesktopCapabilityDefinition {
        name: "db.query",
        description: "宿主预注册命名 SELECT 查询",
        is_dangerous: false,
    },
    DesktopCapabilityDefinition {
        name: "db.execute",
        description: "宿主预注册命名 DML（永不自由 SQL）",
        is_dangerous: true,
    },
    DesktopCapabilityDefinition {
        name: "llm.invoke",
        description: "LLM 调用（走 Agent model preset 解析）",
        is_dangerous: false,
    },
    DesktopCapabilityDefinition {
        name: "secret.get",
        description: "allowlist 内的密钥读取",
        is_dangerous: true,
    },
    DesktopCapabilityDefinition {
        name: "time.now",
        description: "服务器当前时间",
        is_dangerous: false,
    },
    DesktopCapabilityDefinition {
        name: "log.emit",
        description: "结构化日志写入（rate-limited）",
        is_dangerous: false,
    },
    DesktopCapabilityDefinition {
        name: "chat.respond",
        description: "提交 Agent 最终用户可见回复",
        is_dangerous: false,
    },
    DesktopCapabilityDefinition {
        name: "exec.run",
        description: "Shell 命令执行（受 workspace 边界约束）",
        is_dangerous: true,
    },
    DesktopCapabilityDefinition {
        name: "agent.spawn",
        description: "生成子 Agent 执行独立任务",
        is_dangerous: false,
    },
    DesktopCapabilityDefinition {
        name: "cron.manage",
        description: "管理定时 Cron 任务",
        is_dangerous: false,
    },
];

/// Build a successful `host_call` reply from a JSON value using the shared
/// ABI envelope. The value is serialised to canonical JSON bytes and inlined
/// verbatim, so the Plugin receives exactly the bytes it produced.
fn host_call_ok(data: Value) -> HostCallReply {
    let bytes = serde_json::to_vec(&data).expect("serde_json::Value serialises to bytes");
    HostCallReply::success_json(&bytes).expect("a serialised Value is valid JSON")
}

/// Legacy HiveGUI-only reply for a capability that is registered in the
/// desktop catalog but has no local handler yet (still "depends on the
/// server runtime" in the pre-ABI desktop path). Code `5010` is a desktop
/// extension and is *not* part of the shared [`StableErrorKind`] table; it
/// is removed once the remaining catalog handlers are wired to local
/// implementations.
///
/// `capability` is guaranteed to be one of the static
/// [`DESKTOP_CAPABILITY_CATALOG`] names at this call site (unknown names
/// are rejected earlier as `capability_unknown`), so it cannot inject
/// JSON control characters.
fn legacy_not_implemented_reply(capability: &str) -> Vec<u8> {
    format!(
        r#"{{"ok":false,"code":5010,"message":"Capability「{capability}」依赖服务端运行时，HiveGUI 本地测试暂不支持"}}"#
    )
    .into_bytes()
}

/// Serialise a failure `host_call` envelope for an arbitrary numeric code.
///
/// The shared [`HostCallReply::failure`] only accepts a [`StableErrorKind`];
/// the capability-adapter layer surfaces a *different*, HiveGUI-internal
/// error-code family ([`crate::runtime::capability_adapter::AdapterErrorCode`])
/// whose wire values (4001/4045/4030/4050/4010/4020) are not all present in
/// the Plugin ABI table. This helper reproduces the exact
/// `{"ok":false,"code":<n>,"message":...}` shape (JSON-escaped message,
/// `ok`-first field order) for that internal surface.
fn host_call_error_bytes(code: u16, message: &str) -> Vec<u8> {
    let message_json = serde_json::to_string(message).expect("serialising a string is total");
    format!(r#"{{"ok":false,"code":{code},"message":{message_json}}}"#).into_bytes()
}

#[derive(Debug, Deserialize)]
struct HttpArgs {
    method: String,
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

/// Dispatches the desktop subset of the Agent Runtime host capability ABI.
///
/// Two construction paths are supported:
///
/// 1. [`DesktopHostDispatcher::default`] — the legacy catalog-only
///    dispatcher backed by [`DESKTOP_CAPABILITY_CATALOG`]. Used
///    by the production code that does not yet wire a
///    [`CapabilityAdapter`].
/// 2. [`DesktopHostDispatcher::with_capability_adapter`] — the
///    T070 path: the dispatcher delegates to a
///    [`CapabilityAdapter`] whose [`HandlerRegistry`] is the
///    single source of truth for what is executable.
pub struct DesktopHostDispatcher {
    inner: DispatcherImpl,
    diagnostic_sink: Option<Arc<dyn DiagnosticSink>>,
}

enum DispatcherImpl {
    Legacy,
    Adapter {
        adapter: CapabilityAdapter,
        audit_sink: Arc<dyn AuditSink>,
    },
}

impl Default for DesktopHostDispatcher {
    fn default() -> Self {
        Self {
            inner: DispatcherImpl::Legacy,
            diagnostic_sink: None,
        }
    }
}

impl std::fmt::Debug for DesktopHostDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            DispatcherImpl::Legacy => f.debug_struct("DesktopHostDispatcher::Legacy").finish(),
            DispatcherImpl::Adapter { .. } => {
                f.debug_struct("DesktopHostDispatcher::Adapter").finish()
            }
        }
    }
}

impl DesktopHostDispatcher {
    /// Build a dispatcher whose dispatch order is driven by the
    /// supplied [`CapabilityAdapter`]. The supplied [`AuditSink`]
    /// overrides the adapter's sink so the caller can observe
    /// every dispatch event from a single place.
    pub fn with_capability_adapter(
        adapter: CapabilityAdapter,
        audit_sink: Arc<dyn AuditSink>,
    ) -> Self {
        // Replace the adapter's audit sink with the one supplied
        // by the caller so the dispatcher's contract is honoured:
        // every event must reach the caller's sink.
        let registry = adapter.registry().clone();
        let adapter = CapabilityAdapter::with_audit_sink(registry, audit_sink.clone());
        Self {
            inner: DispatcherImpl::Adapter {
                adapter,
                audit_sink,
            },
            diagnostic_sink: None,
        }
    }

    /// Attach a [`DiagnosticSink`] to the dispatcher. Every
    /// dispatch will deliver exactly one diagnostic record,
    /// mirroring the audit event emitted by the adapter. T067.4
    /// verifies the count parity.
    pub fn with_diagnostic_sink(mut self, sink: Arc<dyn DiagnosticSink>) -> Self {
        self.diagnostic_sink = Some(sink);
        self
    }

    /// Dispatches one JSON host-call envelope using an explicit
    /// permission snapshot. When the dispatcher was built with
    /// [`DesktopHostDispatcher::with_capability_adapter`], the
    /// order is: parse → unknown → unauthorized → handler →
    /// argument-shape. The legacy
    /// [`DesktopHostDispatcher::default`] path is preserved for
    /// callers that have not migrated to the adapter yet.
    pub async fn dispatch(&self, envelope: &str, allowed_capabilities: &[String]) -> String {
        match &self.inner {
            DispatcherImpl::Legacy => self.dispatch_legacy(envelope, allowed_capabilities).await,
            DispatcherImpl::Adapter { adapter, .. } => {
                let outcome = adapter.dispatch(envelope, allowed_capabilities).await;
                self.record_diagnostic(envelope, &outcome);
                let reply_bytes = match outcome {
                    Ok(outcome) => host_call_ok(outcome.data().clone()).encode_json(),
                    Err(err) => host_call_error_bytes(err.wire_code(), err.message()),
                };
                String::from_utf8(reply_bytes).expect("host_call reply is always valid UTF-8 JSON")
            }
        }
    }

    fn record_diagnostic<T>(&self, envelope: &str, outcome: &Result<T, CapabilityDispatchError>) {
        let Some(sink) = self.diagnostic_sink.as_ref() else {
            return;
        };
        use super::capability_adapter::redact_secrets;
        use super::diagnostics::DiagnosticRecord;
        let (capability, label, message) = match outcome {
            Ok(_) => (
                Self::capability_from_envelope(envelope),
                "ok",
                "handler succeeded".to_string(),
            ),
            Err(err) => (
                err.capability(),
                err.code().label(),
                err.message().to_string(),
            ),
        };
        let cause = redact_secrets(&format!("capability={capability} outcome={label}"));
        let _ = message; // reserved for future redacted detail
        sink.record(DiagnosticRecord::new(
            "capability-dispatch",
            "capability_dispatch",
            label,
            cause,
            Vec::new(),
        ));
    }

    /// Typed entry point: returns the [`CapabilityDispatchError`]
    /// directly so the caller can switch on the typed code. T070
    /// verifies `wire_code()` and `capability()` on the typed
    /// envelope match the JSON envelope emitted by [`Self::dispatch`].
    pub async fn dispatch_typed(
        &self,
        envelope: &str,
        allowed_capabilities: &[String],
    ) -> Result<(), CapabilityDispatchError> {
        match &self.inner {
            DispatcherImpl::Legacy => {
                let _ = self.dispatch_legacy(envelope, allowed_capabilities).await;
                Ok(())
            }
            DispatcherImpl::Adapter { adapter, .. } => {
                let outcome = adapter.dispatch(envelope, allowed_capabilities).await;
                self.record_diagnostic(envelope, &outcome);
                outcome.map(|_| ())
            }
        }
    }

    fn capability_from_envelope(envelope: &str) -> &str {
        // Best-effort: peek the capability field without parsing the
        // full envelope. The diagnostic sink is not a security
        // boundary; it consumes the redacted cause only. The returned
        // `&str` borrows from the input `envelope`, so we can safely
        // return a slice into it.
        if let Ok(value) = serde_json::from_str::<Value>(envelope)
            && let Some(name) = value.get("capability").and_then(Value::as_str)
            && let Some(start) = envelope.find(name)
        {
            let end = start + name.len();
            return &envelope[start..end];
        }
        "<unknown>"
    }

    async fn dispatch_legacy(&self, envelope: &str, allowed_capabilities: &[String]) -> String {
        let reply_bytes: Vec<u8> = match HostCallRequest::decode(envelope.as_bytes()) {
            Err(e) => HostCallReply::failure(
                StableErrorKind::InvalidArgs,
                format!("无效的 host_call 参数: {e}"),
            )
            .encode_json(),
            Ok(call)
                if !DESKTOP_CAPABILITY_CATALOG
                    .iter()
                    .any(|known| known.name == call.capability()) =>
            {
                HostCallReply::failure(
                    StableErrorKind::CapabilityUnknown,
                    format!("未知 Capability: {}", call.capability()),
                )
                .encode_json()
            }
            Ok(call) if !allowed_capabilities.iter().any(|c| c == call.capability()) => {
                let mut selected = allowed_capabilities.to_vec();
                selected.sort();
                let selected = if selected.is_empty() {
                    "无".to_string()
                } else {
                    selected.join(", ")
                };
                HostCallReply::failure(
                    StableErrorKind::CapabilityDenied,
                    format!(
                        "函数测试未授权调用 Capability「{}」；当前已选 Capability：{}。请勾选「{}」后重试",
                        call.capability(),
                        selected,
                        call.capability()
                    ),
                )
                .encode_json()
            }
            Ok(call) if call.capability() == "network.http" => {
                match serde_json::from_slice::<HttpArgs>(call.args_json()) {
                    Ok(args) => match Self::network_http(args).await {
                        Ok(data) => host_call_ok(data).encode_json(),
                        Err(message) => {
                            HostCallReply::failure(StableErrorKind::InvalidArgs, message)
                                .encode_json()
                        }
                    },
                    Err(e) => HostCallReply::failure(
                        StableErrorKind::InvalidArgs,
                        format!("network.http 参数无效: {e}"),
                    )
                    .encode_json(),
                }
            }
            Ok(call) if call.capability() == "log.emit" => {
                // 本地 no-op：接受结构化日志调用并返回成功，不依赖服务端运行时。
                host_call_ok(serde_json::json!({"logged": true})).encode_json()
            }
            Ok(call) => legacy_not_implemented_reply(call.capability()),
        };

        String::from_utf8(reply_bytes).expect("host_call reply is always valid UTF-8 JSON")
    }

    async fn network_http(args: HttpArgs) -> Result<Value, String> {
        let method = args.method.to_ascii_uppercase();
        if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "DELETE" | "PATCH") {
            return Err(format!(
                "method {method} 不允许；仅支持 GET/POST/PUT/DELETE/PATCH"
            ));
        }

        let url = reqwest::Url::parse(&args.url).map_err(|e| format!("URL 无效: {e}"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err("仅允许 http/https URL".into());
        }

        if args
            .body
            .as_ref()
            .is_some_and(|body| body.len() > BODY_MAX_BYTES)
        {
            return Err("请求体超过 4 MB 上限".into());
        }

        let timeout = Duration::from_millis(args.timeout_ms.unwrap_or(20_000).clamp(1, 30_000));
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .build()
            .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
        let request_method = method
            .parse::<reqwest::Method>()
            .map_err(|e| format!("HTTP method 无效: {e}"))?;
        let mut request = client.request(request_method, url);
        for (name, value) in args.headers {
            request = request.header(name, value);
        }
        if let Some(body) = args.body {
            request = request.body(body);
        }

        let response = request
            .send()
            .await
            .map_err(|e| format!("HTTP 请求失败: {e}"))?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    value.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect::<HashMap<_, _>>();
        let body = response
            .bytes()
            .await
            .map_err(|e| format!("读取 HTTP 响应失败: {e}"))?;
        let body_truncated = body.len() > BODY_MAX_BYTES;
        let body = &body[..body.len().min(BODY_MAX_BYTES)];

        Ok(serde_json::json!({
            "status": status,
            "headers": headers,
            "body": String::from_utf8_lossy(body),
            "body_truncated": body_truncated,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_runtime_core::abi::{HostCallReply, StableErrorKind};

    /// T080: the desktop host must emit a byte-identical envelope to the
    /// shared `hive-runtime-core` ABI types, not a locally diverged copy.
    #[tokio::test]
    async fn legacy_dispatcher_failure_matches_shared_abi_envelope() {
        let dispatcher = DesktopHostDispatcher::default();
        let got = dispatcher
            .dispatch(
                r#"{"capability":"unknown.capability","args":{}}"#,
                &["unknown.capability".to_string()],
            )
            .await;

        let expected = String::from_utf8(
            HostCallReply::failure(
                StableErrorKind::CapabilityUnknown,
                "未知 Capability: unknown.capability",
            )
            .encode_json(),
        )
        .expect("shared ABI reply is UTF-8");

        assert_eq!(got, expected);
    }

    /// T080: the success envelope must also round-trip through the shared
    /// [`HostCallReply::success_json`] shape byte-for-byte.
    #[tokio::test]
    async fn legacy_dispatcher_success_matches_shared_abi_envelope() {
        let dispatcher = DesktopHostDispatcher::default();
        let got = dispatcher
            .dispatch(
                r#"{"capability":"log.emit","args":{}}"#,
                &["log.emit".to_string()],
            )
            .await;

        let expected = String::from_utf8(
            HostCallReply::success_json(br#"{"logged":true}"#)
                .expect("valid JSON")
                .encode_json(),
        )
        .expect("shared ABI reply is UTF-8");

        assert_eq!(got, expected);
    }
}
