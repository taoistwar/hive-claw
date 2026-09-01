# GPUI UI and Accessibility Contract

## 1. 导航

顶层路由固定为 `Home`、`Ai`、`Tools`。AI 管理页固定为 13 个 Tab，顺序为：Agent、工具、技能、函数、流程、插件、LLM、Capabilities、分类、标签、会话、数据管理、全局配置。对话入口、备份/恢复和诊断入口必须挂在这三个现有路由内，不新增平行 Web 风格路由。

## 2. 后台任务

Agent、LLM、Tool、Workflow、Plugin、连接测试、备份和恢复预验证都在 Tokio 后台任务执行。GPUI 主线程只接收轻量状态更新。

状态：`idle → queued → running → stopping → completed|failed|cancelled`。运行时停止按钮始终可聚焦和触发；触发后 100ms 内显示“正在停止”，不得等底层退出才更新 UI。

## 3. 键盘

- Tab/Shift+Tab 按视觉顺序移动焦点，Enter/Space 激活，Escape 关闭当前可关闭层。
- 所有 CRUD、搜索、分页、Agent 资源分配、对话、备份/恢复都无需鼠标。
- DAG：Tab 进入画布；方向键选择/移动节点；Enter 开始/确认连线；Escape 取消；Delete 删除选中节点或边；属性面板可完整键盘编辑。
- 焦点指示器始终可见，新增/删除/重排后移动到可预测且仍存在的控件。

## 4. 模态与错误

模态打开后焦点进入首个有效控件并限制在模态内；关闭后恢复触发控件。表单错误保留用户输入，把焦点移动到错误摘要，并让字段暴露可访问错误状态。公开 Store/运行时/导入边界返回 `invalid_input { field, reason }` 时，错误摘要必须显示稳定字段名和原因且不得显示被拒绝值；唯一性 `conflict { field, value }` 才显示被拒绝值的安全表示，value 仅允许 identifier、name、slug、key 等安全字段；引用或状态 `conflict { field, reason, references }` 必须显示原因和安全引用列表且不得携带或显示 value。密码、token、消息正文等机密值只显示字段名和脱敏原因。重复 identifier 必须显示 `identifier` 与安全冲突值，不关闭表单、不清空其他字段且不产生部分写入。

停止终态必须显示已完成、被中断和未开始步骤，以及“已完成的外部副作用不会自动回滚”。Plugin 超过 2 秒被终止时显示稳定错误类别。

数据库损坏恢复界面必须把“从备份恢复”“保留损坏文件并重建”和“退出”暴露为独立键盘操作；重建使用二次确认并恢复可预测焦点。迁移失败界面只能显示“重试迁移”和“退出”，不得混用损坏重建动作。

设备密钥缺失、损坏、权限不安全或不可读时必须显示阻断式恢复界面，只提供重新配置、从加密备份恢复和退出等不会静默覆盖现有密文的操作；主业务界面和敏感数据写入口不得开放。

Placeholder Function 的 Kind 标签、input/output schema 和不可执行状态必须暴露给无障碍树；UI 不显示 Plugin、Capability 或测试执行操作。若共享组件无法移除通用执行控件，则控件必须禁用并暴露 `function_not_executable` 原因，不能触发后台任务。

## 5. 无障碍树与视觉

- 交互控件通过 GPUI/AccessKit 暴露 name、role、state、value 和错误。
- 加载、成功、失败、危险、选中和禁用状态不能只靠颜色。
- 普通文本对比度≥4.5:1；大字号文本、焦点指示器和关键 UI 图形≥3:1。
- 对话消息和 DAG 状态变化提供非抢占式 live 状态；错误与取消终态可被辅助技术读取。

## 6. 性能验收

- 用户输入到可见反馈 p95≤100ms。
- 主 UI 线程单次连续阻塞≤250ms。
- 同时运行 Agent、100 节点 no-op Workflow 和备份预验证时，停止控件可用率 100%。

自动测试使用 VisualTestContext 驱动键盘、焦点、滚动和边界；AccessKit 完整树若测试平台不可见，则增加测试钩子并保留 Linux/macOS/Windows 各一次真实辅助技术 smoke test。
