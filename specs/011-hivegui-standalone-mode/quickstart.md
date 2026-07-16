# Quickstart: HiveGUI 独立桌面管理工具开发

**Feature**: HiveGUI Standalone Mode  
**Date**: 2026-07-02 (Updated)

## 开发环境准备

### 前置依赖

```bash
# Rust toolchain (stable)
rustup default stable
rustup component add clippy rustfmt

# SQLite (编译 sqlx 需要)
# Ubuntu/Debian: sudo apt install libsqlite3-dev
# macOS: 系统自带
# Windows: 不需要额外安装（sqlx 使用 bundled feature）
```

### 克隆与构建

```bash
git clone <repo-url>
cd hive-claw-worktree
git checkout 260517-hivegui-standalone-mode

# 构建 hivegui
cargo build -p hivegui

# 运行（默认配置）
cargo run -p hivegui
```

### 环境变量

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `HIVEGUI_LOG_LEVEL` | `info` | 日志级别: trace, debug, info, warn, error |
| `HIVEGUI_LOG_DIR` | `$XDG_DATA_HOME/hivegui/logs` | 日志目录 |
| `HIVEGUI_HEADLESS` | (未设置) | 设为 `1` 启用无头模式（CI 测试用） |
| `HIVEGUI_DB_DIR` | `$XDG_DATA_HOME/hivegui` | SQLite 数据库目录 |

## 项目结构

```
crates/hivegui/src/
├── main.rs              # 入口: Tokio runtime init → gpui app launch
├── lib.rs               # 公共导出
├── config.rs            # 环境变量配置解析
├── logging.rs           # tracing 日志初始化
├── version.rs           # 版本号
├── datasource/          # 数据存储层
│   ├── mod.rs
│   ├── store.rs         # SQLite Store (data_sources, global_configs)
│   ├── llm_store.rs     # LLM Store (models, llm_presets, llm_providers)
│   ├── entity_store.rs  # [新增] 实体 Store (tags, categories, capabilities, plugins, functions, workflows, tools, skills, agents)
│   ├── crypto.rs        # chacha20poly1305 加解密
│   └── mysql_client.rs  # MySQL 连接测试
└── ui/                  # UI 组件
    ├── mod.rs
    ├── app.rs           # AppRoute 枚举 + RootView
    ├── sidebar_nav.rs   # 侧边栏导航
    ├── home.rs          # 首页视图
    ├── datasource_view.rs    # 数据源列表
    ├── datasource_form.rs    # 数据源表单
    ├── global_config.rs      # 全局配置管理
    ├── llm_config.rs         # LLM 配置管理
    ├── tag_view.rs           # [新增] 标签管理
    ├── category_view.rs      # [新增] 分类管理（树形视图）
    ├── capability_view.rs    # [新增] 能力管理
    ├── plugin_view.rs        # [新增] 插件管理
    ├── function_view.rs      # [新增] 函数管理
    ├── workflow_view.rs      # [新增] 工作流管理
    ├── tool_view.rs          # [新增] 工具管理
    ├── skill_view.rs         # [新增] 技能管理
    └── agent_view.rs         # [新增] Agent 管理
```

## 核心流程

### 1. 应用启动流程

```
main()
  ├── Config::from_env()           # 解析环境变量
  ├── logging::init()              # 初始化 tracing
  ├── Store::new(db_dir)           # 打开/创建 SQLite
  │   └── 执行所有 CREATE TABLE IF NOT EXISTS
  └── gpui::Application::new() → 进入主界面
```

### 2. 实体管理 CRUD 流程

```
用户打开管理面板
  ├── 加载实体列表（分页 + 搜索）
  ├── 点击"添加" → 显示表单
  ├── 填写表单 → 保存 → 插入数据库
  ├── 点击"编辑" → 显示表单（预填充）
  ├── 修改表单 → 保存 → 更新数据库
  └── 点击"删除" → 确认 → 删除数据库记录
```

### 3. Category 树形视图

```
加载所有 categories
  ├── 构建 parent_id 树形结构
  ├── 按层级排序（父 → 子）
  └── 渲染缩进列表（每级缩进 2 空格）
```

## 测试

```bash
# 运行所有 hivegui 测试
cargo test -p hivegui

# 运行特定模块测试
cargo test -p hivegui --lib -- datasource::store      # Store CRUD 测试
cargo test -p hivegui --lib -- datasource::entity_store  # 实体 CRUD 测试
cargo test -p hivegui --lib -- datasource::crypto     # 加密模块测试

# 无头模式冒烟测试
HIVEGUI_HEADLESS=1 cargo run -p hivegui
```

## 调试

```bash
# 启用 debug 日志
HIVEGUI_LOG_LEVEL=debug cargo run -p hivegui

# 日志文件位置
tail -f $HOME/.local/share/hivegui/logs/hivegui.log

# 检查 SQLite 数据库
sqlite3 $HOME/.local/share/hivegui/hivegui.db ".tables"
sqlite3 $HOME/.local/share/hivegui/hivegui.db "SELECT * FROM tags;"
```

## 开发检查清单

- [ ] 新增实体表创建（entity_store.rs）
- [ ] 9 个实体的 CRUD 方法实现
- [ ] 9 个 UI 视图文件创建
- [ ] AppRoute 枚举扩展（9 个新路由）
- [ ] SidebarNav 扩展（9 个新导航按钮）
- [ ] identifier 唯一性验证
- [ ] Category 树形视图实现
- [ ] Plugin 软删除支持
- [ ] Tool 的 CHECK 约束验证
- [ ] 分页 + 搜索功能实现
