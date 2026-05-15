# 大数据离线分析 AI Agent - 技术设计文档

**版本**: 1.0  
**日期**: 2026-05-15  
**状态**: 草稿

---

## 一、架构概述

### 1.1 系统架构图

```
┌─────────────────────────────────────────────────────────────────┐
│                        PC GUI (Rust + gpui)                      │
│  ┌─────────────┐ ┌─────────────┐ ┌─────────────┐ ┌───────────┐ │
│  │ 创建向导    │ │ 任务管理    │ │ DAG 可视化  │ │SQL 编辑器 │ │
│  └─────────────┘ └─────────────┘ └─────────────┘ └───────────┘ │
│  ┌─────────────┐ ┌─────────────┐ ┌─────────────┐ ┌───────────┐ │
│  │ 执行历史    │ │ 日志查看    │ │ 数据预览    │ │ Git 集成  │ │
│  └─────────────┘ └─────────────┘ └─────────────┘ └───────────┘ │
└─────────────────────────────────────────────────────────────────┘
                                │
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│                        应用服务层                                 │
│  ┌─────────────┐ ┌─────────────┐ ┌─────────────┐ ┌───────────┐ │
│  │ 任务生成器  │ │ 元数据同步  │ │ AI 服务代理 │ │ Git 服务  │ │
│  └─────────────┘ └─────────────┘ └─────────────┘ └───────────┘ │
│  ┌─────────────┐ ┌─────────────┐ ┌─────────────┐ ┌───────────┐ │
│  │ Azkaban API │ │ 数据质量    │ │ 审计日志    │ │ 更新服务  │ │
│  └─────────────┘ └─────────────┘ └─────────────┘ └───────────┘ │
└─────────────────────────────────────────────────────────────────┘
                                │
        ┌───────────────────────┼───────────────────────┐
        ▼                       ▼                       ▼
┌───────────────┐      ┌───────────────┐     ┌───────────────┐
│ Hive Cluster  │      │ MySQL         │     │ Azkaban       │
│ (Hive 3.x)    │      │ (Metastore)   │     │ (3.x)         │
└───────────────┘      └───────────────┘     └───────────────┘
        │                       │                       │
        ▼                       ▼                       ▼
┌───────────────┐      ┌───────────────┐     ┌───────────────┐
│ LDAP          │      │ GPT-4 API     │     │ Git Remote    │
│ (认证)        │      │ (云端)        │     │ (代码仓库)    │
└───────────────┘      └───────────────┘     └───────────────┘
```

---

## 二、模块设计

### 2.1 GUI 模块（Rust + gpui）

#### 2.1.1 布局设计

**整体布局**: 上下布局（顶部导航 + 下方内容区 + 底部状态栏）

```
┌─────────────────────────────────────────────────────────────────┐
│ Logo | 任务 | 元数据 | 监控 | 设置 |  [搜索框]  | [用户] [通知] │
├─────────────────────────────────────────────────────────────────┤
│                        主内容区                                  │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                                                            │  │
│  │                    视图内容                                │  │
│  │                                                            │  │
│  └───────────────────────────────────────────────────────────┘  │
├─────────────────────────────────────────────────────────────────┤
│ 状态栏 | 当前任务 | 连接状态 | 缓存状态 | 版本 v1.0             │
└─────────────────────────────────────────────────────────────────┘
```

**窗口管理**:
- 最小窗口限制：1280x800
- 支持自由调整大小
- 支持多显示器
- 标签 + 窗口混合模式

**主题**: 跟随系统自动切换（深色/浅色）

#### 2.1.2 模块结构
```
src/gui/
├── app.rs                  # 应用入口和主窗口
├── layout/
│   ├── header.rs           # 顶部导航栏
│   ├── footer.rs           # 底部状态栏
│   └── workspace.rs        # 工作区（多标签管理）
├── views/
│   ├── wizard.rs           # 创建向导视图（7 步）
│   ├── task_list.rs        # 任务列表视图
│   ├── dag_view.rs         # DAG 可视化视图
│   ├── sql_editor.rs       # SQL 编辑器视图
│   ├── data_preview.rs     # 数据预览视图
│   ├── logs.rs             # 日志查看视图
│   ├── history.rs          # 执行历史视图
│   ├── metadata.rs         # 元数据管理视图
│   └── settings.rs         # 设置中心视图
├── components/
│   ├── button.rs           # 按钮组件
│   ├── input.rs            # 输入框组件
│   ├── table.rs            # 表格组件（支持排序/筛选/分页）
│   ├── modal.rs            # 弹窗组件
│   ├── toast.rs            # Toast 通知组件
│   ├── progress.rs         # 进度条组件
│   ├── skeleton.rs         # 骨架屏组件
│   ├── tree.rs             # 树形选择器组件
│   └── editor.rs           # 代码编辑器组件
├── styles/
│   ├── theme.rs            # 主题样式（深色/浅色）
│   ├── colors.rs           # 颜色定义
│   └── typography.rs       # 字体排版
├── actions/                # 快捷键定义
│   └── shortcuts.rs
└── notifications/          # 通知系统
    └── manager.rs
```

#### 2.1.3 创建向导（7 步）

| 步骤 | 功能 | 交互方式 |
|------|------|----------|
| 1 | 模板选择 | 7 大模板卡片选择（单表聚合、多表关联、增量同步、全量同步、去重清洗、SCD、指标计算） |
| 2 | 表选择 | 树形结构（数据库→表）+ 搜索建议 + 最近使用 + 收藏表 |
| 3 | 字段映射 | 源字段→目标字段拖拽映射，支持添加/删除映射关系 |
| 4 | 分区配置 | 分区字段选择（dt/hour）、同步方式（增量/全量）、分区值模板 |
| 5 | 调度配置 | 调度时间设置、依赖任务选择、资源队列选择 |
| 6 | 自然语言 | 自然语言描述输入框，AI 辅助生成按钮 |
| 7 | 预览确认 | 任务信息汇总、SQL 预览、确认创建 |

#### 2.1.4 任务列表功能

| 功能 | 描述 |
|------|------|
| 排序 | 按名称/状态/最后执行时间/下次执行时间排序 |
| 过滤 | 按状态（成功/失败/运行/等待）、模板类型、时间范围过滤 |
| 搜索 | 关键词搜索任务名、描述 |
| 视图切换 | 列表视图/网格视图切换 |
| 批量操作 | 批量执行、批量删除、批量导出 |
| 自定义列 | 用户自定义显示列 |
| 导出列表 | 导出任务列表为 CSV/Excel |
| 分页 | 每页 20/50/100 条，支持跳转页码 |

#### 2.1.5 DAG 可视化交互

| 功能 | 描述 |
|------|------|
| 拖拽节点 | 节点可自由拖拽移动 |
| 画布缩放 | 鼠标滚轮缩放，平移画布 |
| 节点详情 | 点击节点显示任务详情 |
| 依赖高亮 | 鼠标悬停高亮上下游依赖 |
| 状态颜色 | 成功 (绿)、运行 (蓝)、等待 (灰)、失败 (红) |
| 进度动画 | 执行中任务显示进度条和动画 |
| 导出图片 | 导出 DAG 为 PNG/SVG 格式 |

#### 2.1.6 SQL 编辑器功能

| 功能 | 描述 |
|------|------|
| 语法高亮 | HiveQL 关键词、函数、类型、注释高亮 |
| 智能补全 | 表名、字段名、函数名自动补全 |
| 错误标记 | 语法错误实时波浪线标记，悬停显示错误详情 |
| 格式化 | SQL 一键格式化和美化 |
| 执行计划 | 显示 SQL 执行计划（Stage、数据量、预计时间） |
| 性能分析 | SQL 性能分析和优化建议 |
| 代码片段 | 常用 SQL 片段快速插入（INSERT、SELECT、JOIN 等） |
| 多光标 | 支持多光标编辑 |
| 撤销重做 | Ctrl+Z/Y 撤销重做 |

#### 2.1.7 数据预览功能

| 功能 | 描述 |
|------|------|
| 表格展示 | 可滚动表格，支持固定表头 |
| 分页加载 | 每页 50/100 行，支持跳转 |
| 列调整 | 列宽调整、列排序、列隐藏 |
| 复制单元格 | 单击复制单元格内容 |
| 数据导出 | 导出当前页/全部数据为 CSV/Excel |
| 字段统计 | 显示数值字段的最小值、最大值、总和、平均值 |
| 数据筛选 | 条件筛选器（=、>、<、LIKE、IN 等） |

#### 2.1.8 日志查看功能

| 功能 | 描述 |
|------|------|
| 实时日志 | 日志流式实时更新 |
| 级别过滤 | 按 INFO/WARN/ERROR 过滤显示 |
| 日志搜索 | 关键词搜索和高亮显示 |
| 自动滚动 | 新日志自动滚动到底部（可关闭） |
| 时间戳 | 显示日志时间戳（精确到毫秒） |
| 日志导出 | 导出日志为文本文件 |
| 错误定位 | 快速定位 ERROR/WARN 日志 |

#### 2.1.9 通知和反馈

| 类型 | 展示方式 | 持续时间 | 示例 |
|------|----------|----------|------|
| Toast | 右上角弹窗 | 3 秒自动消失 | "保存成功" |
| 消息栏 | 顶部固定条 | 手动关闭 | "元数据同步完成" |
| 弹窗 | 居中对话框 | 用户操作 | 错误详情、确认对话框 |
| 进度条 | 状态栏/弹窗 | 任务完成 | "任务执行中 67%" |
| 系统通知 | 系统级通知 | 5 秒 | "任务执行完成" |

#### 2.1.10 加载状态

| 类型 | 使用场景 | 展示方式 |
|------|----------|----------|
| 全局加载遮罩 | 长时间操作（>2 秒） | 居中加载动画 + 百分比 + 取消按钮 |
| 骨架屏 | 数据加载（<2 秒） | 内容区域灰色占位动画 |
| 进度条 | 有进度操作 | 线性进度条/环形进度条 |
| 超时提示 | 加载超时（>30 秒） | 提示"加载时间过长" + 重试按钮 |

#### 2.1.11 快捷键

| 快捷键 | 功能 | 作用域 |
|--------|------|--------|
| Ctrl+N | 新建任务 | 全局 |
| Ctrl+S | 保存 | 编辑器/表单 |
| Ctrl+F | 搜索 | 当前视图 |
| Ctrl+R | 运行任务 | 编辑器/任务详情 |
| Ctrl+Z | 撤销 | 编辑器 |
| Ctrl+Y | 重做 | 编辑器 |
| F5 | 刷新 | 当前视图 |
| Esc | 关闭弹窗/取消操作 | 全局 |
| Ctrl+1~5 | 切换视图（任务/元数据/DAG/编辑器/设置） | 全局 |
| Ctrl+? | 打开快捷键帮助 | 全局 |

#### 2.1.12 设置中心

| 分类 | 配置项 |
|------|--------|
| 外观 | 主题（深色/浅色/跟随系统）、字体大小、语言 |
| 编辑器 | 缩进大小、自动换行、语法检查、代码片段管理 |
| 快捷键 | 快捷键自定义和重置 |
| 连接 | Hive 连接、Azkaban 连接、LDAP 连接配置管理 |
| 缓存 | 缓存 TTL、缓存清理、缓存大小限制 |
| 默认值 | 默认任务模板、默认资源队列、默认调度时间 |
| 关于 | 版本号、检查更新、更新日志、降级回滚 |

#### 2.1.13 帮助系统

| 功能 | 描述 |
|------|------|
| 新手引导 | 首次启动自动触发，分步介绍核心功能，可跳过 |
| 提示气泡 | 鼠标悬停显示功能说明和 Tips |
| 在线文档 | 内置帮助菜单入口，打开浏览器访问文档 |
| 快捷键列表 | Ctrl+? 或帮助菜单查看快捷键列表 |
| 视频教程 | 链接到视频教程页面 |
| FAQ | 常见问题解答列表 |
| 联系客服 | 联系支持团队入口 |

#### 2.1.14 版本和更新

**检查更新流程**:
```
应用启动 → 检查远程版本 → 发现新版本 → 下载更新包 → 提示用户重启安装
```

**更新对话框**:
- 显示新版本号、当前版本号
- 更新内容列表（新增功能、优化、Bug 修复）
- 更新日志链接
- 操作按钮：稍后提醒、下载并安装

**降级回滚**:
- 设置 → 关于 → 历史版本
- 选择历史版本进行降级

#### 2.1.15 扩展功能

| 功能 | 描述 |
|------|------|
| 模板管理 | 查看预置模板、自定义模板创建、模板导入导出 |
| SQL 历史 | 记录历史 SQL、快速复用、按时间/表名搜索 |
| 工作台定制 | 保存常用布局、快速切换视图组合 |
| 快捷入口 | 常用任务快速启动、常用表快速预览 |
| 血缘可视化 | 表级血缘关系图、任务级依赖追踪 |
| 统计报表 | 任务执行成功率、任务耗时统计、数据量趋势图 |

#### 2.1.16 性能和可访问性

**性能要求**:
| 指标 | 要求 |
|------|------|
| 启动时间 | <5 秒 |
| 界面响应 | <100ms |
| 内存占用 | 功能优先，不限制 |
| 多显示器 | 支持 |

**可访问性**:
- 键盘导航支持（Tab 键切换）
- 所有功能支持键盘操作

#### 2.1.17 错误处理交互

| 组件 | 功能 |
|------|------|
| 错误详情对话框 | 显示错误标题、错误码、错误描述、堆栈信息 |
| 错误日志查看 | 展开查看完整错误日志 |
| 复制错误 | 一键复制错误信息到剪贴板 |
| 解决建议 | 根据错误类型提供解决建议 |
| 重试按钮 | 支持重试操作 |
| 错误上报 | 上报错误到支持团队（可选） |

### 2.2 任务生成器模块

#### 2.2.1 模板引擎
```rust
pub enum TaskTemplate {
    SingleTableAggregation,  // 单表聚合
    MultiTableJoin,          // 多表关联
    IncrementalSync,         // 增量同步
    FullSync,                // 全量同步
    Deduplication,           // 去重清洗
    SCD,                     // 缓慢变化维度
    MetricCalculation,       // 指标计算
}

pub struct TaskGenerator {
    template: TaskTemplate,
    params: TaskParams,
    ai_service: AIService,
}
```

#### 2.2.2 AI 生成流程
```
用户输入（自然语言 + 表单）
    │
    ▼
Prompt 构建（Few-shot + RAG + CoT）
    │
    ▼
GPT-4 API 调用
    │
    ▼
SQL 解析和验证
    │
    ├── 语法正确 ──► 生成 Azkaban .job 脚本
    │
    └── 语法错误 ──► 自动修正重试
```

### 2.3 元数据管理模块

#### 2.3.1 MySQL 元数据库连接
```rust
pub struct MetastoreClient {
    pool: MySqlPool,
    cache: MetaCache,
}

impl MetastoreClient {
    // 直接查询 Hive Metastore 的 MySQL 表
    pub async fn get_table(&self, db: &str, table: &str) -> Result<TableMeta>;
    pub async fn get_partitions(&self, db: &str, table: &str) -> Result<Vec<Partition>>;
    pub async fn get_fields(&self, db: &str, table: &str) -> Result<Vec<Field>>;
}
```

#### 2.3.2 Hive Metastore 表结构
```
DBS (数据库表)
TABLES (表信息表)
COLUMNS_V2 (字段信息表)
PARTITIONS (分区表)
PARTITION_KEYS (分区键表)
SDS (存储描述表)
```

### 2.4 Azkaban API 模块

#### 2.4.1 API 封装
```rust
pub struct AzkabanClient {
    base_url: String,
    session_id: String,
    http_client: HttpClient,
}

impl AzkabanClient {
    pub async fn login(&self, username: &str, password: &str) -> Result<String>;
    pub async fn upload_project(&self, project: &str, zip: Vec<u8>) -> Result<()>;
    pub async fn execute_flow(&self, project: &str, flow: &str) -> Result<i64>;
    pub async fn get_job_status(&self, exec_id: i64, job_id: &str) -> Result<JobStatus>;
}
```

#### 2.4.2 .job 文件格式
```properties
# sample.job
type=command
command=hive -e "INSERT INTO TABLE target SELECT * FROM source;"
```

### 2.5 Git 集成模块

#### 2.5.1 工作流
```
任务创建/修改
    │
    ▼
git checkout -b <task-branch>
    │
    ▼
git add <job-files>
    │
    ▼
git commit -m "<auto-generated>"
    │
    ▼
git push -u origin <branch>
    │
    ▼
自动创建 Merge Request
```

### 2.6 数据质量模块

#### 2.6.1 检查规则引擎
```rust
pub enum QualityRule {
    NotNull { field: String },
    Unique { field: String },
    Range { field: String, min: f64, max: f64 },
    Enum { field: String, values: Vec<String> },
    Fluctuation { metric: String, threshold: f64 },
    RowCount { threshold: f64 },
}

pub struct QualityChecker {
    rules: Vec<QualityRule>,
}
```

### 2.7 更新服务模块

#### 2.7.1 自动更新流程
```
应用启动
    │
    ▼
检查远程版本（GitHub Releases）
    │
    ├── 有新版本 ──► 下载更新包 ──► 提示用户重启安装
    │
    └── 无新版本 ──► 正常运行
```

---

## 三、数据设计

### 3.1 本地配置文件（config.toml）
```toml
[azkaban]
host = "http://azkaban.example.com"
username = "your-username"
password = "your-password"

[hive_metastore]
host = "mysql.example.com"
port = 3306
database = "hive"
username = "hive"
password = "hive-password"

[ldap]
host = "ldap.example.com"
base_dn = "dc=example,dc=com"

[git]
remote = "git@gitlab.example.com:data-team/azkaban-jobs.git"
branch_prefix = "azkaban-task/"

[ai]
provider = "openai"
model = "gpt-4o"
api_key = "sk-xxx"

[cache]
metastore_ttl_seconds = 3600
```

### 3.2 本地缓存结构
```rust
struct MetaCache {
    tables: HashMap<String, TableMeta>,      // db.table -> TableMeta
    last_sync: Option<DateTime<Utc>>,         // 最后同步时间
}

struct AuditLog {
    id: i64,
    user: String,
    action: String,
    resource: String,
    timestamp: DateTime<Utc>,
    details: String,
}
```

---

## 四、接口设计

### 4.1 内部接口

#### 4.1.1 任务生成接口
```rust
pub async fn generate_task(
    template: TaskTemplate,
    params: TaskParams,
    description: Option<String>,
) -> Result<TaskArtifact>;

pub struct TaskArtifact {
    job_script: String,
    sql: String,
    dependencies: Vec<String>,
    dag_config: String,
}
```

#### 4.1.2 元数据查询接口
```rust
pub async fn list_databases() -> Result<Vec<String>>;
pub async fn list_tables(db: &str) -> Result<Vec<String>>;
pub async fn get_table_schema(db: &str, table: &str) -> Result<TableSchema>;
pub async fn preview_data(db: &str, table: &str, limit: i32) -> Result<Vec<Row>>;
```

### 4.2 外部接口

#### 4.2.1 Azkaban REST API
- POST `/login` - 登录获取 session
- POST `/upload` - 上传项目包
- POST `/execute` - 执行工作流
- GET `/status` - 查询任务状态

#### 4.2.2 GPT-4 API
- POST `/v1/chat/completions` - 生成 SQL 和任务脚本

---

## 五、安全设计

### 5.1 认证流程
```
用户打开 GUI
    │
    ▼
LDAP 认证
    │
    ├── 成功 ──► 加载主界面
    │
    └── 失败 ──► 显示错误提示
```

### 5.2 审计日志
```rust
struct AuditEvent {
    timestamp: DateTime<Utc>,
    user: String,
    action: AuditAction,
    resource: String,
    result: String,
}

enum AuditAction {
    Login,
    CreateTask,
    ModifyTask,
    ExecuteTask,
    DeleteTask,
    SyncMetadata,
}
```

---

## 六、性能设计

### 6.1 缓存策略
| 数据类型 | 缓存位置 | TTL | 更新策略 |
|----------|----------|-----|----------|
| 元数据 | 本地内存 | 1 小时 | 手动同步刷新 |
| AI 生成结果 | 本地磁盘 | 24 小时 | LRU 淘汰 |
| 数据库连接 | 连接池 | 长连接 | 自动重连 |

### 6.2 并发控制
- GUI 线程：主线程处理 UI 渲染
- 后台线程：异步任务（AI 调用、数据库查询、API 请求）
- 并发限制：最多 5 个并发任务执行

---

## 七、错误处理

### 7.1 错误分类
```rust
pub enum AppError {
    // AI 相关
    AIServiceUnavailable,
    AIGenerationFailed(String),
    
    // 数据库相关
    DatabaseConnectionFailed,
    QueryTimeout,
    MetadataSyncFailed,
    
    // Azkaban 相关
    AzkabanAPIError(String),
    TaskExecutionFailed,
    
    // Git 相关
    GitOperationFailed(String),
    
    // GUI 相关
    LDAPAuthFailed,
    UpdateCheckFailed,
}
```

### 7.2 重试策略
| 错误类型 | 重试次数 | 重试间隔 |
|----------|----------|----------|
| AI API 超时 | 3 次 | 指数退避 |
| Azkaban API 失败 | 3 次 | 1 秒 |
| 数据库查询超时 | 2 次 | 2 秒 |

---

## 八、测试设计

### 8.1 单元测试
- 任务生成器单元测试
- 元数据解析单元测试
- SQL 语法验证单元测试

### 8.2 集成测试
- GUI 与后端服务集成测试
- Azkaban API 集成测试
- Git 集成测试

### 8.3 E2E 测试
- 完整任务创建流程测试
- 任务执行流程测试

### 8.4 AI 质量测试
- 生成 SQL 准确率测试（目标>90%）
- 模板匹配准确率测试

---

## 九、部署设计

### 9.1 运行环境要求
```
操作系统：Linux (Ubuntu 20.04+)
内存：最低 2GB，推荐 4GB
磁盘：最低 1GB 可用空间
网络：需要访问 Hive、MySQL、Azkaban、GPT-4 API
```

### 9.2 安装步骤
```bash
# 1. 下载安装包
wget https://releases.example.com/offline-analysis-agent-v1.0.tar.gz

# 2. 解压
tar -xzf offline-analysis-agent-v1.0.tar.gz

# 3. 配置
cp config.example.toml config.toml
# 编辑 config.toml 填入配置

# 4. 运行
./offline-analysis-agent
```

---

## 十、技术债务清单

| ID | 债务项 | 优先级 | 计划重构时间 |
|----|--------|--------|--------------|
| TD001 | GUI 性能优化 | 低 | MVP 后 |
| TD002 | 错误处理完善 | 低 | MVP 后 |
| TD003 | 日志系统完善 | 低 | MVP 后 |
| TD004 | 配置中心集成 | 低 | 后续迭代 |

---

## 修订历史

| 版本 | 日期 | 作者 | 变更说明 |
|------|------|------|----------|
| 1.0 | 2026-05-15 | AI Agent | 初始版本 |
