# Contracts: HiveGUI 独立运行模式

**Feature**: HiveGUI Standalone Mode  
**Date**: 2026-06-15

## 1. UI Navigation Contract

hivegui 的导航合约定义了所有可访问的面板路由及其层级关系。

### Route Enum

```rust
pub enum AppRoute {
    // === 用户功能 (已有) ===
    Home,                    // 首页（= 仪表盘 Dashboard）
    Conversation,            // AI 对话
    Tools(ToolSeriesKind),   // 工具系列 (Day+1, Hour+1)
    DataSource,              // 数据源管理 (可选高级功能)

    // === 管理功能 (新增) ===
    LockScreen,              // 解锁界面 (应用入口)
    AgentConfig,             // 代理配置管理
    WorkflowEditor,          // 工作流编辑器
    PluginManager,           // 插件管理
    FunctionRegistry,        // 函数/工具/技能注册表
    SensitiveWordFilter,     // 敏感词过滤管理
    GlobalSettings,          // 全局设置 (LLM 提供商、备份/恢复)
    Dashboard,               // 仪表盘（与 Home 等效）
}
```

### Sidebar Navigation Groups

```
┌──────────────┐
│  🏠 Home     │  用户功能区（= Dashboard）
│  💬 对话     │
│  📅 Day+1    │
│  ⏰ Hour+1   │
│  🗄 数据源   │
│──────────────│  ← 分隔线
│  🤖 代理     │  管理功能区
│  🔀 工作流   │
│  🧩 插件     │
│  📦 函数库   │
│  🛡 敏感词   │
│  ⚙ 设置     │
└──────────────┘
注：GameManager 已移除（Session 2026-06-17 澄清）
    Dashboard 通过 Home 进入，无独立导航按钮
```

### Route → View Mapping

| Route | View | Source File |
|-------|------|-------------|
| `Home` | `DashboardView` | `ui/dashboard.rs` (新: Home 即 Dashboard) |
| `Conversation` | `ConversationView` | `ui/conversation.rs` |
| `Tools(DayPlusOne)` | `ToolsSectionView` | `ui/tools_section.rs` |
| `Tools(HourPlusOne)` | `ToolsSectionView` | `ui/tools_section.rs` |
| `DataSource` | `DataSourceView` | `ui/datasource_view.rs` |
| `LockScreen` | `LockScreenView` | `auth/lock_screen.rs` (新增) |
| `AgentConfig` | `AgentConfigView` | `ui/agent_config.rs` (新增) |
| `WorkflowEditor` | `WorkflowEditorView` | `ui/workflow_editor.rs` (新增) |
| `PluginManager` | `PluginManagerView` | `ui/plugin_manager.rs` (新增) |
| `FunctionRegistry` | `FunctionRegistryView` | `ui/function_registry.rs` (新增) |
| `SensitiveWordFilter` | `SensitiveWordFilterView` | `ui/sensitive_word.rs` (新增) |
| `GlobalSettings` | `GlobalSettingsView` | `ui/global_settings.rs` (新增, 含备份/恢复) |
| `Dashboard` | `DashboardView` | `ui/dashboard.rs` (新增，与 Home 等效) |

## 2. Internal Module Interface Contract

### Store (Local Database Manager)

```rust
/// 通用本地数据库管理器 — 所有 SQLite 操作的入口
pub struct Store { /* sqlx::Pool<Sqlite> + Crypto */ }

impl Store {
    /// 打开/创建数据库，执行所有 CREATE TABLE IF NOT EXISTS
    pub async fn new(db_dir: &Path, crypto: Crypto) -> Result<Self>;

    // -- Agent CRUD --
    pub async fn create_agent(&self, req: CreateAgentRequest) -> Result<AgentConfig>;
    pub async fn list_agents(&self) -> Result<Vec<AgentConfig>>;
    pub async fn get_agent(&self, id: i64) -> Result<Option<AgentConfig>>;
    pub async fn update_agent(&self, id: i64, req: UpdateAgentRequest) -> Result<AgentConfig>;
    pub async fn delete_agent(&self, id: i64) -> Result<()>;

    // -- Workflow CRUD --
    pub async fn create_workflow(&self, req: CreateWorkflowRequest) -> Result<WorkflowDefinition>;
    pub async fn list_workflows(&self) -> Result<Vec<WorkflowDefinition>>;
    pub async fn get_workflow(&self, id: i64) -> Result<Option<WorkflowDefinition>>;
    pub async fn update_workflow(&self, id: i64, req: UpdateWorkflowRequest) -> Result<WorkflowDefinition>;
    pub async fn delete_workflow(&self, id: i64) -> Result<()>;

    // -- Function/Tool/Skill CRUD --
    pub async fn create_function(&self, req: CreateFunctionRequest) -> Result<FunctionDef>;
    pub async fn list_functions(&self, function_type: Option<&str>) -> Result<Vec<FunctionDef>>;
    pub async fn delete_function(&self, id: i64) -> Result<()>;

    // -- Plugin CRUD --
    pub async fn register_plugin(&self, req: RegisterPluginRequest) -> Result<Plugin>;
    pub async fn list_plugins(&self) -> Result<Vec<Plugin>>;
    pub async fn delete_plugin(&self, id: i64) -> Result<()>;

    // -- Sensitive Word CRUD --
    pub async fn add_sensitive_word(&self, req: AddSensitiveWordRequest) -> Result<SensitiveWordRule>;
    pub async fn list_sensitive_words(&self) -> Result<Vec<SensitiveWordRule>>;
    pub async fn delete_sensitive_word(&self, id: i64) -> Result<()>;

    // -- LLM Provider CRUD --
    pub async fn add_provider(&self, req: AddProviderRequest) -> Result<LlmProvider>;
    pub async fn list_providers(&self) -> Result<Vec<LlmProvider>>;
    pub async fn delete_provider(&self, id: i64) -> Result<()>;

    // -- Auth --
    pub async fn set_master_password(&self, hash: &str, hint: Option<&str>) -> Result<()>;
    pub async fn verify_master_password(&self, password: &str) -> Result<bool>;
    pub async fn get_password_hint(&self) -> Result<Option<String>>;

    // -- Usage Records --
    pub async fn record_usage(&self, record: UsageRecord) -> Result<()>;
    pub async fn get_dashboard_stats(&self) -> Result<DashboardStats>;

    // -- Backup & Restore (FR-016) --
    pub async fn export_backup(&self, output_path: &Path) -> Result<()>;
    pub async fn import_backup(&self, zip_path: &Path) -> Result<ImportResult>;
}
```

### Backup/Restore Contract (FR-016)

```rust
/// 导入结果：逐项统计合并情况
pub struct ImportResult {
    pub total: usize,       // 备份中的总记录数
    pub imported: usize,    // 成功导入数
    pub updated: usize,     // updated_at 更新覆盖数
    pub skipped: usize,     // 因用户选择跳过的数目
    pub conflicts: usize,   // 需要手动选择的冲突数（updated_at 相同）
}

/// 导入冲突项
pub struct ImportConflict {
    pub entity_type: String,   // 实体类型: "agent", "workflow", etc.
    pub name: String,
    pub existing_updated_at: String,
    pub import_updated_at: String,
}
```

导入冲突解决策略：
1. 若 `import.updated_at > existing.updated_at` → 自动覆盖
2. 若 `import.updated_at < existing.updated_at` → 自动跳过
3. 若 `import.updated_at == existing.updated_at` → 弹出对话框，用户逐项选择"覆盖"或"跳过"

### Crypto Service

```rust
/// 加密服务 — 主密码派生密钥 + chacha20poly1305 加解密
pub struct Crypto { /* chacha20poly1305 key */ }

impl Crypto {
    /// 从用户主密码派生加密密钥 (argon2id KDF)
    pub fn derive_from_password(password: &str, salt: &[u8]) -> Result<Self>;

    /// 加密敏感数据 (API keys, passwords)
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<(Vec<u8>, Vec<u8>)>;  // (ciphertext, nonce)

    /// 解密敏感数据
    pub fn decrypt(&self, ciphertext: &[u8], nonce: &[u8]) -> Result<Vec<u8>>;
}
```

### Runtime Integration

```rust
/// hivegui 本地运行时 — 集成 hiveweb runtime 组件
pub struct LocalRuntime {
    pub capabilities: Arc<CapabilityRegistry>,     // from hiveweb
    pub pool: Arc<InstancePool>,                   // from hiveweb
    pub invoker: Arc<Invoker>,                     // from hiveweb
    pub workflows: Arc<WorkflowExecutor>,          // from hiveweb
    pub builtins: BuiltinRegistry,                  // hiveweb builtins adapted
    pub provider_registry: ProviderRegistry,        // local LLM providers
}

impl LocalRuntime {
    /// 初始化本地运行时
    pub async fn new(store: Store, crypto: Crypto) -> Result<Self>;

    /// 执行工作流
    pub async fn execute_workflow(
        &self,
        workflow_id: i64,
        input: Value,
    ) -> Result<WorkflowResult>;

    /// 执行单个函数
    pub async fn invoke_function(
        &self,
        function_name: &str,
        input: Value,
    ) -> Result<Value>;
}
```

## 3. LLM Provider Communication Contract

hivegui 通过 HTTP 与 LLM 提供商通信，使用 OpenAI-compatible API 格式。

### Request Format (OpenAI-compatible Chat Completions)

```
POST {provider.base_url}/v1/chat/completions
Authorization: Bearer {decrypted_api_key}
Content-Type: application/json

{
  "model": "{agent.model}",
  "messages": [
    {"role": "system", "content": "{agent.system_instructions}"},
    {"role": "user", "content": "{user_message}"}
  ],
  "temperature": {agent.temperature},
  "stream": true
}
```

### Response Format (SSE Stream)

```
data: {"id":"...","object":"chat.completion.chunk","choices":[{"delta":{"content":"Hello"}}]}

data: {"id":"...","object":"chat.completion.chunk","choices":[{"delta":{"content":" world"}}]}

data: [DONE]
```

### Error Handling

| HTTP Status | 行为 |
|-------------|------|
| 401 | 提示 API key 无效，引导用户检查提供商配置 |
| 429 | 显示速率限制提示，建议稍后重试 |
| 5xx | 显示提供商服务异常，建议检查 base_url 或稍后重试 |
| Timeout | 显示请求超时，建议检查网络或增加超时设置 |
