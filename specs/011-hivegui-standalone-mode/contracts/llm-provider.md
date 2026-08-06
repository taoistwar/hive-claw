# Local LLM Provider Contract

## 1. 本地解析

- HiveGUI 从本地 LlmPreset 加载按 priority 排序的 Model。
- 每个 Model 引用一个独立 LlmProvider。Provider 稳定字段为 `name/category/base_url/token_encrypted/token_env`；name 唯一，category 必须由本地 provider registry 支持，base_url 为空或为绝对 HTTP/HTTPS URL。token_env 非空时优先读取对应环境变量，否则解密 `token_encrypted`。
- 当前 Agent 的 `model_preset` 非空时解析指定 Preset；为空时解析唯一默认 Preset。
- Preset、Model 或 Provider 无效时在执行前返回 `invalid_input { field, reason }`，field 使用 `model_preset`、`model` 或 `provider` 等稳定配置路径，不携带无效值且不得查询 HiveWeb。
- Agent 保存非空 model_preset 时必须验证 Preset 存在；Preset 重命名必须原子更新全部 Agent.model_preset。删除被 Agent 引用的 Preset 时返回 `conflict { field: "name", reason: "referenced_by_agent", references }`，不得携带 value；references 只列出引用 Agent 的安全 ID、identifier 和 name。legacy `kind/api_key_encrypted/api_key_env` 仅在迁移中映射，运行时和新写入不得继续使用旧字段。

## 2. Provider 适配

本地配置转换为 workspace `providers` crate 的 `ProviderBuildConfig`，由其构造 `Arc<dyn LLMProvider>`。HiveGUI 不复制 vendor HTTP client，也不引入第二套 tool-call JSON 解析器。

请求包含：system prompt、会话消息、当前 Agent 的 Tool schema、Preset 的温度和 token 上限。消息正文只在调用期间解密到内存，不写诊断日志。

## 3. Fallback

- Preset 中 Model 按 priority 形成 fallback 链。
- 429、5xx、网络/TLS 错误和节点超时可切换到下一 Model。
- 参数错误、认证失败、Capability 拒绝和用户取消不得 fallback。
- 每次切换发出 `fallback_used`，所有节点失败才发 `failed`。

## 4. Tool call 与流式输出

- Provider 的增量文本映射为 runtime `token` 事件。
- Tool call 必须先经 ToolRegistry schema 校验，再经当前 Agent Capability 校验；失败作为 Tool 结果反馈或终止执行，取决于错误类别。
- 最终 assistant 消息只在完整成功后加密持久化；被取消的未完成增量不得写为完整 assistant 消息。

## 5. 取消与超时

LLM future 与 execution 取消令牌绑定。停止请求到达后取消 HTTP future并忽略迟到回调；取消不得触发 fallback。每个节点的超时必须小于或等于外层 execution 预算，并在日志中单列 `llm_ms`，不计入本地编排预算。
