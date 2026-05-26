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
struct Args { city: String }

#[derive(Serialize)]
struct Reply { temp_c: f64, summary: String }

#[plugin_fn]
pub fn lookup(args: String) -> FnResult<String> {
    let a: Args = serde_json::from_str(&args)?;

    // 调宿主能力 network.http
    let req = serde_json::json!({
        "capability": "network.http",
        "args": {
            "method": "GET",
            "url": format!("https://wttr.in/{}?format=j1", a.city)
        }
    });
    let resp = unsafe { host_call(&serde_json::to_string(&req)?)? };
    // ... 解析 resp ...

    Ok(serde_json::to_string(&Reply {
        temp_c: 22.5,
        summary: "Sunny".into(),
    })?)
}

// host_call 由宿主注入
#[host_fn]
extern "ExtismHost" {
    fn host_call(envelope: String) -> String;
}
```

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
