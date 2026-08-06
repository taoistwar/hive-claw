# Quickstart: Agent Runtime

> **范围更新（2026-07-23）：** 原 `web-admin` 测试聊天和 `/api/chat/sessions*` SSE 流已由 `81a84fe` 移除并 superseded，不得恢复。现行普通用户聊天位于 `web-user`，调用 `/api/assistant`、`/api/newsession`、`/api/messages`；本 quickstart 只说明边界，详细请求签名与 payload 以 `007-external-assistant-api` 和当前实现为准。

**Created**: 2026-05-26
**Feature**: 004-agent-runtime
**Audience**: 内部开发者 + Plugin 作者

---

## 1. 前置

已完成 003-admin-center 的部署（MySQL / Redis / Rustfs + hiveweb backend + web-admin frontend）。本特性在其上叠加。

```bash
./scripts/dev-up.sh -d                  # MySQL / Redis / MinIO
cargo run -p hiveweb --bin migrate      # 应用实际 V001–V033 链（V013 保留空号）
```

不要按旧设计文档补建 V019–V038 Agent Runtime SQL：旧字段已合并到
V005–V012；当前 V032 是 runtime audit，V033 是 Workflow timeout 默认值
规范化迁移。

Rustfs / MinIO 使用自定义 endpoint 时保持 path-style 桶寻址（默认即为 `true`）；若目标服务要求虚拟主机桶寻址，可由用户显式改为 `false`：

```bash
export AWS_S3_FORCE_PATH_STYLE=true
```

开发 CLI 的请求体/密码不得放入 argv。`api-test` 使用
`--body-stdin` 或 Unix 上由当前用户持有、权限 0600 且无链接的
`--body-file`；旧位置参数 body 仅为有弃用警告的兼容：

```bash
jq -nc --arg user_id 448 '{user_id:$user_id}' | \
  cargo run -p hiveweb --features dev-tools --bin api-test -- \
    GET /api/quota --body-stdin --host https://cca.haimacloud.com/
```

`create-super-admin` 仅用于 INSERT bootstrap；重复 phone 会非零失败，不会更新
现有账户。密码须为 6–20 个 Unicode code point 并同时含 ASCII 字母和数字，后续轮换必须登录后使用
认证的修改密码流程。

`seed-bench` 会删除既有非 Super fixture，故使用独立 `bench-tools` feature，
并且 `DATABASE_URL` 的安全解析结果必须以 `_test` 或 `_bench` 结尾：

```bash
# DATABASE_URL 由私密环境注入，并已选择 hiveweb_bench。
cargo run -p hiveweb --features bench-tools --bin seed-bench -- \
  100 100 --confirm-destructive hiveweb_bench
```

先复制并校验 LLM preset 配置。HiveWeb 在构建 Router 和监听端口前加载该文件；
文件缺失、TOML 无效或默认 preset 无效都会以静态错误非零退出，不会退化为空
registry。无效的非默认 preset 会被静态告警并跳过：

```bash
cp crates/hiveweb/llm_presets.toml.example ./llm_presets.toml
export LLM_PRESETS_PATH="$PWD/llm_presets.toml"
export OPENAI_API_KEY="<injected-by-your-secret-manager>"
```

provider 默认必须声明 `api_key_env`，且该环境变量在启动时存在并非空；
唯一免认证形式是 `auth="none"` + 显式合法 http(s) `base_url`，并且不得同时
声明 `api_key_env`。变量名不会被当作 API Key。HiveWeb 在启动期构造并缓存
每个命名 preset 的完整 primary + fallback chain；修改配置后必须重启。不同
provider 条目可以使用相同 model，runtime 以 preset 名 + `providers[]` 顺序
识别节点，不按 model 名反查。

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
- **model_preset 下拉**：选 `cheap-fast` / `code-expert` 等，对应 `llm_presets.toml` 中的命名 preset；留空 = 用启动期标记 `default = true` 的 preset。只有留空/NULL 会用 default；保存的显式名称若在当前 registry 中未知，请求会以 5007 失败，不会静默换模型
- system_prompt：`You are the entry agent. Use the weather tool when the user asks about temperature.`

> 危险 capability（`db.execute` / `secret.get` / `network.http`）+ main 编辑都需要 Super 角色。System 角色看不到危险 capability 的勾选框。

---

## 8. 验证现行普通用户 Assistant

不要在 `web-admin` 中寻找或恢复“测试聊天”。通过 `web-user` 新建用户会话并发送“北京现在多少度”；客户端依次使用：

1. `POST /api/newsession` 创建普通用户会话
2. `POST /api/assistant` 执行 Agent 并接收完整 JSON 消息
3. `POST /api/messages` 拉取该用户的历史消息

三者使用 `chat_sessions_user` / `chat_messages_user` 并按 `user_id` 隔离。Plugin/Tool/Agent 的运行审计仍可用于确认 `weather.lookup` 是否执行；客户端不会收到原管理端 SSE 的 `token` / `tool_call` / `done` 事件。

---

## 9. 编排一个 Workflow

**Web → Workflow → 新建**：拖入 2 个 Function 节点，画连线，配置 mapping：
- node A: `weather.lookup` → outputs `temp_c`
- node B: 内置 function `format.message` → input.template = `"今天 {} 度"`，input.args = `[A.outputs.temp_c]`
- 边：`{ "args.0": "A.output.temp_c" }`
- 分别点击开始/结束节点，在节点详情中填写开始描述与结束描述；保存后重新打开
  DAG，两个节点必须显示同一描述（分别由虚拟
  `start.position.start_description` / `end.position.end_description` 往返，
  不写入 `workflow_nodes`）。

保存 → 执行：
```bash
curl -X POST http://localhost:3300/api/workflows/<id>/execute \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"input": {"city": "北京"}}'
```

---

## 10. 路由到专家 Agent

**Web → Agent → 新建** `coding-expert`（parent = main）→ tools / skills / permissions 视需要配置 → 在 main 的 system_prompt 中提示"对编程问题路由到 coding-expert"。

下次从 `web-user` 问"Rust 写个 async function" → `/api/assistant` 返回最终 JSON 回复；通过 runtime audit / tracing 确认请求路由到 coding-expert。客户端不再依赖原管理端 SSE `routed` 事件。

---

## 11. 验证 LLM fallback 合同

为测试 preset 配置 primary 与至少一个 fallback，并让 primary 返回 503。验证：

- 启动后多次调用复用同一个缓存 chain；调用路径不出现 `build_primary`
- 备用 provider 可与 primary 使用相同 model，仍按配置顺序命中正确 backend
- 省略 `max_tokens` / `temperature` 时使用 preset 默认；显式值在 fallback 后不变
- 成功结果、tracing 与 runtime audit 一致包含
  `actual_model` / `fallback_used` / `reason`
- provider 切换记录 `llm_fallback`；generate-answer 等调用方合成本地文本时
  记录 `llm_local_fallback`，且 `fallback_used=false`
- 单 provider 25 秒、普通整链 45 秒；`llm.invoke` 整链 25 秒
- `llm.invoke` 显式 `max_tokens=0` 返回 4001，且 provider 请求数为 0
- registry 的 `build_chain(None)` 使用 default；对 `llm.invoke`，只有存在的
  Agent DB 行中 `model_preset IS NULL` 才走 default。显式未知 preset 返回
  5007 / typed error，缺失 Agent 行安全失败，两者都不请求 default provider

---

## 12. Plugin Instance Pool 健康检查

`GET /api/runtime/pool/stats` 返回每个 Plugin 当前的：
- in_use（已占用 invocation slot）
- idle（空闲 `CompiledPlugin` 缓存）
- created_total（成功创建 fresh Store/Instance 的累计次数；失败尝试不计）
- cache_misses（真实成功编译次数；编译失败和缓存命中实例化不计）
- wait_count（进入 FIFO 等待的次数）
- audit.enqueued / persisted / tracing_only
- audit.dropped_queue_full / dropped_writer_closed / dropped_no_writer /
  persist_failures

Pool 同时强制 per-Plugin/global 上限；容量只计算 `in_use + reserved`，idle
编译缓存不占 permit。FIFO acquire 超时返回 5009，调用取消不得泄漏 slot。
idle reaper 随服务启动并在 shutdown 停止。每次调用创建 fresh Store/Instance，
不复用 memory/global/table。期望：稳态后 `cache_misses` 不再增长；
`dropped_*` 与 `persist_failures` 保持 0。Hook 会增加 `tracing_only`，这是预期
的 tracing-only 审计策略。

用 SIGINT（Ctrl-C）或 SIGTERM 停止服务时，HiveWeb 先停止 accept，再最多等待
30 秒 drain 活跃请求，随后停止 reaper、drop Pool/编译缓存并关闭 DB pool。

---

## 12. 常见错误

| 错误码 | 含义 | 怎么修 |
| --- | --- | --- |
| 4030 Capability denied | Agent 缺少该 capability | 编辑 Agent → 勾选对应 capability |
| 4045 Capability unknown | Plugin 调了错误的 capability 名 | 检查 envelope JSON 中的拼写 |
| 5004 Plugin invocation timeout | Plugin 跑超 30 秒 | 检查 Plugin 逻辑；或在 Workflow 节点放大 timeout |
| 5009 Plugin pool busy | FIFO acquire 超时 | 检查 per-Plugin/global 上限与 wait_count |
| 5002 Schema mismatch | Tool 与 Function schema 不一致 | 编辑 Tool 或 Function 让 schema 对齐 |
| 5006 Agent depth exceeded | 层级 > 10 | 重新组织 Agent 层级 |
