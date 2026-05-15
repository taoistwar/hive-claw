# 离线分析 AI Agent

一个基于 Rust 的 Azkaban 离线分析任务管理工具，通过 AI 辅助实现快速任务创建和管理。

## 🎯 功能特性

### GUI 方案

本项目支持两种 GUI 方案：

| 方案 | 状态 | 说明 |
|------|------|------|
| **TUI (当前默认)** | ✅ **已实现并可运行** | 基于 ratatui 的终端界面，跨平台，无需图形环境，SSH 直连可用 |
| **GPUI (实验性)** | 🚧 **编译中** | Zed 编辑器的桌面 GUI 框架，需要图形环境支持，API 复杂度高 |

**当前推荐**：使用 TUI 版本进行开发和使用。GPUI 由于是 Zed 内部框架，API 未稳定且文档有限，暂不建议启用。

### 启用 GPUI

GPUI 是高性能桌面 GUI 框架，但需要图形环境：

```toml
# Cargo.toml 中启用：
gpui = { git = "https://github.com/zed-industries/zed.git", branch = "main" }
# 并注释掉 ratatui 和 crossterm
```

**系统要求**：
- **Linux**: `apt-get install libwayland-dev libxkbcommon-dev libgtk-3-dev`
- **macOS**: Xcode Command Line Tools
- **Windows**: Visual Studio C++ 工具

### 核心功能

| 功能模块 | 描述 | 状态 |
|---------|------|------|
| **任务管理** | 创建、编辑、删除 Azkaban 任务 | ✅ |
| **可视化向导** | 7 步创建向导，7 大任务模板 | ✅ |
| **AI 辅助** | GPT-4 辅助生成 SQL 和任务配置 | ✅ |
| **元数据管理** | 同步 Hive Metastore，表结构预览 | ✅ |
| **版本管理** | Git 集成，自动创建分支和 PR | ✅ |
| **数据质量** | 8 类质量检查，邮件告警 | ✅ |

### 支持的 7 种任务模板

1. **单表聚合** - 对单表进行 GROUP BY 聚合计算
2. **多表关联** - 多表 JOIN 生成宽表
3. **增量同步** - 基于时间戳/水位的增量数据同步
4. **全量同步** - 全量数据覆盖同步
5. **去重清洗** - 数据去重和清洗
6. **SCD** - 缓慢变化维（Type 2）处理
7. **指标计算** - 业务指标计算（DAU、留存、转化率等）

### 数据质量检查

- 非空检查
- 唯一性检查
- 值域检查
- 枚举值检查
- 波动检查
- 行数检查
- 格式检查
- 一致性检查

## 🚀 快速开始

### 环境要求

- **Rust**: 1.95+
- **操作系统**: Linux (Ubuntu 20.04+)
- **依赖库**:
  ```bash
  # Ubuntu/Debian
  apt-get install -y libssl-dev pkg-config libsqlite3-dev libgit2-dev
  ```

### 安装步骤

#### 1. 克隆/下载

```bash
cd /workspace/offline-analysis-agent
```

#### 2. 编译

```bash
# Debug 版本（开发用）
cargo build

# Release 版本（生产用）
cargo build --release
```

#### 3. 配置

```bash
# 复制配置模板
cp config.template.toml config.toml

# 编辑配置文件，填入实际连接信息
vim config.toml
```

#### 4. 运行

```bash
# 使用启动脚本
./start.sh

# 或直接运行
./target/release/offline-analysis-agent
```

### 配置说明

编辑 `config.toml` 填入以下配置：

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

[git]
remote = "git@github.com:data-team/azkaban-jobs.git"
branch_prefix = "task/"
username = "your-github-username"

[ai]
provider = "openai"
model = "gpt-4o"
api_key = "sk-your-api-key-here"
```

## 📖 使用指南

### 创建新任务

1. 启动应用后，选择"创建向导"标签页
2. 选择任务模板（7 种之一）
3. 填写任务名称和描述
4. 选择源表和目标表
5. AI 自动生成 SQL（可手动调整）
6. 配置调度策略
7. 提交并生成 Git PR

### 任务管理

- **任务列表**: 查看所有任务及状态
- **任务详情**: 查看任务配置和执行历史
- **执行日志**: 查看 Azkaban 执行日志
- **重新执行**: 失败任务可重新执行

### 元数据浏览

- **数据库列表**: 浏览所有 Hive 数据库
- **表结构**: 查看表的列、分区、统计信息
- **数据预览**: 采样查看表数据
- **搜索**: 按表名模糊搜索

### 数据质量

- **规则配置**: 为表配置质量检查规则
- **执行检查**: 任务执行后自动进行质量检查
- **告警通知**: 质量问题通过邮件告警
- **质量报告**: 生成质量检查报告

## 🏗️ 项目结构

```
offline-analysis-agent/
├── Cargo.toml              # 项目配置
├── Cargo.lock              # 依赖锁定
├── config.template.toml    # 配置模板
├── config.example.toml     # 配置示例
├── start.sh                # 启动脚本
├── README.md               # 本文档
└── src/
    ├── main.rs             # 应用入口
    ├── config/
    │   └── mod.rs          # 配置管理
    ├── gui/
    │   ├── app.rs          # GUI 应用主循环
    │   ├── mod.rs
    │   ├── views/          # 视图模块
    │   ├── components/     # UI 组件
    │   └── styles/         # 样式主题
    ├── models/
    │   ├── mod.rs
    │   ├── task.rs         # 任务模型
    │   ├── metadata.rs     # 元数据模型
    │   └── quality.rs      # 质量模型
    └── services/
        ├── mod.rs
        ├── metadata.rs     # 元数据服务
        ├── generator.rs    # 任务生成器
        ├── azkaban.rs      # Azkaban 集成
        ├── git.rs          # Git 集成
        ├── quality.rs      # 数据质量
        └── ai.rs           # AI 服务
```

## 🧪 测试

```bash
# 运行所有测试
cargo test

# 运行特定模块测试
cargo test --lib services::metadata

# 生成测试覆盖率报告（需要 tarpaulin）
cargo tarpaulin --output html
```

## 📝 开发计划

### 已完成 (MVP)

- [x] 项目初始化和基础架构
- [x] 配置管理系统
- [x] 元数据服务（MySQL 连接）
- [x] 任务生成器（7 种模板）
- [x] Azkaban API 客户端
- [x] Git 集成
- [x] AI 服务（OpenAI/Anthropic/Azure）
- [x] 数据质量服务（8 类检查）
- [x] TUI 界面（Mock）
- [x] 单元测试

### 进行中

- [ ] gpui 桌面 GUI 实现
- [ ] HiveServer2 数据预览
- [ ] AI 提示词优化（Few-shot + RAG + CoT）
- [ ] GitHub PR 自动化

### 待开发

- [ ] 用户认证（LDAP 集成）
- [ ] 任务 DAG 可视化
- [ ] 执行历史分析
- [ ] 性能优化建议
- [ ] 批量操作
- [ ] 导出/导入任务

## 🔧 故障排查

### 常见问题

#### 编译失败：找不到 OpenSSL

```bash
# 安装开发库
apt-get install -y libssl-dev pkg-config
```

#### 连接 Hive Metastore 失败

- 检查 MySQL 连接信息是否正确
- 确认网络连通性：`telnet mysql.example.com 3306`
- 验证数据库权限

#### Azkaban 登录失败

- 确认 Azkaban 地址正确
- 检查用户名密码
- 验证 Azkaban 服务状态

#### AI 服务不可用

- 检查 API Key 是否有效
- 确认网络连接
- 查看 API 配额限制

### 日志查看

应用日志输出到标准错误，可通过以下方式查看：

```bash
# 设置日志级别
RUST_LOG=debug ./target/release/offline-analysis-agent

# 查看完整日志
RUST_LOG=trace ./target/release/offline-analysis-agent 2>&1 | tee app.log
```

## 🤝 贡献指南

1. Fork 本仓库
2. 创建特性分支 (`git checkout -b feature/amazing-feature`)
3. 提交改动 (`git commit -m 'Add amazing feature'`)
4. 推送到远程 (`git push origin feature/amazing-feature`)
5. 创建 Pull Request

## 📄 许可证

MIT License

## 📧 联系方式

- 项目仓库：[GitHub](https://github.com/data-team/offline-analysis-agent)
- 问题反馈：[Issues](https://github.com/data-team/offline-analysis-agent/issues)
