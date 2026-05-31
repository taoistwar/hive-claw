# Quickstart: Agent Runtime

**Created**: 2026-05-26
**Feature**: 004-agent-runtime
**Audience**: 内部开发者 + Plugin 作者

---

## 1. 前置

已完成 003-admin-center 的部署（MySQL / Redis / Rustfs + hiveweb backend + web-admin frontend）。本特性在其上叠加。

```bash
./scripts/dev-up.sh -d                  # MySQL / Redis / MinIO
cargo run -p hiveweb --bin migrate      # 应用 V008..V018
```

LLM 环境变量（OpenAI 兼容；本地 vLLM / Ollama / 商业 API 任选）：

```bash
export LLM_BASE_URL="https://api.openai.com/v1"
export LLM_API_KEY="sk-..."
export LLM_MODEL="gpt-4o-mini"
```

---

## 2. 启动后端 + 前端

```bash
cargo run -p hiveweb --bin hiveweb &     # :3300
cd web-admin && npm install && npm run dev     # :5173
```

浏览器打开 http://localhost:5173/，用管理中心账号登录（Super）。

---

## 3. 写一个最小 Plugin（Rust PDK）

```bash
mkdir -p plugins/weather && cd plugins/weather
cargo init --lib
```

`Cargo.toml`：
```toml
[package]
name = "weather"
version = "0.1.0"
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

#[host_fn]
extern "ExtismHost" {
    fn host_call(envelope: String) -> String;
}

#[derive(Deserialize)]
pub struct Input { pub city: String }
#[derive(Serialize)]
pub struct Output { pub temp_c: f64, pub summary: String }

#[plugin_fn]
pub fn lookup(input: Json<Input>) -> FnResult<Json<Output>> {
    let req = serde_json::json!({
        "capability": "network.http",
        "args": {
            "method": "GET",
            "url": format!("https://wttr.in/{}?format=j1", input.0.city),
        }
    });
    let raw = unsafe { host_call(serde_json::to_string(&req)?)? };
    let resp: serde_json::Value = serde_json::from_str(&raw)?;
    if !resp["ok"].as_bool().unwrap_or(false) {
        return Err(WithReturnCode::new(
            Error::msg(resp["message"].as_str().unwrap_or("").to_string()), 1,
        ));
    }
    // wttr 返回的 JSON 在 data.body 里（字符串），demo 不做精细解析
    Ok(Json(Output { temp_c: 22.0, summary: "demo".into() }))
}
```

编译：
```bash
cargo build --release --target wasm32-unknown-unknown
# 产物：target/wasm32-unknown-unknown/release/weather.wasm
```

---

## 4. 上传 Plugin

UI 路径：**Web → 插件管理 → 添加** → 上传 `weather.wasm`：
- identifier: `weather`
- name: `Weather`
- version: `0.1.0`
- runtime: `extism`
- description: `Lookup weather via wttr.in`

或用 curl：
```bash
TOKEN=...
curl -X POST http://localhost:3300/api/plugins \
  -H "Authorization: Bearer $TOKEN" \
  -F file=@target/wasm32-unknown-unknown/release/weather.wasm \
  -F 'meta={"identifier":"weather","name":"Weather","version":"0.1.0","runtime":"extism","description":"Lookup weather"};type=application/json'
```

---

## 5. 注册 Function

UI 路径：**Web → 函数管理 → 添加（定制）**：
- identifier: `weather.lookup`
- name: `查天气`
- plugin_id: 选刚才上传的 weather
- plugin_export: `lookup`
- input_schema:
  ```json
  { "type": "object", "properties": { "city": { "type": "string" }}, "required": ["city"] }
  ```
- output_schema:
  ```json
  { "type": "object", "properties": {
      "temp_c": { "type": "number" }, "summary": { "type": "string" }
    }, "required": ["temp_c", "summary"] }
  ```

---

## 6. 包装 Tool

**Web → 工具管理 → 添加**：
- kind: function
- function_id: `weather.lookup`
- 自动复用 schema

---

## 6b. （可选）创建 Skill — markdown 内容

Skill 是 markdown 文档，调用时拼到 Agent 的 system prompt（不出现在 LLM tools 列表中）。

**Web → 技能管理 → 添加**：
- identifier: `weather-style`
- name: 天气问答风格
- description: 用日常口语 + emoji 描述温度
- content：
  ```markdown
  当用户询问天气时：
  - 用日常口语，避免数字罗列
  - 适当加 emoji（☀️ 🌧️ ❄️ 🌫️）
  - 主动建议穿衣
  ```

或用 curl：
```bash
curl -X POST http://localhost:3300/api/skills \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{
    "identifier": "weather-style",
    "name": "天气问答风格",
    "description": "用日常口语 + emoji 描述温度",
    "content": "当用户询问天气时：\n- 用日常口语，避免数字罗列\n- 适当加 emoji（☀️ 🌧️ ❄️ 🌫️）\n- 主动建议穿衣"
  }'
```

---

## 7. 配置 Agent 权限 + 模型 preset

**Web → Agent 管理 → main → 编辑**：
- 添加 Tool: `weather.lookup`
- 添加 Skill: `weather-style`（其 markdown 内容会拼到 system prompt）
- permissions：勾选 `network.http`、`llm.invoke`
- **model_preset 下拉**：选 `cheap-fast` / `code-expert` 等，对应 `llm_presets.toml` 中的命名 preset；留空 = 用启动期标记 `default = true` 的 preset
- system_prompt：`You are the entry agent. Use the weather tool when the user asks about temperature.`

> 危险 capability（`db.execute` / `secret.get` / `network.http`）+ main 编辑都需要 Super 角色。System 角色看不到危险 capability 的勾选框。

---

## 8. 测试聊天

**Web → 测试聊天 → 新会话**：

输入：`北京现在多少度`

应当看到：
1. SSE 流推送 `token` 事件 → 增量文本
2. 一个 `tool_call` 事件指向 `weather.lookup` 工具
3. 一个 `tool_result` 事件返回 Plugin 的输出
4. 最终 `done`

如果 Agent 没有 `network.http` capability，会看到 `error` 事件 + 错误码 4030 + audit 中的拒绝记录。

---

## 9. 编排一个 Workflow

**Web → Workflow → 新建**：拖入 2 个 Function 节点，画连线，配置 mapping：
- node A: `weather.lookup` → outputs `temp_c`
- node B: 内置 function `format.message` → input.template = `"今天 {} 度"`，input.args = `[A.outputs.temp_c]`
- 边：`{ "args.0": "A.output.temp_c" }`

保存 → 执行：
```bash
curl -X POST http://localhost:3300/api/workflows/<id>/execute \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"input": {"city": "北京"}}'
```

---

## 10. 路由到专家 Agent

**Web → Agent → 新建** `coding-expert`（parent = main）→ tools / skills / permissions 视需要配置 → 在 main 的 system_prompt 中提示"对编程问题路由到 coding-expert"。

下次问"Rust 写个 async function" → 应见 SSE `routed` 事件 → 最终回复来自 coding-expert。

---

## 11. Plugin Instance Pool 健康检查

`GET /api/runtime/pool/stats` 返回每个 Plugin 当前的：
- in_use（被借走的实例数）
- idle（空闲）
- created（累计创建数）
- cache_misses（编译次数）

期望：稳态后 `cache_misses` 不再增长（命中率 100%）。

---

## 12. 常见错误

| 错误码 | 含义 | 怎么修 |
| --- | --- | --- |
| 4030 Capability denied | Agent 缺少该 capability | 编辑 Agent → 勾选对应 capability |
| 4040 Capability unknown | Plugin 调了错误的 capability 名 | 检查 envelope JSON 中的拼写 |
| 5004 Plugin invocation timeout | Plugin 跑超 30 秒 | 检查 Plugin 逻辑；或在 Workflow 节点放大 timeout |
| 5002 Schema mismatch | Tool 与 Function schema 不一致 | 编辑 Tool 或 Function 让 schema 对齐 |
| 5006 Agent depth exceeded | 层级 > 10 | 重新组织 Agent 层级 |
