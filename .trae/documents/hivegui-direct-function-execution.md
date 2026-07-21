# hivegui 直接执行函数（不依赖 hiveweb HTTP）

## Context

hivegui 测试函数功能当前通过 HTTP 请求 hiveweb（`POST /api/functions/:id/invoke`），但 hiveweb 需要 MySQL/Redis/S3 等基础设施，本地开发时经常未运行。用户要求 hivegui 直接执行内置函数和 extism 插件函数，不经过 HTTP。

**用户决策：**
- 纯计算内置函数（format_template, json_parse, json_stringify, text_regex_match）抽取到共享 crate
- WASM 插件文件存储到本地文件系统
- 移除 UserInput 上下文配置

## 实现步骤

### Step 1: 创建共享 crate `hive-builtins`

**新增文件：**
- `crates/hive-builtins/Cargo.toml` — 依赖 serde, serde_json, thiserror, regex
- `crates/hive-builtins/src/lib.rs` — BuiltinRegistry + BuiltinError + BuiltinResult
- `crates/hive-builtins/src/format_template.rs` — 从 hiveweb 复制，移除 `BuiltinContext` 参数
- `crates/hive-builtins/src/json_parse.rs` — 同上
- `crates/hive-builtins/src/json_stringify.rs` — 同上
- `crates/hive-builtins/src/text_regex_match.rs` — 同上

每个函数签名从 `fn(args: Value, _ctx: &BuiltinContext) -> BuiltinResult` 改为 `fn(args: Value) -> BuiltinResult`。

**修改文件：**
- `Cargo.toml` — workspace members 添加 `"crates/hive-builtins"`
- `crates/hiveweb/Cargo.toml` — 添加 `hive-builtins = { path = "../hive-builtins" }`
- `crates/hiveweb/src/runtime/builtins/mod.rs` — 纯计算函数改为 re-export hive-builtins

### Step 2: hivegui 添加依赖

**修改文件：** `crates/hivegui/Cargo.toml`
```toml
hive-builtins = { path = "../hive-builtins" }
extism = "1"
```

### Step 3: hivegui 创建 runtime 模块

**新增文件：**
- `crates/hivegui/src/runtime/mod.rs` — 模块声明
- `crates/hivegui/src/runtime/builtin_executor.rs` — 调用 hive_builtins::BuiltinRegistry::execute()
- `crates/hivegui/src/runtime/plugin_executor.rs` — 用 extism 加载本地 WASM 并执行

PluginExecutor 核心逻辑：
```rust
pub async fn execute(wasm_path: &Path, export_name: &str, input_json: &str) -> Result<String, String> {
    let wasm_bytes = tokio::fs::read(wasm_path).await.map_err(...)?;
    let manifest = Manifest::new([Wasm::data(wasm_bytes)]);
    let mut plugin = PluginBuilder::new(manifest).with_wasi(true).build().map_err(...)?;
    plugin.call::<&str, String>(export_name, input_json).map_err(...)
}
```

**修改文件：** `crates/hivegui/src/lib.rs` — 添加 `pub mod runtime;`

### Step 4: WASM 文件本地存储

**修改文件：** `crates/hivegui/src/datasource/entity_store.rs`
- Plugin 添加 `wasm_path(base_dir)` 辅助方法，返回 `{base_dir}/plugins/{id}/plugin.wasm`

**修改文件：** `crates/hivegui/src/ui/plugin_view.rs`
- 插件保存成功后，将 WASM 文件复制到本地目录 `{data_dir}/plugins/{plugin_id}/plugin.wasm`
- 插件删除时清理本地目录

### Step 5: 修改 function_view.rs

**修改文件：** `crates/hivegui/src/ui/function_view.rs`

1. **移除 UserInput 相关字段** — show_user_input, test_user_raw_text, test_user_actor_id 等 7 个字段
2. **简化 show_test_dialog** — 移除 UserInput InputState 创建
3. **重写 run_test** — 替换 HTTP 请求为：
   - kind==1（内置函数）：调用 `BuiltinExecutor::execute(&function.identifier, input)`
   - kind==2（插件函数）：获取 plugin → 读取本地 WASM → 调用 `PluginExecutor::execute(wasm_path, export_name, input_json)`
4. **移除 UserInput UI** — 删除渲染代码中的 UserInput 展开/收起区域和测试输入字段
5. **简化重置按钮** — 移除 UserInput 清理逻辑

## 关键文件

| 文件 | 操作 |
|------|------|
| `crates/hive-builtins/` (新 crate) | 新增 |
| `crates/hivegui/src/runtime/` (新模块) | 新增 |
| `crates/hivegui/src/ui/function_view.rs` | 大量修改 |
| `crates/hivegui/src/ui/plugin_view.rs` | 添加 WASM 本地保存 |
| `crates/hivegui/src/datasource/entity_store.rs` | 添加 wasm_path 方法 |
| `crates/hivegui/Cargo.toml` | 添加依赖 |
| `crates/hiveweb/src/runtime/builtins/mod.rs` | re-export 共享实现 |

## 验证

1. `cargo check -p hive-builtins` — 共享 crate 编译通过
2. `cargo check -p hivegui` — hivegui 编译通过
3. `cargo check -p hiveweb` — hiveweb 不受影响
4. 在 hivegui 中测试内置函数（如 format.template）直接执行
5. 上传插件后测试插件函数直接执行
