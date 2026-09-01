# LLM 提示词调试工具 — 设计文档

**日期**: 2026-07-16
**状态**: 待实现

## 概述

在 HiveGUI 左侧"工具"菜单下新增"LLM 提示词调试"Tab（位于"数据源"之前），提供三栏布局的 LLM 提示词调试界面：左侧 LLM 设置、中间消息编辑、右侧结果展示。支持真实 API 调用。

## 需求

| # | 需求 | 优先级 |
|---|------|--------|
| 1 | 在工具 Tab 中新增"LLM 提示词调试"Tab，排在"数据源"之前 | P0 |
| 2 | 左侧面板：从 LLM 配置的 Model 列表中下拉选择模型 | P0 |
| 3 | 左侧面板：Temperature / Max Tokens / Top P 参数滑块 | P0 |
| 4 | 中间面板：消息卡片列表，支持 System/User/Assistant 角色 | P0 |
| 5 | 中间面板：消息可增删改、拖拽排序 | P1 |
| 6 | 右侧面板：展示 API 响应结果 | P0 |
| 7 | 真实 API 调用：使用 LLM Provider 配置的 base_url + API Key | P0 |
| 8 | 错误处理：API 调用失败时在结果区显示错误信息 | P0 |

## 架构

### 文件变更

| 文件 | 操作 | 说明 |
|------|------|------|
| `crates/hivegui/src/ui/prompt_debugger.rs` | 新增 | 提示词调试工具主视图 |
| `crates/hivegui/src/ui/utility_view.rs` | 修改 | 新增 Tab，调整顺序 |
| `crates/hivegui/src/ui/mod.rs` | 修改 | 导出新模块 |

### 数据流

```
LlmStore (SQLite)
  ├── list_models()     → 模型下拉列表
  ├── list_providers()  → Provider base_url + category
  └── crypto.decrypt()  → 解密 API Key
        ↓
  reqwest::Client → POST {base_url}/chat/completions
        ↓
  ResultsPanel 展示响应
```

### 关键设计决策

1. **API 调用方式**：直接使用 `reqwest` 发送 OpenAI 兼容格式请求（`POST /v1/chat/completions`），不依赖 `providers` crate 的工厂模式（该模式面向 CLI 配置系统）。从 `LlmStore` 获取 Provider 的 `base_url` 和加密的 `token`，解密后作为 Bearer Token。

2. **消息模型**：
   ```rust
   struct DebugMessage {
       id: u64,
       role: MessageRole,  // System | User | Assistant
       content: String,
   }
   ```

3. **结果展示**：纯文本 + 基础 Markdown 渲染（代码块识别）。首次实现不做完整的 Markdown 渲染引擎，用 `text` 组件展示即可，代码块用等宽字体区分。

4. **异步调用**：使用 `gpui::AsyncContext` 在后台线程执行 HTTP 请求，避免阻塞 UI。

5. **状态管理**：所有状态（消息列表、选中模型、参数、结果）保存在 `PromptDebugger` struct 中，通过 `cx.notify()` 触发重渲染。

### 三栏布局

```
──────────────────────────────────────────────────────────────────┐
│  TabBar: [LLM 提示词调试] [数据源]                                  │
├──────────────┬───────────────────────────┬───────────────────────┤
│  LLM 设置     │  Messages                 │  Results              │
│  (w: 220px)  │  (flex-1)                 │  (w: 320px)           │
│              │                           │                       │
│  模型 [下拉▼] │  + 添加消息                 │  空状态 / 结果         │
│              │                           │                       │
│  Temperature │  ── Message Cards ──      │                       │
│  ──●── 0.70  │  [System] 你是一个...     │                       │
│  精确    创造 │  [User]    你好            │                       │
│              │  [Assistant] (待执行)      │                       │
│  Max Tokens  │                           │                       │
│  ──●── 2048  │  [▷ 执行]                  │                       │
│              │                           │                       │
│  Top P       │                           │                       │
│  ────────●─  │                           │                       │
└──────────────┴───────────────────────────┴───────────────────────
```

### 组件结构

```
PromptDebugger
├── LlmSettingsPanel
│   ├── ModelSelector (Dropdown)
│   ├── TemperatureSlider
│   ├── MaxTokensSlider
│   └── TopPSlider
├── MessageEditor
│   ├── MessageCard[] (可增删改)
│   └── ExecuteButton
└── ResultsPanel
    └── ResponseContent (text)
```

## 测试策略

- 单元测试：消息增删改排序逻辑
- 几何测试：三栏布局尺寸验证
- 集成测试：API 调用 mock 验证（不依赖真实网络）
