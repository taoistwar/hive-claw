//! Capability dispatcher used by local desktop function tests.

use std::{collections::HashMap, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::Value;

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

#[derive(Debug, Deserialize)]
struct CallEnvelope {
    capability: String,
    #[serde(default)]
    args: Value,
}

#[derive(Debug, Serialize)]
struct ReplyEnvelope {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl ReplyEnvelope {
    fn ok(data: Value) -> Self {
        Self {
            ok: true,
            data: Some(data),
            code: None,
            message: None,
        }
    }

    fn error(code: u16, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            code: Some(code),
            message: Some(message.into()),
        }
    }
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
pub struct DesktopHostDispatcher;

impl DesktopHostDispatcher {
    /// Dispatches one JSON host-call envelope using an explicit permission snapshot.
    pub async fn dispatch(envelope: &str, allowed_capabilities: &[String]) -> String {
        let reply = match serde_json::from_str::<CallEnvelope>(envelope) {
            Err(e) => ReplyEnvelope::error(4001, format!("无效的 host_call 参数: {e}")),
            Ok(call)
                if !DESKTOP_CAPABILITY_CATALOG
                    .iter()
                    .any(|known| known.name == call.capability) =>
            {
                ReplyEnvelope::error(4045, format!("未知 Capability: {}", call.capability))
            }
            Ok(call) if !allowed_capabilities.contains(&call.capability) => {
                let mut selected = allowed_capabilities.to_vec();
                selected.sort();
                let selected = if selected.is_empty() {
                    "无".to_string()
                } else {
                    selected.join(", ")
                };
                ReplyEnvelope::error(
                    4030,
                    format!(
                        "函数测试未授权调用 Capability「{}」；当前已选 Capability：{}。请勾选「{}」后重试",
                        call.capability, selected, call.capability
                    ),
                )
            }
            Ok(call) if call.capability == "network.http" => {
                match serde_json::from_value::<HttpArgs>(call.args) {
                    Ok(args) => match Self::network_http(args).await {
                        Ok(data) => ReplyEnvelope::ok(data),
                        Err(message) => ReplyEnvelope::error(4001, message),
                    },
                    Err(e) => ReplyEnvelope::error(4001, format!("network.http 参数无效: {e}")),
                }
            }
            Ok(call) => ReplyEnvelope::error(
                5010,
                format!(
                    "Capability「{}」依赖服务端运行时，HiveGUI 本地测试暂不支持",
                    call.capability
                ),
            ),
        };

        serde_json::to_string(&reply).unwrap_or_else(|e| {
            format!(r#"{{"ok":false,"code":5000,"message":"序列化响应失败: {e}"}}"#)
        })
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
