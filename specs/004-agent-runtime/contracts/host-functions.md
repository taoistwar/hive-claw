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

HiveWeb 使用 capability-only Extism runtime：WASI 被显式关闭，Plugin 的
`wasi_snapshot_preview1` 等 import 会在实例化时被拒绝。Extism 自身的
`extism:host/env` PDK 基础设施和本节的 `extism:host/user.host_call` 保持可用；
关闭 WASI 不改变下面的 PDK 调用方式。

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

`capability` 必须是宿主 §4 列出的某个已注册名。任何拼写错误或未注册的名 → 立即返回 `{ ok: false, code: 4045, message: "Capability unknown: ..." }`。

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
| 4045 | Capability unknown（避免与 003 NOT_FOUND 冲突） |
| 4001 | Invalid args（不符合 capability 的 input schema） |
| 4081 | Timeout（capability 调用上游超时） |
| 4292 | Capability rate limited（宿主策略拒绝；稍后重试） |
| 5000 | Internal error |

`4001` / `4081` 只能来自宿主内部的 typed/static handler 分类。下游响应正文、
provider content 或任意错误字符串即使包含 `timeout`，也不得改变 reply code。
reply message、tracing 顶层 `error_kind` 与 runtime audit payload summary 必须由同一
安全分类生成；原始错误文本不进入任一输出。

限流拒绝使用独立的 `4292` 策略错误码，不得复用已 superseded 的 admin
SSE `4291` 或误报为 `5000`。固定 reply message 为
`Capability rate limit exceeded`，不得包含 Plugin/session/URL/log 内容。

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
  "messages": [
    { "role": "system", "content": "..." },
    { "role": "user", "content": "..." }
  ],
  "prompt": null,
  "temperature": 0.2,
  "max_tokens": 1024
}
```

`messages` 与 `prompt` 至少提供一个。Plugin 不能在 capability payload 中选择
其他 preset；宿主始终使用当前 Agent 的 `model_preset`，只有该字段为 NULL
时才使用全局 default。显式非 NULL 但未知的 preset 返回 typed
`ModelPresetUnknown` 并 fail closed。这里的 NULL 指已存在 Agent DB 行中的
字段值；若 Agent 行不存在，宿主在 registry/provider 前安全失败，不得将缺失行
解释为 NULL 或请求 default provider。

`temperature` / `max_tokens` 均可省略。省略时使用命名 preset 的默认值；显式
提供时，该值在 primary 与所有 fallback provider 间保持不变。显式
`max_tokens=0` 无效，宿主返回 4001，且不得把它当作省略值应用 preset 或
provider 默认值。data:

```json
{
  "content": "...",
  "finish_reason": "stop",
  "usage": {},
  "actual_model": "fallback-model",
  "fallback_used": true,
  "reason": "server_error"
}
```

`reason` 是可空的宿主静态分类；primary 成功时为 NULL。不得返回 provider
原始错误文本。每个 provider 节点最多 25 秒；由于 `llm.invoke` 位于 30 秒
Plugin 外层硬超时内，其**整条 fallback chain** 最多 25 秒，而不是普通调用
使用的 45 秒。链预算耗尽返回 typed 4081 timeout 并停止尝试后续 provider。
`llm.invoke` 不生成本地文本兜底；调用方应用层若自行合成文本，必须记
`llm_local_fallback`，不能把它标记为 `fallback_used=true`。

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

宿主侧仅记录结构化元数据：`plugin_id`、`agent_id`、规范化 level、
`message_bytes` 与 `field_count`。Plugin 提供的 message、字段名和字段值均不得
进入宿主日志。

---

## 5. 限制与边界

| 限制 | 默认 | 说明 |
| --- | --- | --- |
| 单次 Plugin 调用墙钟时间 | 30 秒 | 超时强制中断（Wasmtime fuel + tokio timeout） |
| Plugin linear memory | 128 MB | 在 manifest 中配置 |
| `host_call` payload 大小 | 4 MB | 入参/出参各 4MB |
| `network.http` body 大小 | 4 MB | 响应超过 → 5000 错误 |
| `network.http` 并发 | 8/Plugin/会话 | 单宿主进程按 `(plugin_id, Option<session_id>)` 计数；`None` 为共享 sentinel；permit 覆盖完整 HTTP future |
| `log.emit` 速率 | 100/秒/Plugin | 容量 100、补充 100 token/s 的令牌桶；生产时钟在 bucket mutex 内采样，refill/seen 时间只可单调前进；只限制 Plugin log handler，拒绝 audit 不采样 |
| `db.execute` 单语句行数 | 1000 | 通过 LIMIT 强制 |
| LLM provider 节点 | 25 秒 | 单节点墙钟上限；若整链仍有预算才切下一节点 |
| 普通 LLM fallback chain | 45 秒 | orchestrator / Workflow / 测试 / builtin 的整链上限 |
| `llm.invoke` fallback chain | 25 秒 | 留出 5 秒供 30 秒 Plugin 外层取消、归还与审计 |

Pool 只缓存 `CompiledPlugin`；每次调用 fresh Store/Instance，因此 memory、
mutable global 与 table 不跨调用。per-Plugin/global permit 按 FIFO 获取，
acquire 超时返回 5009 并增加 `wait_count`。调用方 abort/cancel 时宿主取消 WASM
并 cancellation-safe 地释放 checked-out slot；fresh Instance 直接 drop。
容量只计算 `in_use + reserved`，idle 编译缓存不占 permit。真实编译成功才增加
`cache_misses`，fresh Store/Instance 成功创建才增加 `created_total`。

dispatcher 先完成 capability 权限检查和 args 反序列化，再做上述准入，因此
未知/未授权调用优先返回原错误，非法 args 不消耗配额。限流拒绝写恰好一条
脱敏的 `capability_denied` audit，不记录 URL path/query、headers、Plugin
message/fields 或 session 原文。

---

## 6. 与 Agent permissions 的关系

- Agent 的 `permissions` 是一组 capability 名字符串
- Plugin 在 Agent 上下文里跑时，每次 `host_call` 先在 dispatcher 查 Agent.permissions
- 不包含 → 立即返回 `{ ok: false, code: 4030 }`，不进入真正的 capability handler
- permissions DB 查询失败 → 返回固定 `5000`，并写恰好一条脱敏的
  `capability_call/error`；不得记录连接错误或原始 envelope/payload
- 危险 capability（`is_dangerous = true`）的赋予动作要求 Super；调用动作仍由 dispatcher 鉴权（Super 可以授给 Normal Agent，调用就能通）

---

## 7. 兼容性

- 本契约版本：`v1`
- 后续若添加新 capability，仅是 §4 增项 — Plugin 不需重编译
- 若改 envelope 或错误码语义 — 视为破坏性变更，进 `v2` 并在宿主同时支持 v1/v2 ≥ 一个 minor 版本
