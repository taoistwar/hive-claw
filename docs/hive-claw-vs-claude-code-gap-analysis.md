# Hive-Claw vs Claude Code 功能差距分析报告（修正版）

> 生成时间: 2025-06-12
> 基线: Hive-Claw v0.1.0 (20cbc9e) vs Claude Code v2.1.x
> 修正: 2025-06-12 — 基于 dev-guids.md 修正架构理解

---

## 〇、架构差异说明

**核心区别：配置驱动方式不同**

| 维度 | Claude Code | Hive-Claw |
|------|-------------|-----------|
| 配置方式 | 文件驱动（CLAUDE.md、settings.json） | **数据库驱动**（通过 `/api/assistant` 入口） |
| 管理界面 | CLI + 文件编辑 | **Web 管理中心**（Admin Center） |
| 动态性 | 修改文件需重启会话 | **数据库热更新**，无需重启 |
| 多租户 | 单用户本地配置 | **用户中心 + agent_permissions 多租户** |

Hive-Claw 的所有组件（agent/skill/tool/hook/workflow/function/capability）都存储在数据库表中，通过 Web 管理中心配置。这与 Claude Code 的文件驱动是**不同的设计哲学**，各有优劣。

---

## 一、概述

**Hive-Claw 当前定位**：基于 Rust 的多代理 AI 平台，数据库驱动配置，包含 Web 管理中心、WASM 插件运行时、桌面 GUI 和 CLI，支持多 LLM 提供商。

**Claude Code 当前定位**：终端/IDE/桌面/Web 多端 agentic coding 工具，文件驱动配置，强调开发者工作流集成。

**已修正**：原报告将 Hive-Claw 的数据库驱动配置误判为功能缺失。Hive-Claw 已具备 hooks、skills、tools、permissions、plugins 等核心能力，差距主要在于**能力粒度和生态集成**，而非功能有无。

---

## 二、核心功能模块差距（修正）

### 2.1 配置管理方式

| 能力 | Claude Code | Hive-Claw | 差距 |
|------|-------------|-----------|------|
| 配置存储 | 文件系统 | 数据库 | **架构不同（非差距）** |
| 管理方式 | CLI + 编辑器 | Web Admin Center | **Hive-Claw 更优** |
| 热更新 | 需重启会话 | 数据库热更新 | **Hive-Claw 更优** |
| 多租户 | 单用户 | agent_permissions 多租户 | **Hive-Claw 更优** |
| 版本控制 | Git 管理文件 | 数据库快照（无 Git） | **Claude Code 更优** |
| 离线使用 | 文件可离线编辑 | 需要数据库连接 | **Claude Code 更优** |
| 团队协作 | Git PR 审查 | Web UI 共享 | **各有优劣** |

**结论**：配置管理方式是架构选择差异，Hive-Claw 的数据库驱动在多租户和热更新方面有优势，但在版本控制和离线使用方面不如 Claude Code。

### 2.2 Hook 生命周期系统（已修正）

| 能力 | Claude Code | Hive-Claw | 差距 |
|------|-------------|-----------|------|
| 生命周期事件 | 20+ 种 | 基础事件（before/after） | **粒度差距** |
| Hook 类型 | 5种: command/http/mcp_tool/prompt/agent | 3种: function/workflow/webhook | **类型差距** |
| 工具级拦截 | PreToolUse/PostToolUse 精确拦截 | 无 | **缺失** |
| 异步执行 | 支持 async hook | 无 | **缺失** |
| Matcher 模式 | glob/regex 工具名匹配 | 无 | **缺失** |
| 条件执行 | `if` 字段权限规则语法 | 无 | **缺失** |
| 配置存储 | settings.json 文件 | `agent_hooks` 数据库表 | **Hive-Claw 更灵活** |

**详细描述**：

Hive-Claw 已有的 Hook 能力（`agent_hooks` 表）：
- 3种 Hook 类型：function（内置函数）、workflow（工作流）、webhook（HTTP 回调）
- Agent 绑定：每个 agent 可配置多个 hook
- 数据库驱动：通过 Web 管理中心配置

Claude Code 多出的能力：
- **PreToolUse/PostToolUse**：工具调用前/后精确拦截（可阻止危险操作）
- **20+ 生命周期事件**：SessionStart、Stop、SubagentStart/Stop、FileChanged 等
- **5种 Hook 类型**：command（shell）、http、mcp_tool、prompt（LLM 评估）、agent（多轮子代理）
- **Matcher 模式匹配**：按工具名精确匹配（如 `Bash`、`Edit|Write`、`mcp__.*`）
- **条件执行**：`if` 字段支持权限规则语法（如 `Bash(rm *)`）
- **异步执行**：`async: true` 后台运行不阻塞

### 2.3 Skills 技能系统（已修正）

| 能力 | Claude Code | Hive-Claw | 差距 |
|------|-------------|-----------|------|
| 存储方式 | Markdown 文件 | `skills` 数据库表 | **架构不同** |
| 调用方式 | 模型自动 + `/name` 手动 | `invoke_workflow` / `invoke_function` | **已存在** |
| 参数传递 | `$ARGUMENTS` / `$0` / `$1` | 基础参数 | **粒度差距** |
| 动态注入 | `` !`command` `` shell 预执行 | 无 | **缺失** |
| 子代理运行 | `context: fork` | 无 | **缺失** |
| 工具权限 | `allowed-tools` / `disallowed-tools` | `agent_permissions` 检查 | **已存在（不同机制）** |
| 模型切换 | `model` frontmatter | 无 | **缺失** |
| 作用域限制 | `paths` glob 匹配 | 无 | **缺失** |
| 支持文件 | 模板/脚本/示例目录 | 无 | **缺失** |

**详细描述**：

Hive-Claw 已有的 Skill 能力（`skills` 表）：
- 每个 skill 通过 `invoke_workflow` 或 `invoke_function` 调用工作流/函数
- 每个 skill 需要对应若干个 capability 才允许执行
- 数据库驱动：通过 Web 管理中心配置

Claude Code 多出的能力：
- **动态上下文注入**：`` !`command` `` 语法在 skill 加载前执行 shell 命令
- **子代理运行**：`context: fork` 在隔离子代理中执行 skill
- **模型切换**：skill 级别指定使用不同模型
- **路径作用域**：`paths` glob 匹配，仅在操作特定文件时加载
- **支持文件**：skill 目录可包含模板、脚本、示例等辅助文件

### 2.4 Tools 工具系统（已修正）

| 能力 | Claude Code | Hive-Claw | 差距 |
|------|-------------|-----------|------|
| 工具定义 | 内置 + MCP 扩展 | `tools` 表（包装 function/workflow） | **已存在** |
| 工具注册 | 自动发现 | `agent_tools` 关联表 | **已存在** |
| 权限控制 | `allowed-tools` / `disallowed-tools` | `agent_permissions` 检查 | **已存在（不同机制）** |
| MCP 扩展 | 标准 MCP 协议 | 无 | **缺失** |
| 工具类型 | 30+ 种内置工具 | 函数节点 + 工作流节点 | **粒度差距** |

**Claude Code 内置工具清单**（Hive-Claw 缺少的）：
- 文件操作：Read、Write、Edit、Glob、Grep
- 终端：Bash（完整 shell 访问）
- 网络：WebFetch（URL 内容获取）
- 代理：Agent（子代理生成）、SendMessage（代理间通信）
- 任务：Task（任务管理工具）
- 会话：Memory（BM25 语义搜索）、History（原始对话搜索）
- 问题：AskUserQuestion（用户交互）
- 规划：EnterPlanMode、ExitPlanMode
- 技能：Skill（技能调用）
- 等等

### 2.5 Plugins 插件系统（已修正）

| 能力 | Claude Code | Hive-Claw | 差距 |
|------|-------------|-----------|------|
| 定义方式 | 文件目录 | 数据库 `plugins` 表 | **架构不同** |
| 运行时 | Node.js / Shell | **extism WASM** | **Hive-Claw 更优** |
| 安全模型 | 文件权限 | **7种能力沙箱** | **Hive-Claw 更优** |
| 实例管理 | 无 | **实例池化** | **Hive-Claw 更优** |
| 组合能力 | skills/agents/hooks/mcp 组合 | 单一 WASM 模块 | **Claude Code 更优** |
| 分发方式 | Marketplace（官方+社区） | ClawHub（基础） | **Claude Code 更优** |
| 热更新 | `/reload-plugins` | 数据库热更新 | **Hive-Claw 更优** |

**结论**：Hive-Claw 的 WASM 插件在安全性和企业级部署方面有优势，但缺少 Claude Code 的组合能力和生态分发。这是**架构选择差异**，不是功能缺失。

### 2.6 Subagent 自定义子代理

| 能力 | Claude Code | Hive-Claw | 差距 |
|------|-------------|-----------|------|
| 定义方式 | Markdown 文件 + YAML frontmatter | 代码定义 | **架构不同** |
| 配置存储 | `.claude/agents/` 目录 | `agents` 数据库表 | **Hive-Claw 更灵活** |
| 工具集控制 | tools / disallowedTools | `agent_tools` 关联表 | **已存在（不同机制）** |
| 模型选择 | sonnet/opus/haiku/fable/inherit | 继承主会话 | **缺失** |
| 权限模式 | 6种模式 | `agent_permissions` | **已存在（不同机制）** |
| 工作树隔离 | `isolation: worktree` | 无 | **缺失** |
| 持久记忆 | `memory: user/project/local` | MemoryStore | **部分存在** |
| 后台运行 | `background: true` | SubagentManager | **已存在** |
| 推理深度 | `effort` 控制 | 无 | **缺失** |
| 会话恢复 | SendMessage 恢复 | 无 | **缺失** |
| Fork 继承 | 完整对话历史 | 无 | **缺失** |

### 2.7 Agent Teams 多代理协作

| 能力 | Claude Code | Hive-Claw | 差距 |
|------|-------------|-----------|------|
| 协调模式 | Lead + Teammates 独立实例 | Workflow DAG | **架构不同** |
| 任务列表 | 共享任务列表 + 依赖关系 | Workflow DAG 节点 | **已有类似机制** |
| 代理间通信 | 直接消息 + 邮箱系统 | 无 | **缺失** |
| 分屏显示 | tmux/iTerm2 分屏 | 无 | **缺失** |
| 计划审批 | Teammate 提交 → Lead 审批 | 无 | **缺失** |
| 文件锁定 | 任务竞争防止 | 无 | **缺失** |

**结论**：Hive-Claw 的 Workflow DAG 可以实现类似 Agent Teams 的任务分解，但缺少代理间实时通信和协作能力。

---

## 三、缺失的工具和能力（修正）

### 3.1 MCP (Model Context Protocol) 标准协议

| 能力 | Claude Code | Hive-Claw | 差距 |
|------|-------------|-----------|------|
| 传输协议 | stdio/http/sse/websocket | 无 | **完全缺失** |
| 配置方式 | .mcp.json + settings.json | 无 | 缺失 |
| 管理策略 | 企业级 MCP 配置 | 无 | 缺失 |
| 子代理作用域 | 子代理独立 MCP 服务器 | 无 | 缺失 |
| 插件 MCP | 插件内嵌 MCP 服务器 | 无 | 缺失 |
| Channel 通知 | Telegram/Discord/iMessage | 无 | 缺失 |

**重要性**：MCP 是 AI 工具连接外部数据源的开放标准，缺失则无法接入主流工具链。这是**最关键的差距**。

### 3.2 工具级拦截能力

| 能力 | Claude Code | Hive-Claw | 差距 |
|------|-------------|-----------|------|
| PreToolUse 拦截 | 工具调用前可阻止 | 无 | **缺失** |
| PostToolUse 后处理 | 工具调用后可触发 | 无 | **缺失** |
| 条件执行 | `if` 字段匹配工具参数 | 无 | **缺失** |
| Matcher 模式 | 工具名精确匹配 | 无 | **缺失** |

**重要性**：工具级拦截是安全基础，可在危险操作（如 `rm -rf`）执行前阻止。

### 3.3 IDE 集成

| 能力 | Claude Code | Hive-Claw | 差距 |
|------|-------------|-----------|------|
| VS Code | 扩展：内联 diff、@-mentions | 无 | **完全缺失** |
| JetBrains | 插件：交互式 diff 查看 | 无 | **缺失** |
| Cursor | 扩展 | 无 | **缺失** |
| Chrome | 浏览器集成：网页自动化 | 无 | **缺失** |
| Desktop GUI | 独立桌面应用 | gpui GUI | **已存在** |

### 3.4 CI/CD 集成

| 能力 | Claude Code | Hive-Claw | 差距 |
|------|-------------|-----------|------|
| GitHub Actions | 原生支持 | 无 | **缺失** |
| GitLab CI/CD | 原生支持 | 无 | **缺失** |
| 非交互模式 | `claude -p "query"` | CLI 存在 | **已存在** |
| 管道化 | `cat file \| claude -p` | 无 | **缺失** |

### 3.5 远程和多端协作

| 能力 | Claude Code | Hive-Claw | 差距 |
|------|-------------|-----------|------|
| Remote Control | 从 Claude.ai/手机控制本地终端 | 无 | **缺失** |
| Web 界面 | 浏览器中运行 | web-user | **已存在** |
| iOS App | 移动端编程 | 无 | **缺失** |
| Teleport | 会话设备间迁移 | 无 | **缺失** |

### 3.6 动态上下文注入

| 能力 | Claude Code | Hive-Claw | 差距 |
|------|-------------|-----------|------|
| Shell 预执行 | `` !`command` `` 语法 | 无 | **缺失** |
| 多行块 | `` ```! `` 代码块 | 无 | **缺失** |
| 运行时数据 | skill 加载时注入实际数据 | 静态模板 | **缺失** |

### 3.7 Plan Mode 只读规划模式

| 能力 | Claude Code | Hive-Claw | 差距 |
|------|-------------|-----------|------|
| 只读分析 | 进入 plan mode 后只允许读取 | 无 | **缺失** |
| 计划输出 | 结构化实施计划 | 无 | **缺失** |
| 审批流程 | 用户批准后执行 | 无 | **缺失** |

### 3.8 Context Window 管理

| 能力 | Claude Code | Hive-Claw | 差距 |
|------|-------------|-----------|------|
| 自动压缩 | 95% 容量触发 | autocompact | **已存在** |
| 手动压缩 | `/compact` 命令 | 无 | **缺失** |
| 可视化 | 上下文窗口可视化 | 无 | **缺失** |
| 压缩后重注入 | CLAUDE.md + 技能内容保留 | 无 | **缺失** |

### 3.9 调度系统

| 能力 | Claude Code | Hive-Claw | 差距 |
|------|-------------|-----------|------|
| Routines | Anthropic 托管定时任务 | cron crate | **架构不同** |
| 本地定时 | Desktop scheduled tasks | cron crate | **已存在** |
| 循环轮询 | `/loop` 会话内循环 | 无 | **缺失** |
| 事件触发 | API 调用、GitHub 事件 | 无 | **缺失** |

### 3.10 调试和诊断

| 能力 | Claude Code | Hive-Claw | 差距 |
|------|-------------|-----------|------|
| 调试模式 | `--debug` + 日志文件 | 无 | **缺失** |
| 配置诊断 | `/doctor` | 无 | **缺失** |
| 上下文查看 | `/context` | 无 | **缺失** |
| 详细日志 | `DEBUG_LOG_LEVEL=verbose` | 无 | **缺失** |

### 3.11 模型配置增强

| 能力 | Claude Code | Hive-Claw | 差距 |
|------|-------------|-----------|------|
| Effort levels | low/medium/high/xhigh/max | 无 | **缺失** |
| Fallback chain | 主模型不可用自动切换 | Fallback Provider | **已存在** |
| Advisor | 服务器端代码顾问模型 | 无 | **缺失** |
| Ultrathink | 深度推理模式 | 无 | **缺失** |
| Bare mode | 最小模式跳过自动发现 | 无 | **缺失** |

### 3.12 Fork 会话

| 能力 | Claude Code | Hive-Claw | 差距 |
|------|-------------|-----------|------|
| 会话分叉 | `/fork "directive"` | 无 | **缺失** |
| 历史继承 | 完整对话历史 | 无 | **缺失** |
| Cache 共享 | 共享 prompt cache | 无 | **缺失** |
| 并行观察 | 多 fork 同时监控 | 无 | **缺失** |

---

## 四、Hive-Claw 已有优势（修正）

| 能力 | Hive-Claw | Claude Code |
|------|-----------|-------------|
| **数据库驱动配置** | 所有组件数据库管理，热更新 | 文件驱动，需重启 |
| **Web 管理中心** | 完整 REST API + Web UI | 无 |
| **多租户** | agent_permissions 用户中心 | 单用户本地 |
| **Workflow DAG** | 完整 DAG 执行器，支持并行层 | 无 |
| **WASM 插件系统** | extism WASM + 7种能力沙箱 + 实例池化 | 文件目录插件 |
| **多 LLM 提供商** | OpenAI/Anthropic/Azure/Bedrock/Copilot | 仅 Anthropic |
| **Desktop GUI** | 基于 gpui 的原生桌面应用 | 独立桌面应用 |
| **Game Management** | 游戏列表、推荐、策略过滤 | 无 |
| **Sensitive Word Filter** | 内存过滤引擎（regex/aho-corasick） | 无 |
| **External API** | MD5 签名认证的外部调用 API | 无 |
| **审计日志** | 操作审计追踪 | 无 |
| **Fallback Provider** | 模型不可用自动切换 | 有 |
| **Autocompact** | 上下文自动压缩 | 有 |

### 4.1 插件系统对比

| 维度 | Claude Code 插件 | Hive-Claw 插件 |
|------|-----------------|----------------|
| **定义方式** | 文件目录（skills/agents/hooks/mcp/lsp） | 数据库 `plugins` 表 + WASM 模块 |
| **运行时** | Node.js / Shell 脚本 | **extism WASM 沙箱** |
| **安全模型** | 文件系统权限 | **7种能力类型隔离**（db/fs/llm/network/s3/secret/utility） |
| **实例管理** | 无 | **实例池化**（pool.rs），可配置并发上限 |
| **热更新** | 需 `/reload-plugins` | **数据库热更新** |
| **分发方式** | Marketplace（官方+社区） | **ClawHub**（基础） |
| **组合能力** | 单插件包含 skills/agents/hooks/mcp | **单一 WASM 模块** |
| **配置存储** | 文件系统 | **数据库 `plugins` 表** |

**Hive-Claw 插件系统优势**：
- WASM 沙箱提供更强的安全隔离（7种能力类型）
- 实例池化支持高并发场景
- 数据库驱动支持热更新和多租户
- 适合企业级部署和安全敏感场景

**Claude Code 插件系统优势**：
- 组合能力强（单插件包含多种组件）
- Marketplace 生态成熟
- 开发门槛低（Markdown + Shell）
- 社区贡献活跃

**结论**：Hive-Claw 的 WASM 插件在安全性和企业级部署方面有优势，但缺少 Claude Code 的组合能力和生态分发。这是**架构选择差异**，不是功能缺失。

---

## 五、优先级建议（修正）

### P0 - 必须实现（生态互通）

| 功能 | 理由 | 预估工作量 |
|------|------|-----------|
| MCP 标准协议 | 生态互通基础，缺失则无法接入主流工具链 | 2-3 周 |

### P1 - 高优先级（安全 + 可扩展性）

| 功能 | 理由 | 预估工作量 |
|------|------|-----------|
| 工具级拦截（PreToolUse/PostToolUse） | 安全基础，可在危险操作前阻止 | 1-2 周 |
| Hook 系统增强（Matcher/条件执行） | 可扩展性核心，支撑自动化工作流 | 1-2 周 |
| 动态上下文注入（`` !`command` ``） | 技能从静态模板升级为可编程工作流 | 1 周 |
| Plan Mode | 避免理解不充分就修改代码 | 1 周 |

### P2 - 中优先级（开发者体验）

| 功能 | 理由 | 预估工作量 |
|------|------|-----------|
| VS Code 扩展 | 开发者体验，扩大用户群 | 4-6 周 |
| CI/CD 管道集成 | 自动化运维能力 | 1-2 周 |
| Effort 控制 | 推理深度调节 | 1 周 |
| 调试诊断工具 | 开发者体验 | 1-2 周 |

### P3 - 低优先级（锦上添花）

| 功能 | 理由 | 预估工作量 |
|------|------|-----------|
| 远程控制/多端 | 协作灵活性 | 2-3 周 |
| 调度系统增强 | 定时自动化 | 1-2 周 |
| IDE 集成（JetBrains） | 扩大用户群 | 2-3 周 |
| Fork 会话 | 高级工作流 | 2 周 |
| Agent Teams 实时协作 | 复杂任务并行分解 | 3-4 周 |

---

## 六、实施路线图建议（修正）

### Phase 1: 生态互通（Month 1-2）
- MCP 标准协议支持

### Phase 2: 安全增强（Month 2-3）
- 工具级拦截（PreToolUse/PostToolUse）
- Hook Matcher 模式匹配
- Hook 条件执行

### Phase 3: 核心能力（Month 3-5）
- 动态上下文注入
- Plan Mode
- Effort 控制
- 调试诊断工具

### Phase 4: 生态集成（Month 5-8）
- VS Code 扩展
- CI/CD 管道集成

### Phase 5: 高级特性（Month 8+）
- 远程控制/多端
- Fork 会话
- Agent Teams 实时协作

---

## 七、结论（修正）

Hive-Claw 与 Claude Code 的差距**不是功能有无，而是能力粒度和生态集成**。

**Hive-Claw 已有核心能力**：
- 数据库驱动配置（Web 管理中心）✅
- Hooks（agent_hooks 表，3种类型）✅
- Skills（skills 表，invoke_workflow/invoke_function）✅
- Tools（tools 表，包装 function/workflow）✅
- Permissions（agent_permissions 表）✅
- Plugins（plugins 表 + extism WASM 沙箱，7种能力类型）✅
- Workflow DAG ✅
- WASM 插件沙箱 ✅
- 多 LLM 提供商 ✅
- 多租户 ✅

**关键差距**：
1. **MCP 标准协议**（P0）— 生态互通基础
2. **工具级拦截**（P1）— 安全基础
3. **Hook 增强**（P1）— 可扩展性
4. **IDE 集成**（P2）— 开发者体验
5. **CI/CD 集成**（P2）— 自动化运维

**建议**：优先实现 MCP 协议（P0），然后增强安全和可扩展性（P1），最后补齐 IDE 集成和高级特性（P2/P3）。预计关键差距补齐需要 4-6 个月。
