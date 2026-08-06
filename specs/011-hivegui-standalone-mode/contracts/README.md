# Contracts: HiveGUI 独立本地 Agent

本目录定义 HiveGUI 对本地 UI、运行时、Plugin 和备份文件承诺的稳定边界。HiveGUI 不暴露也不消费 HiveWeb HTTP API；“共享”指编译期 Rust 代码和兼容性夹具复用，不代表运行时通信。

| 契约 | 范围 |
| --- | --- |
| [local-runtime.md](local-runtime.md) | 本地 Agent 命令、事件、执行状态、错误与取消传播 |
| [llm-provider.md](llm-provider.md) | 本地 LLM Preset/Provider 解析、fallback、流式事件与取消 |
| [plugin-abi.md](plugin-abi.md) | 版本化 Extism ABI、manifest、host_call、隔离和资源限制 |
| [backup-package.md](backup-package.md) | 认证加密备份包、清单、兼容性和完整替换恢复 |
| [storage-migration.md](storage-migration.md) | SQLite schema 检测、顺序迁移、快照和失败阻断 |
| [ui-accessibility.md](ui-accessibility.md) | GPUI 导航、后台任务状态、键盘、焦点和对比度 |

所有契约错误都使用稳定的字符串 `kind`，面向用户的中文文案由 UI 映射；内部错误、密钥、会话正文和 Tool 参数不得直接作为用户文案或日志字段。
