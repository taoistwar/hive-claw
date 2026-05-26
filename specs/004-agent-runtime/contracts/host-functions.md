# Host Function ABI — Capability Bridge

**Created**: 2026-05-26
**Audience**: WASM Plugin 作者（用 Extism PDK 编写 Plugin 时遵循此契约）

---

## 1. Single Entry: `host_call`

宿主只暴露一个 host function：

```
host_call(envelope_ptr: i64, envelope_len: i64) -> i64
```

- 入参：UTF-8 编码的 JSON 字符串（envelope，见 §2）
- 出参：i64 — Extism 标准的"指针/长度打包"返回值，指向 UTF-8 JSON 结果（见 §3）

Extism PDK 已经把"读写宿主内存"封装好，作者只需把 envelope 序列化、调用、再反序列化。

### Rust PDK 示例

```rust
use extism_pdk::*;
use serde::{Deserialize, Serialize};

#[host_fn]
extern "ExtismHost" {
    fn host_call(envelope: String) -> String;
}

#[derive(Serialize)]
struct Request<'a, T: Serialize> {
    capability: &'a str,
    args: T,
}

#[derive(Deserialize)]
struct Response<T> {
    ok: bool,
    code: Option<u16>,
    message: Option<String>,
    data: Option<T>,
}

#[plugin_fn]
pub fn lookup_weather(city: String) -> FnResult<String> {
    let req = Request {
        capability: "network.http",
        args: serde_json::json!({
            "method": "GET",
            "url": format!("https://api.example.com/weather?city={}", city),
        }),
    };
    let raw = unsafe { host_call(serde_json::to_string(&req)?)? };
    let resp: Response<serde_json::Value> = serde_json::from_str(&raw)?;
    if !resp.ok {
        return Err(WithReturnCode::new(
            Error::msg(format!("{}: {}", resp.code.unwrap_or(0), resp.message.unwrap_or_default())),
            1,
        ));
    }
    Ok(resp.data.unwrap().to_string())
}
```

---

## 2. Request Envelope

```json
{
  "capability": "<capability name>",
  "args": <capability-specific JSON object>
}
```

`capability` 必须是宿主 §4 列出的某个已注册名。任何拼写错误或未注册的名 → 立即返回 `{ ok: false, code: 4040, message: "Capability unknown: ..." }`。

---

## 3. Response Envelope

成功：
```json
{ "ok": true, "data": <capability-specific JSON> }
```

失败（鉴权拒绝 / 超时 / 上游错误 / payload 反序列化失败 / 资源限制）：
```json
{ "ok": false, "code": <u16>, "message": "<human readable>" }
```

错误码：

| code | 含义 |
| --- | --- |
| 4030 | Capability denied（Agent permissions 未包含该 capability） |
| 4040 | Capability unknown |
| 4001 | Invalid args（不符合 capability 的 input schema） |
| 4081 | Timeout（capability 调用上游超时） |
| 5000 | Internal error |

---

## 4. Capability Catalog（args / data schema）

> 每个 capability 内部有自己的"前置校验 → 真正调用 → 后置审计"。Plugin 作者只需关心 args / data。

### 4.1 `network.http`

args:
```json
{
  "method": "GET | POST | PUT | DELETE | PATCH",
  "url": "https://...",   // 必须匹配宿主白名单（hosts allowlist，由 Agent 间接控制）
  "headers": { "Authorization": "Bearer ..." } /* optional */,
  "body": "string"        /* optional, only for POST/PUT/PATCH */,
  "timeout_ms": 5000      /* optional, capped at 30000 */
}
```
data:
```json
{
  "status": 200,
  "headers": { "content-type": "application/json" },
  "body": "..."           // 文本；二进制响应当前未支持
}
```

### 4.2 `fs.read` / `fs.write`

宿主提供受控的临时目录（per-session sandbox），路径必须以 `/tmp/plugin/` 开头。
args:
- read: `{ "path": "/tmp/plugin/foo.txt" }`
- write: `{ "path": "/tmp/plugin/foo.txt", "content_base64": "..." }`

data:
- read: `{ "content_base64": "..." }`
- write: `{ "bytes_written": 1024 }`

### 4.3 `s3.read` / `s3.write`

桶名由宿主固定为本租户桶，args 只接收 key：
- read: `{ "key": "agent/abc/data.json" }` → `{ "content_base64": "..." }`
- write: `{ "key": "...", "content_base64": "...", "content_type"?: "..." }` → `{ "size_bytes": 1024 }`

### 4.4 `db.query` / `db.execute`

**自由 SQL 永远禁用**。Plugin 只能通过宿主预注册的 named query：

```json
{
  "query_name": "admins_by_phone",
  "params": { "phone": "13800138000" }
}
```

宿主端的 `named_queries.toml`（或代码注册）维护 query_name → SQL + 入参 schema。

data:
- query: `{ "rows": [ {...}, ... ] }`
- execute: `{ "rows_affected": 1 }`

### 4.5 `llm.invoke`

args:
```json
{
  "model": "gpt-4o-mini",   // 可选，默认走宿主 env LLM_MODEL
  "messages": [
    { "role": "system", "content": "..." },
    { "role": "user", "content": "..." }
  ],
  "temperature": 0.7,
  "max_tokens": 1024,
  "tools": [...]   // 可选，OpenAI tool-calling 格式
}
```
data: OpenAI Chat Completion 响应原样回传。

### 4.6 `secret.get`

args: `{ "key": "OPENAI_API_KEY" }`
data: `{ "value": "..." }`

宿主维护 secret allowlist；Plugin 不能列出全部 secrets。Agent 的 permissions 必须包含 `secret.get`，且 Super-only。

### 4.7 `time.now`

args: `{}`
data: `{ "unix_ms": 1716700000000, "rfc3339": "2026-05-26T10:00:00Z" }`

### 4.8 `log.emit`

args: `{ "level": "info | warn | error", "message": "...", "fields"?: { ... } }`
data: `{ "logged": true }`

宿主侧会包装为结构化 tracing 事件，挂上 `plugin_id` / `agent_id` / `request_id` 字段。

---

## 5. 限制与边界

| 限制 | 默认 | 说明 |
| --- | --- | --- |
| 单次 Plugin 调用墙钟时间 | 30 秒 | 超时强制中断（Wasmtime fuel + tokio timeout） |
| Plugin linear memory | 128 MB | 在 manifest 中配置 |
| `host_call` payload 大小 | 4 MB | 入参/出参各 4MB |
| `network.http` body 大小 | 4 MB | 响应超过 → 5000 错误 |
| `network.http` 并发 | 8/Plugin/会话 | 防止 plugin 打外网爆量 |
| `log.emit` 速率 | 100/秒/Plugin | 防止日志 flood |
| `db.execute` 单语句行数 | 1000 | 通过 LIMIT 强制 |

---

## 6. 与 Agent permissions 的关系

- Agent 的 `permissions` 是一组 capability 名字符串
- Plugin 在 Agent 上下文里跑时，每次 `host_call` 先在 dispatcher 查 Agent.permissions
- 不包含 → 立即返回 `{ ok: false, code: 4030 }`，不进入真正的 capability handler
- 危险 capability（`is_dangerous = true`）的赋予动作要求 Super；调用动作仍由 dispatcher 鉴权（Super 可以授给 Normal Agent，调用就能通）

---

## 7. 兼容性

- 本契约版本：`v1`
- 后续若添加新 capability，仅是 §4 增项 — Plugin 不需重编译
- 若改 envelope 或错误码语义 — 视为破坏性变更，进 `v2` 并在宿主同时支持 v1/v2 ≥ 一个 minor 版本
