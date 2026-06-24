# code guide

## crates/hiveweb
是整个业务的入口。

`/api/assistant` 接口，是整个Agent的入口。

## Agent 编排

`crates/hiveweb/src/runtime/orchestrator.rs`

## Agent 组成

- `capabilities` 表：能力仓库
- `plugins` 表，插件仓库，使用 extism 开发 wasm 插件。
- `functions` 表：函数仓库，每个函数需要对应若干个capability才允许执行。
  - 内部函数写在当前代码仓库中 `crates/hiveweb/src/runtime/builtins`
  - 外部函数关联插件和插件的函数名称
- `workflows` 表：工作流仓库，每个工作流中调用若干个function和一些特殊节点。对应的 capability 是包含的函数和特殊节点需要的 capability总和。
- `workflow_nodes`表：工作流节点仓库，现在支持节点类型：开始节点、函数节点、生成回答节点、结束节点。
- `workflow_edges`表：工作流边仓库，定义工作流节点之间的连接关系。
- `skills` 表：技能仓库，通过`invoke_workflow`和`invoke_function`两种方式调用工作流和函数。每个技能需要对应若干个capability才允许执行。
- `agent_skills` 表：agent和技能的关联表
- `tools` 表：工具仓库，每个工具包装一个function或workflow。每个工具需要对应若干个capability才允许执行。
- `agent_tools` 表：agent和工具的关联表
- `agents` 表：配置agent的基本信息
- `agent_permissions` 表：agent和用户的关联表，agent调用工具和技能时，需要检查所需权限Agent是否拥有。
- `agent_hooks` 表：agent和hook的关联表，hook分三种：function、workflow、Webhook。
