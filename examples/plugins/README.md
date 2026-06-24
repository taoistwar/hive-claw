# Agent Runtime — Plugin Examples

WASM Plugin 作者使用 [Extism PDK](https://extism.org/docs/concepts/pdk) 编写插件。本目录提供 Rust PDK 示例。

## 编写 Rust Plugin

`Cargo.toml`：
```toml
[package]
name = "weather-tool"
version = "1.0.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
extism-pdk = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

`src/lib.rs`：
```rust
use extism_pdk::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct Args {
    city: String,
    /// AgentContext snapshot (auto-injected by orchestrator)
    #[serde(default)]
    _agent_context: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct Reply {
    temp_c: f64,
    summary: String,
    /// AgentContext updates: extensions, records (auto-extracted by orchestrator)
    #[serde(skip_serializing_if = "Option::is_none")]
    _agent_context_updates: Option<AgentContextUpdates>,
}

#[derive(Serialize)]
struct AgentContextUpdates {
    #[serde(default)]
    records: Vec<ContextRecord>,
    #[serde(default)]
    extensions: Vec<ExtensionEntry>,
}

#[derive(Serialize)]
struct ContextRecord {
    category: String,  // "ToolResults", "StateChanges", etc.
    key: String,
    value: serde_json::Value,
}

#[derive(Serialize)]
struct ExtensionEntry {
    id: String,
    content_type: String,  // "card", "image", "suggestion", "link"
    data: serde_json::Value,
}

#[plugin_fn]
pub fn lookup(args_json: String) -> FnResult<String> {
    let args: Args = serde_json::from_str(&args_json)?;

    // ★ Read AgentContext — check cached results
    if let Some(ref ctx) = args._agent_context {
        if let Some(cached) = check_cache(ctx, &args.city) {
            return Ok(serde_json::to_string(&Reply {
                temp_c: cached.temp_c,
                summary: cached.summary,
                _agent_context_updates: None,  // no new data to write
            })?);
        }
    }

    // Call host capability
    let req = serde_json::json!({
        "capability": "network.http",
        "args": {
            "method": "GET",
            "url": format!("https://wttr.in/{}?format=j1", args.city)
        }
    });
    let resp = unsafe { host_call(&serde_json::to_string(&req)?)? };
    // ... parse resp ...

    let (temp_c, summary) = parse_weather(&resp);

    // ★ Write AgentContext — persist result as extension
    Ok(serde_json::to_string(&Reply {
        temp_c,
        summary: summary.clone(),
        _agent_context_updates: Some(AgentContextUpdates {
            extensions: vec![ExtensionEntry {
                id: format!("weather_{}", args.city.to_lowercase()),
                content_type: "card".into(),
                data: serde_json::json!({
                    "title": format!("{} 天气", args.city),
                    "temperature": format!("{}°C", temp_c),
                }),
            }],
            records: vec![],  // or persist result as a record
        }),
    })?)
}

// host_call 由宿主注入
#[host_fn]
extern "ExtismHost" {
    fn host_call(envelope: String) -> String;
}
```

## AgentContext 读写

插件可以通过标准 JSON 字段读写 AgentContext，**无需修改现有插件**。

### 读：`_agent_context`

基础设施在调用插件前自动将 AgentContext 快照注入到输入 JSON 的 `_agent_context` 字段：

```json
{
  "city": "Beijing",
  "_agent_context": {
    "tool_results": [
      {"key": "previous_lookup", "value": {...}, "source": "weather"},
      {"key": "...", "value": {...}, "source": "..."}
    ],
    "entities": [...],
    "state_changes": [...],
    "extensions": [...]
  }
}
```

在 Rust 中用 `#[serde(default)]` 标记即可，不存在时不报错：

```rust
#[derive(Deserialize)]
struct Args {
    city: String,
    #[serde(default)]
    _agent_context: Option<serde_json::Value>,
}
```

### 写：`_agent_context_updates`

在输出 JSON 中包含 `_agent_context_updates`，基础设施自动提取并应用到 AgentContext：

```json
{
  "temp_c": 25.0,
  "summary": "Sunny",
  "_agent_context_updates": {
    "records": [
      {"category": "ToolResults", "key": "my_key", "value": {...}}
    ],
    "extensions": [
      {"id": "card_1", "content_type": "card", "data": {...}}
    ]
  }
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `records[].category` | 字符串 | `Entities` / `ToolResults` / `StateChanges` 等 |
| `records[].key` | 字符串 | 记录唯一标识 |
| `records[].value` | 任意 JSON | 记录内容 |
| `extensions[].id` | 字符串 | 扩展唯一标识 |
| `extensions[].content_type` | 字符串 | `card` / `image` / `suggestion` / `link` / `button` / `table` / `chart` |
| `extensions[].data` | 任意 JSON | 扩展数据 |

### 向后兼容

- 不读 `_agent_context`：`#[serde(default)]` 使其可选，原插件正常工作
- 不写 `_agent_context_updates`：`#[serde(skip_serializing_if)]` 使其不出现，`apply_agent_context_updates()` 为 no-op

## 编译

```bash
cargo build --target wasm32-unknown-unknown --release
# 输出：target/wasm32-unknown-unknown/release/weather_tool.wasm
```

## 上传

通过管理中心 UI 的 Plugin 上传页面，或直接调用 API：
```bash
curl -X POST http://localhost:3300/api/plugins \
  -H "Authorization: Bearer $JWT" \
  -F file=@weather_tool.wasm \
  -F 'meta={"identifier":"weather-tool","name":"Weather","version":"1.0.0","runtime":"extism"}'
```

## 调用约束（见 contracts/host-functions.md）

- 单次调用 30s 硬超时（fuel + tokio 双层）
- 线性内存 128 MB 上限
- payload 上下行 4 MB
- 必须经过宿主 Capability 鉴权才能调用（network.http / db.execute / llm.invoke 等）
