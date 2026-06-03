# Quickstart: Agent Context 能力

## 开发上手

### 1. 前置条件

- Rust 1.85+ 已安装
- `crates/agent` 已编译通过

```bash
cd crates/agent
cargo check
```

### 2. 构建 Agent Context

```bash
# 在 crates/agent 目录下
mkdir -p src/context
# 创建新文件: src/context/{mod.rs,core.rs,category.rs,lock.rs,read_view.rs,merge.rs,prompt.rs,response.rs,serialize.rs,audit.rs,config.rs}
```

更新 `crates/agent/src/lib.rs`:

```rust
pub mod context;
pub use context::{AgentContext, ReadView, Category, ContextError, ContextConfig};
```

### 3. 基本使用示例

#### 创建 Context

```rust
use agent::context::{AgentContext, UserInput, ContextConfig};

let user_input = UserInput {
    raw_text: "查询明日游戏活动".to_string(),
    session_id: Some("sess-001".to_string()),
    message_id: None,
    timestamp: SystemTime::now(),
    metadata: HashMap::new(),
};

let config = ContextConfig::default();
let ctx = AgentContext::new("exec-001".to_string(), user_input, config);
```

#### 写入状态

```rust
use agent::context::Category;

ctx.set_record(
    Category::Entities,
    "game_name".to_string(),
    serde_json::json!({"name": "王者荣耀"}),
    "entity_extractor".to_string(),
    0,
)?;
```

#### 读取状态

```rust
let entities = ctx.get_category(Category::Entities)?;
let game = ctx.get_record(Category::Entities, "game_name")?;
```

#### Tool 写入结果

```rust
ctx.set_record(
    Category::ToolResults,
    "search_activities".to_string(),
    serde_json::json!([{"activity": "新春庆典", "time": "2026-02-01"}]),
    "search_activities".to_string(),
    1,
)?;
```

#### 构建 Prompt

```rust
let prompt = ctx.build_prompt_budget(128_000)?;
println!("Estimated tokens: {}", prompt.estimated_tokens);
println!("Truncated: {}", prompt.truncated);
```

#### 子 Agent 协作

```rust
// 创建子 Agent Context（黑名单: SubagentResults）
let child = ctx.fork_for_subagent(
    "sub-001".to_string(),
    vec![Category::SubagentResults],
    ContextConfig::default(),
)?;

// 子 Agent 执行...
child.set_record(
    Category::ToolResults,
    "sub_search".to_string(),
    serde_json::json!({"result": "found"}),
    "sub-001".to_string(),
    0,
)?;

// 合并回主 Context
ctx.merge_subagent_context("sub-001".to_string(), &child)?;
```

#### 只读视图

```rust
let view = ctx.read_view();
let entities = view.get_category(Category::Entities);
// view 不提供任何写入方法，防止意外修改
```

#### 序列化

```rust
// 创建快照
let snapshot = ctx.snapshot();

// 序列化为 JSON
let json = ctx.to_json()?;

// 反序列化
let restored = AgentContext::from_json(&json)?;
```

### 4. 运行测试

```bash
cd crates/agent
cargo test context
```

### 5. 集成到 AgentLoop

在 `AgentLoop` 初始化时创建 Agent Context：

```rust
// AgentLoop 构造函数中
let context = Arc::new(AgentContext::new(
    execution_id.clone(),
    user_input,
    ContextConfig::default(),
));

let sync_hook = Arc::new(AgentContextSyncHook::new(context.clone()));
```

每轮 turn 通过 hook 自动同步状态到 Context。
