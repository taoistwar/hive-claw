# 大数据离线分析 AI Agent - 技术设计文档

**版本**: 2.0  
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
│ LDAP          │      │ GPT-4 API     │     │ GitHub        │
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

**7 大核心模板**:

| 模板 ID | 模板名称 | 描述 | 适用场景 |
|---------|----------|------|----------|
| TMPL-001 | 单表聚合 | 对单表按维度分组聚合 | 日报、周报、月报聚合 |
| TMPL-002 | 多表关联 | 多表 JOIN 关联查询 | 宽表构建、数据整合 |
| TMPL-003 | 增量同步 | 从 MySQL 增量同步数据 | MySQL→Hive 增量同步 |
| TMPL-004 | 全量同步 | 从 MySQL 全量覆盖同步 | 维度表全量同步 |
| TMPL-005 | 去重清洗 | 数据去重和清洗 | 重复数据清理 |
| TMPL-006 | SCD | 缓慢变化维度处理 | 维度表 SCD Type2 |
| TMPL-007 | 指标计算 | 业务指标计算 | UV、PV、留存率等 |

**模板变量类型**:
| 变量类型 | 示例 | 说明 |
|----------|------|------|
| 字符串 | `source_table`, `target_table` | 表名、字段名、别名 |
| 数值 | `limit_count`, `threshold` | 阈值、限制数量 |
| 枚举 | `sync_type` (incremental/full) | 增量/全量、频率等 |
| 日期 | `partition_field`, `date_format` | 分区字段、日期格式 |
| 数组 | `select_fields`, `group_by_fields` | 字段列表、条件列表 |
| SQL 片段 | `where_clause`, `join_clause` | WHERE、JOIN 子句 |

**模板嵌套**:
```
主模板：数据同步流程
├── 子模板：增量同步 SQL 生成
├── 子模板：数据质量检查
└── 子模板：失败告警配置
```

#### 2.2.2 AI 生成策略

**AI+ 模板混合模式**:
```
用户输入（自然语言 + 表单）
    │
    ▼
解析用户意图 → 匹配模板类型
    │
    ▼
AI 填充模板参数:
- 从自然语言提取表名
- 从自然语言提取字段映射
- 从自然语言提取过滤条件
    │
    ▼
渲染模板生成 SQL
    │
    ▼
验证和修正
```

**Prompt 模板管理**:
- 每个模板类型有独立的 Prompt 模板
- Prompt 变更需要版本记录
- 支持 A/B 测试不同 Prompt 效果
- 支持回滚到历史版本

**Few-shot 示例库**:
| 模板类型 | 示例数量 | 难度分布 |
|----------|----------|----------|
| 单表聚合 | 10 | 简单 5、中等 3、困难 2 |
| 多表关联 | 10 | 简单 3、中等 4、困难 3 |
| 增量同步 | 8 | 简单 4、中等 2、困难 2 |
| 全量同步 | 5 | 简单 3、中等 2 |
| 去重清洗 | 7 | 简单 3、中等 3、困难 1 |
| SCD | 5 | 中等 2、困难 3 |
| 指标计算 | 10 | 简单 4、中等 4、困难 2 |

**RAG 知识库检索**:
- 表关系文档
- 指标口径文档
- SQL 案例库
- 命名规范文档
- FAQ 库
- 数据质量标准

#### 2.2.3 SQL 验证

**验证流程**:
```
AI 生成 SQL
    │
    ▼
1. 语法校验（HiveServer2 预编译）
    │
    ├── 通过 ──► 2. 元数据校验
    │              │
    │              ├── 通过 ──► 3. 业务规则校验
    │              │              │
    │              │              ├── 通过 ──► 4. 性能规则校验
    │              │              │              │
    │              │              │              ├── 通过 ──► 5. 安全规则校验
    │              │              │              │              │
    │              │              │              │              └── 通过 ──► 验证成功
    │              │              │              │
    │              │              │              └── 失败 ──► 性能优化建议
    │              │              │
    │              │              └── 失败 ──► 业务规则错误
    │              │
    │              └── 失败 ──► 元数据错误（表/字段不存在）
    │
    └── 失败 ──► 语法错误 ──► 触发修正流程
```

**验证规则管理**:
| 类别 | 规则 ID | 规则描述 | Violation 级别 |
|------|--------|----------|----------------|
| 内置 | RULE-001 | 分区字段必须存在 | Error |
| 内置 | RULE-002 | 目标表必须已存在 | Error |
| 内置 | RULE-003 | SELECT 字段必须在源表中存在 | Error |
| 业务 | RULE-101 | 增量同步必须有时间过滤条件 | Error |
| 业务 | RULE-102 | 聚合必须包含 GROUP BY 字段 | Warning |
| 性能 | RULE-201 | 避免 SELECT * | Warning |
| 性能 | RULE-202 | 大表 JOIN 必须有索引 | Warning |
| 安全 | RULE-301 | 敏感字段需要脱敏 | Error |
| 安全 | RULE-302 | 禁止 DROP/TRUNCATE 操作 | Error |

#### 2.2.4 错误修正

**混合修正模式**:
```
语法错误
    │
    ▼
判断错误类型:
├── 简单语法错误 ──► 规则修正（固定替换规则）
│                    如：关键字大小写、括号匹配
│
├── 复杂语法错误 ──► AI 自动修正
│                    发送错误信息 + 原 SQL → AI 重试
│
└── AI 修正失败 ──► 用户确认修正
                     显示修正建议，用户选择是否应用
```

**智能重试策略**:
```
第 1 次生成 → 验证失败
    │
    ▼
分析错误类型 → 调整 Prompt（添加错误信息和修正指示）
    │
    ▼
第 2 次生成 → 验证失败
    │
    ▼
切换模型（GPT-4 → GPT-4o）或增加 Few-shot 示例
    │
    ▼
第 3 次生成 → 验证失败
    │
    ▼
放弃自动修正 → 显示错误详情和修正建议 → 用户处理
```

#### 2.2.5 生成历史

**历史记录结构**:
```json
{
  "id": "gen-20260515-001",
  "timestamp": "2026-05-15T10:30:00Z",
  "template_id": "TMPL-003",
  "user_input": {
    "natural_language": "每天从 MySQL 同步用户表到 Hive",
    "form_params": {
      "source_table": "mysql.user",
      "target_table": "ods_user",
      "sync_type": "incremental"
    }
  },
  "ai_request": {
    "model": "gpt-4o",
    "prompt_version": "v2.1",
    "tokens_used": 1250
  },
  "ai_response": {
    "sql": "INSERT INTO TABLE...",
    "explanation": "...",
    "duration_ms": 3500
  },
  "validation": {
    "passed": true,
    "rules_checked": 10,
    "warnings": []
  },
  "quality": {
    "ai_self_score": 4.5,
    "user_feedback": null,
    "execution_success": true
  },
  "status": "used"
}
```

### 2.3 元数据管理模块

#### 2.3.1 MySQL 连接配置

```toml
[metastore]
# 基本连接
host = "mysql.example.com"
port = 3306
database = "hive"
username = "hive"
password = "hive-password"

# 连接池
pool_min_connections = 5
pool_max_connections = 20
pool_timeout_sec = 30

# SSL 加密
ssl_enabled = true
ssl_ca_cert = "/path/to/ca.pem"
ssl_client_cert = "/path/to/client.pem"
ssl_client_key = "/path/to/client-key.pem"

# 超时配置
connect_timeout_sec = 10
read_timeout_sec = 30
write_timeout_sec = 30

# 自动重连
auto_reconnect = true
max_reconnect_attempts = 3
reconnect_delay_sec = 5

# 多数据源
[[metastore.sources]]
name = "prod-cluster"
host = "mysql-prod.example.com"
database = "hive"
is_default = true

[[metastore.sources]]
name = "dev-cluster"
host = "mysql-dev.example.com"
database = "hive"
is_default = false
```

#### 2.3.2 元数据模型

```rust
// 数据库
pub struct Database {
    pub name: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub created_at: DateTime<Utc>,
    pub owner: Option<String>,
}

// 表
pub struct Table {
    pub db_name: String,
    pub name: String,
    pub table_type: TableType,  // MANAGED_TABLE / EXTERNAL_TABLE
    pub created_at: DateTime<Utc>,
    pub last_access_time: Option<DateTime<Utc>>,
    pub owner: Option<String>,
    pub comment: Option<String>,
    pub retention: i32,
    pub is_partitioned: bool,
}

// 字段
pub struct Column {
    pub table_name: String,
    pub name: String,
    pub data_type: String,
    pub comment: Option<String>,
    pub position: i32,
    pub is_nullable: bool,
}

// 分区
pub struct Partition {
    pub table_name: String,
    pub name: String,
    pub values: Vec<String>,
    pub location: String,
    pub created_at: DateTime<Utc>,
}

// 表关系
pub struct TableRelation {
    pub source_table: String,
    pub target_table: String,
    pub relation_type: RelationType,
    pub join_columns: Vec<String>,
    pub discovered_at: DateTime<Utc>,
    pub confidence: f32,
}
```

#### 2.3.3 缓存策略

**双层缓存**:
- 内存缓存：Rust HashMap + Arc（LRU + TTL）
- 磁盘缓存：SQLite

**缓存配置**:
```rust
pub struct CacheConfig {
    // TTL 配置（按数据库隔离）
    ttl_default: Duration,
    ttl_by_database: HashMap<String, Duration>,
    
    // LRU 容量
    max_entries: usize,
    
    // 预热配置
    preload_databases: Vec<String>,
}
```

#### 2.3.4 元数据同步

**增量同步流程**:
```
手动触发同步
    │
    ▼
读取 MySQL Metastore 表:
- DBS (数据库)
- TABLES (表信息)
- COLUMNS_V2 (字段信息)
- PARTITIONS (分区)
- SDS (存储描述)
    │
    ▼
增量对比:
- 新表：标记为新增
- 变更表：标记为变更
- 删除表：标记为删除
    │
    ▼
生成变更列表 → 用户确认 → 更新缓存
```

#### 2.3.5 表关系发现

**SQL 解析发现**:
```rust
// 从 SQL 历史中提取表关系
pub async fn extract_from_sql_history(
    &self,
    sql_history: &[String],
) -> Result<Vec<TableRelation>> {
    // 解析 SQL AST，提取 INSERT INTO target SELECT FROM source
    // 构建表级血缘关系
}
```

**血缘分析粒度**:
| 粒度 | 描述 |
|------|------|
| 表级血缘 | 哪些表生成了当前表 |
| 字段级血缘 | 哪些字段生成了当前字段 |
| 任务级血缘 | 哪些任务依赖当前任务 |

### 2.4 Azkaban 集成模块

#### 2.4.1 API 封装

```rust
pub struct AzkabanClient {
    base_url: String,
    session_id: Arc<RwLock<String>>,
    http_client: HttpClient,
    credentials: AzkabanCredentials,
}

pub trait AzkabanApi {
    // 用户认证
    async fn login(&self) -> Result<String>;
    
    // 项目管理
    async fn create_project(&self, name: &str, description: &str) -> Result<()>;
    async fn delete_project(&self, name: &str) -> Result<()>;
    async fn list_projects(&self) -> Result<Vec<ProjectInfo>>;
    
    // 任务上传
    async fn upload_project(&self, name: &str, zip_data: Vec<u8>) -> Result<UploadResponse>;
    
    // 执行控制
    async fn execute_flow(
        &self,
        project: &str,
        flow: &str,
        params: ExecutionParams,
    ) -> Result<i64>;
    
    async fn cancel_flow(&self, exec_id: i64) -> Result<()>;
    
    // 状态查询
    async fn get_flow_status(&self, exec_id: i64) -> Result<FlowStatus>;
    async fn get_job_status(&self, exec_id: i64, job_id: &str) -> Result<JobStatus>;
    
    // 日志下载
    async fn get_job_logs(&self, exec_id: i64, job_id: &str, offset: i32) -> Result<LogResponse>;
}
```

#### 2.4.2 任务文件格式

**ZIP 包结构（扁平）**:
```
project-name.zip
├── sync_user.job
├── agg_order.job
├── clean_log.job
├── project.properties
└── flow.yaml
```

**.job 文件格式**:
```properties
# sync_user.job
type=command
command=/path/to/sync_script.sh ${input_table} ${output_table}

# 依赖配置
dependencies=agg_order,clean_log

# 自定义参数
resource.queue=default
mapreduce.queue.name=default

# 失败策略
failure.action=end
```

#### 2.4.3 执行参数

```rust
pub struct ExecutionParams {
    pub flow_name: String,
    pub job_props: HashMap<String, String>,
    pub concurrent: bool,
    pub max_concurrent: u32,
    pub failure_action: FailureAction,
    pub schedule: Option<ScheduleConfig>,
}

pub enum FailureAction {
    End,      // 停止工作流
    Continue, // 继续执行
}

pub struct ScheduleConfig {
    pub cron: String,
    pub timezone: String,
}
```

#### 2.4.4 状态查询

**状态粒度**:
| 粒度 | 描述 |
|------|------|
| 工作流状态 | 整体执行状态 |
| 任务状态 | 每个 job 的执行状态 |
| 节点状态 | 节点的重试/跳过状态 |
| 进度百分比 | 当前执行进度 |

**状态枚举**:
```rust
pub enum FlowStatus {
    Preparing, Running, Paused, Succeeded, Failed, Killed,
}

pub enum JobStatus {
    Pending, Preparing, Running, Paused, Succeeded, Failed, Killed, Skipped,
}
```

#### 2.4.5 错误重试

**智能重试策略**:
```rust
pub async fn call_with_retry<F, T>(
    client: &AzkabanClient,
    f: F,
) -> Result<T>
where
    F: Fn() -> Future<Output = Result<T>>,
{
    let mut retries = 0;
    let mut delay = client.retry_config.base_delay_ms;
    
    loop {
        match f().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                if retries >= client.retry_config.max_retries {
                    return Err(e);
                }
                if !e.is_retryable() {
                    return Err(e);
                }
                retries += 1;
                delay = (delay * 2).min(client.retry_config.max_delay_ms);
            }
        }
    }
}
```

### 2.5 Git 集成模块

#### 2.5.1 仓库配置

```toml
[git]
git_dir = ".git"
default_branch = "main"
auto_init = true

[[git.remotes]]
name = "origin"
url = "git@github.com:data-team/azkaban-jobs.git"
is_default = true
```

#### 2.5.2 分支管理

**分支命名**: `{任务名}_{时间戳}`
- 示例：`sync_user_20260515_103000`

**分支操作流程**:
```
创建分支 → 切换分支 → 提交改动 → Push → 创建 MR
```

#### 2.5.3 Commit 管理

**AI 生成 Commit Message**:
```
分析变更内容 → 生成 Conventional Commits 格式 → 用户确认 → 执行 commit
```

#### 2.5.4 GitHub 集成

**MR 自动化**:
- 自动创建 Pull Request
- AI 生成 MR 标题
- AI 生成 MR 描述（任务描述、关联 ID、diff 摘要、测试结果、文件列表、检查清单）
- 人工确认合并

**认证方式**: SSH Key

### 2.6 数据质量模块

#### 2.6.1 检查类型

| 检查类型 | 描述 | 配置参数 |
|----------|------|----------|
| 非空检查 | 字段是否为 NULL | 空值比例阈值 |
| 唯一性检查 | 主键/业务键唯一 | 重复比例阈值 |
| 值域检查 | 数值在合理范围 | min/max |
| 枚举值检查 | 值在枚举列表内 | 有效值列表 |
| 波动检查 | 环比/同比波动 | 波动阈值、基线天数 |
| 行数检查 | 数据行数波动 | 行数阈值 |
| 格式检查 | 日期/邮箱格式 | 正则表达式 |
| 一致性检查 | 跨表数据一致 | 差异容忍度 |

#### 2.6.2 问题分级

| 级别 | 描述 | 处理要求 |
|------|------|----------|
| 阻塞 (Blocker) | 数据不可用 | 必须修复 |
| 严重 (Critical) | 数据部分不可用 | 尽快修复 |
| 一般 (Major) | 数据可降级使用 | 建议修复 |
| 提示 (Minor) | 不影响使用 | 可选修复 |

#### 2.6.3 检查执行

**执行方式**: 独立质量检查任务

**检查范围**: 分区检查（最新分区）

**检查 SQL 生成**: SQL 模板

#### 2.6.4 邮件告警

**告警配置**:
- 收件人列表
- 邮件模板自定义
- 告警触发条件
- 告警聚合（多条问题合并邮件）
- 免打扰时段设置

#### 2.6.5 质量评分

**评分体系**:
- 表级质量评分
- 字段级质量评分
- 数据库级质量评分
- 综合质量评分

### 2.7 更新服务模块

#### 2.7.1 自动更新流程

```
应用启动 → 检查远程版本（GitHub Releases）
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
remote = "git@github.com:data-team/azkaban-jobs.git"
branch_prefix = "task/"

[ai]
provider = "openai"
model = "gpt-4o"
api_key = "sk-xxx"

[cache]
metastore_ttl_seconds = 3600

[quality]
alert_email = "data-team@example.com"
default_threshold = 0.05
```

### 3.2 SQLite 缓存表结构

```sql
-- 数据库缓存
CREATE TABLE cache_databases (
    name TEXT PRIMARY KEY,
    description TEXT,
    location TEXT,
    created_at INTEGER,
    updated_at INTEGER
);

-- 表缓存
CREATE TABLE cache_tables (
    id INTEGER PRIMARY KEY,
    db_name TEXT,
    name TEXT,
    table_type TEXT,
    created_at INTEGER,
    comment TEXT,
    is_partitioned BOOLEAN,
    UNIQUE(db_name, name)
);

-- 字段缓存
CREATE TABLE cache_columns (
    id INTEGER PRIMARY KEY,
    table_id INTEGER,
    name TEXT,
    data_type TEXT,
    comment TEXT,
    position INTEGER,
    FOREIGN KEY (table_id) REFERENCES cache_tables(id)
);

-- 版本快照
CREATE TABLE metadata_versions (
    version INTEGER PRIMARY KEY,
    created_at INTEGER,
    sync_type TEXT,
    snapshot_path TEXT
);

-- 质量检查结果
CREATE TABLE quality_results (
    id INTEGER PRIMARY KEY,
    check_time INTEGER,
    table_name TEXT,
    rule_id TEXT,
    severity TEXT,
    passed BOOLEAN,
    actual_value REAL,
    threshold REAL,
    message TEXT
);
```

---

## 四、接口设计

### 4.1 内部接口

```rust
// 任务生成
pub async fn generate_task(
    template: TaskTemplate,
    params: TaskParams,
    description: Option<String>,
) -> Result<TaskArtifact>;

// 元数据查询
pub async fn list_databases() -> Result<Vec<String>>;
pub async fn list_tables(db: &str) -> Result<Vec<String>>;
pub async fn get_table_schema(db: &str, table: &str) -> Result<TableSchema>;
pub async fn preview_data(db: &str, table: &str, limit: i32) -> Result<Vec<Row>>;

// 质量检查
pub async fn run_quality_checks(
    table: &str,
    partition: &str,
    rules: Vec<QualityRule>,
) -> Result<QualityReport>;
```

### 4.2 外部接口

| 接口 | 端点 | 说明 |
|------|------|------|
| Azkaban Login | POST `/login` | 登录获取 session |
| Azkaban Upload | POST `/upload` | 上传项目包 |
| Azkaban Execute | POST `/executor` | 执行工作流 |
| Azkaban Status | GET `/status` | 查询任务状态 |
| Azkaban Logs | GET `/executor?ajax=fetchexeclog` | 下载日志 |
| GPT-4 | POST `/v1/chat/completions` | AI 生成 |
| GitHub | REST API | Git 操作 |

---

## 五、安全设计

### 5.1 LDAP 认证

```
用户打开 GUI → LDAP 认证 → 成功加载主界面 / 失败显示错误
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
    Login, CreateTask, ModifyTask, ExecuteTask, DeleteTask, SyncMetadata,
}
```

---

## 六、性能设计

### 6.1 缓存策略

| 数据类型 | 缓存位置 | TTL | 更新策略 |
|----------|----------|-----|----------|
| 元数据 | 本地内存 + SQLite | 1 小时 | 手动同步刷新 |
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
    AIServiceUnavailable,
    AIGenerationFailed(String),
    DatabaseConnectionFailed,
    QueryTimeout,
    MetadataSyncFailed,
    AzkabanAPIError(String),
    TaskExecutionFailed,
    GitOperationFailed(String),
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

### 8.1 测试类型

| 测试类型 | 说明 |
|----------|------|
| 单元测试 | 核心函数和模块的单元测试 |
| 集成测试 | 模块间集成测试 |
| E2E 测试 | 端到端流程测试 |
| AI 质量测试 | AI 生成准确率测试（目标>90%） |

### 8.2 CI/CD

- 工具：Jenkins

---

## 九、部署设计

### 9.1 运行环境

```
操作系统：Linux (Ubuntu 20.04+)
内存：最低 2GB，推荐 4GB
磁盘：最低 1GB 可用空间
网络：需要访问 Hive、MySQL、Azkaban、GitHub、GPT-4 API
```

### 9.2 安装步骤

```bash
# 1. 下载安装包
wget https://releases.example.com/offline-analysis-agent-v1.0.tar.gz

# 2. 解压
tar -xzf offline-analysis-agent-v1.0.tar.gz

# 3. 配置
cp config.example.toml config.toml

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
| 2.0 | 2026-05-15 | AI Agent | 整合所有模块详细规格 |
