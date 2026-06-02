# Quickstart: Agent Hook 配置管理

**Feature**: `008-agent-hook-config`

---

## 前提条件

- 项目 `crates/hiveweb` 已启动并运行（`cargo run` 或 `restart-hiveweb.sh`）
- MySQL 数据库 migration 已执行（V049-V050）
- 管理后台 `web-admin` 已启动（`npm run dev`）
- 已存在至少一个 Agent（非 main）用于测试

---

## 环境变量

在 `crates/hiveweb/.env` 中确认或新增：

```env
HOOK_TIMEOUT_MS=10000
HOOK_WEBHOOK_RETRY_MAX=3
HOOK_EXECUTION_RETENTION_DAYS=30
```

---

## 验证步骤

### Step 1: 创建 Hook 配置（US1 核心流程）

1. 打开管理后台，进入 Agent 编辑页面
2. 找到 **Hook 配置** 区域（新增的 Tab 页或折叠面板）
3. 点击 **添加 Hook**，填写：
   - 名称: `测试钩子`
   - 触发点: `before_agent_start`
   - 动作类型: `call_function`
   - 选择 Function: `format.template`
   - 参数: `{"template": "Agent 开始执行: {{identifier}}"}`
4. 点击保存 → 验证列表中出现该 Hook

**预期**: Hook 保存成功，列表中展示名称、触发点、动作类型、启用状态。

### Step 2: 触发 Hook 执行（US1 验证）

1. 通过用户端或 API 触发该 Agent 的一次对话
2. 等待对话完成后，检查 `hook_executions` 表：

```sql
SELECT * FROM hook_executions
WHERE agent_id = <agent_id>
ORDER BY created_at DESC
LIMIT 5;
```

**预期**: 至少有一条 `outcome=success` 的记录，`trigger_point=before_agent_start`。

### Step 3: 验证触发点上限（FR-001 边界）

1. 在同一触发点（如 `before_agent_start`）下配置 6 个 Hook
2. 保存第 6 个时

**预期**: 返回错误码 6001，提示"该触发点最多配置 5 个 Hook"。

### Step 4: 验证引用失效拒绝（FR-006）

1. 创建 Hook，动作类型选 `call_function`，选择一个不存在的 function_id
2. 点击保存

**预期**: 返回错误码 6002，提示"引用的 Function 不存在"。

### Step 5: Webhook 验证（US2）

1. 创建 Hook：
   - 触发点: `after_agent_end`
   - 动作类型: `http_webhook`
   - URL: `https://webhook.site/your-test-url`（可使用 webhook.site 生成测试端点）
2. 触发 Agent 对话
3. 检查 webhook.site 收到 POST 请求

**预期**: 外部端点收到包含 `agent_identifier`、`session_id`、`trigger_point`、`timestamp` 的 JSON payload。

### Step 6: Webhook URL 校验（FR-007）

1. 创建 Hook，URL 填写 `http://example.com`（非 HTTPS）

**预期**: 返回错误码 6003，提示"仅支持 HTTPS"。

### Step 7: Workflow 调用验证（US3）

1. 若平台有已配置的 Workflow，创建 Hook：
   - 动作类型: `call_workflow`
   - 选择目标 Workflow
2. 触发 Agent 对话
3. 检查 `hook_executions` 表

**预期**: 有 `outcome=success` 的执行记录。

### Step 8: 查看执行历史（US4）

1. 进入 **Hook 执行历史** 页面
2. 筛选对应 Agent + 时间范围
3. 验证列表中展示执行记录

**预期**: 可看到之前各步骤触发的所有 Hook 执行记录，支持按结果筛选。

### Step 9: Agent 删除后历史保留（Edge Case）

1. 删除测试 Agent
2. 进入 Hook 执行历史页面（全局查询，Super 角色）

**预期**: 该 Agent 的 Hook 执行历史仍可见，Agent 字段显示为"已删除"。

---

## 故障排查

| 症状 | 可能原因 | 检查 |
|------|---------|------|
| Hook 保存后列表为空 | V049 migration 未执行 | `SHOW TABLES LIKE 'agent_hooks'` |
| 对话后无 hook_executions 记录 | Hook disabled 或 orchestrator 未集成 | 检查 Agent 的 hooks 是否 enabled=true |
| Webhook POST 未到达 | 防火墙/SSRF 拦截或 URL 不可达 | 检查 server 日志中的 SSRF rejection 记录 |
| 6001 在 5 个内触发 | sort_order 冲突 | 检查 `idx_agent_hooks_seq` unique 约束 |
