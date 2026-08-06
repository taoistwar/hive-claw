# Shared Plugin ABI Contract

## 1. 兼容边界

HiveWeb 与 HiveGUI 共享同一 Rust ABI 类型、manifest 校验器、host_call envelope、错误类别和兼容性 fixture。HiveGUI 的兼容性判断是纯本地操作，不请求 HiveWeb。

当前 ABI 为 `hive-extism/v1`。manifest 必须包含：

```json
{
  "abi_version": "hive-extism/v1",
  "required_capabilities": ["network.http"],
  "exports": [{"name":"lookup","input":"json","output":"json"}]
}
```

未知字段可保留；required_capabilities 必须是去重字符串数组，每个名称去除首尾空白后长度为 1..=255 且存在于目标 host 的 Capability registry。不支持的 ABI、结构错误、空白/重复 Capability 或宿主缺失 Capability 必须在复制文件和写库前一次性返回。

## 2. Plugin export

每个声明 export 接收 UTF-8 JSON 字节并返回 UTF-8 JSON 字节。输入先按 Function.input_schema 校验，输出在超过大小上限前中止或拒绝，并在返回后按 Function.output_schema 校验。

Placeholder Function 不声明 Plugin export，也不得进入 ABI 或制品解析；任一执行入口必须在 guest 实例化、Capability 检查和 Plugin 查找前返回 `function_not_executable`。

## 3. host_call

请求：

```json
{"capability":"network.http","args":{}}
```

成功：`{"ok":true,"data":...}`。失败：`{"ok":false,"code":4030,"message":"..."}`，失败 envelope 不得带 `data`。

v1 保留现有数值错误码，并允许非破坏性增项：

| code | kind |
| ---: | --- |
| 4001 | invalid_args |
| 4030 | capability_denied |
| 4045 | capability_unknown |
| 4081 | capability_timeout |
| 4990 | cancelled |
| 5000 | internal |
| 5004 | plugin_timeout |
| 5011 | plugin_memory_limit |
| 5012 | plugin_output_limit |
| 5013 | wasi_denied |

鉴权顺序：解析 envelope → 确认 Capability 已注册 → 确认 manifest 声明 → 确认当前 Agent 已显式分配 → 校验 args → 调用本地 handler → 脱敏记录结果。

## 4. 隔离

- PluginBuilder 必须 `with_wasi(false)`。
- manifest 的 allowed_hosts、allowed_paths、环境变量或其它自提权字段不得启用。
- 文件、网络、环境变量和宿主进程资源只可经 host_call 的已授权 Capability 获取。
- HiveGUI 必须先以平台等价的 no-follow 语义打开并固定托管 Plugin 根目录句柄；数据库中的 `s3_key` 只作为该句柄下的安全相对制品键。每个路径段都必须相对于前一已打开目录句柄解析，中间目录和最终目标均不得是 Unix symlink、Windows junction/reparse point、hardlink、device、FIFO、socket 或其他非普通文件/可重定向链接；无法可靠判定时必须失败关闭。
- 禁止只做字符串规范化，或先 `canonicalize`、关闭检查句柄后再按路径重新打开。最终 WASM 必须是 link count 为 1 的普通文件；打开后必须从同一文件句柄读取身份元数据、实际大小与内容并计算 SHA-256，路径检查、文件类型/link count、哈希和 guest 实例化必须绑定同一已验证对象。
- Plugin resolver 必须向 runtime adapter 传递已验证且仍打开的只读制品句柄（以及保持路径边界成立所需的根目录句柄），而不是只传递 `s3_key` 或绝对路径。runtime 不得按路径重新打开制品；句柄至少保持到 runtime 从该对象完成 guest 实例化，确保并发 rename 或链接替换不能改变本次执行实际使用的字节。

## 5. 资源限制

| 资源 | 默认 | 用户覆盖硬上限 |
| --- | ---: | ---: |
| 墙钟时间 | 30,000 ms | 120,000 ms |
| linear memory | 128 MiB（2048 × 64KiB pages） | 512 MiB（8192 × 64KiB pages） |
| 输出 | 10,485,760 bytes | 52,428,800 bytes |

数据库字段 `memory_limit_mb` 为兼容既有 schema 保留名称，逻辑单位固定为 MiB；内存按 `memory_limit_mb × 16` 换算为 64KiB WASM pages 后配置 Extism manifest，不得直接传 MiB 数值或字节数。墙钟 timeout 与 fuel 同时生效。用户停止时先传播协作取消，2 秒后用 Extism CancelHandle 中断并丢弃实例。timeout、memory、output、cancel 分别返回稳定错误类别。

### HiveGUI 空闲实例池

- HiveGUI host 的池只保存未被执行占用的健康实例，全局最多 8 个空闲实例，每个完整 cache key 最多 1 个；达到上限时按最近最少使用（LRU）淘汰。该本地容量策略不改变 HiveWeb host 的 ABI 行为。
- 完整 cache key 为 `(artifact_sha256, abi_version, runtime, runtime_version, fuel_limit, timeout_ms, memory_limit_mb, output_limit_bytes, capability_policy_hash)`；任一字段变化都不得命中旧实例。
- 制品替换、Plugin 配置修改、Capability 授权策略修改或软删除时，必须立即淘汰相关空闲实例。实例发生取消、timeout、trap、memory/output 超限或 host error 后必须丢弃，只有成功完成且仍健康的实例可回池。
- 活跃实例不得被并发执行共享；淘汰或丢弃只影响目标实例，不得中断其他会话或退出本地运行时。

## 6. 导入与替换

每次新导入或替换都必须分配从未复用的不可变唯一制品键，例如包含 SHA-256 与随机操作 ID 的受控相对名称；数据库中的 `s3_key` 只引用该唯一对象，发布后禁止原地改写。`plugin_artifact_operations` 的两类操作字段不变量如下：

| `operation_kind` | `prepared` / 后续必填字段 | 初始为空字段 | 用户可见提交 |
| --- | --- | --- | --- |
| `create` | `target_identifier`、由 operation_id 派生的唯一 `staging_name`、新键/哈希/大小；`staged` 后另有 staging identity，`published` 后另有 new identity | `plugin_id` 在 `prepared|staged|published` 为空；全部 `expected_old_*`、`expected_row_revision` 在所有状态始终为空 | 在同一个 SQLite 事务中插入 Plugin 行、回填生成的 `plugin_id` 并标记 `referenced` |
| `replace` | `plugin_id`、`target_identifier`、唯一 `staging_name`、新键/哈希/大小、旧键/哈希/大小/文件身份、`expected_row_revision`；`staged|published` 的 identity 同上 | 无 | 以旧 tuple 与精确 revision 做 live CAS，并在同一个 SQLite 事务中切换引用、递增 revision、标记 `referenced` |

数据库 CHECK 必须同时约束状态相关 identity：`prepared` 的 `staging_identity`/`new_identity` 均为空；`staged` 的 `staging_identity` 非空且 `new_identity` 为空；`published|referenced` 的两项 identity 均非空。`done` 可从未创建 staging、清理 staging、发布后登记 GC 或 `referenced` 收敛进入，但 CHECK 必须精确要求 `new_identity IS NULL OR staging_identity IS NOT NULL`，只允许 none/staging-only/both 三种既有阶段形状；`conflict` 不限制这两列并原样保留观测字段。对 create，`plugin_id` 还必须在 `referenced` 非空；对 replace，`plugin_id` 与完整 expected-old tuple/revision 在每个状态都非空。

`create` 顺序：固定托管根目录句柄并逐段 no-follow 验证相对目标键 → 检查 WASM magic/import/export → 校验 manifest/ABI/Capability → 计算大小与 SHA-256；这些预校验失败时用户表和内部 operation/GC ledger 均为零修改。随后按上表持久化含唯一 staging_name 的 `prepared` → 相对于已打开目标父目录以排他方式创建同目录临时普通文件 → flush/fsync 并从同一句柄复核普通文件类型、link count=1、大小与哈希 → 先耐久记录 staging identity 与 `staged` → 以平台等价的原子 no-replace 操作把该已验证 staging 发布到唯一最终键 → fsync 父目录并记录新文件身份/`published` → 在 SQLite 事务提交前从同一受控父目录再次解析最终键并复核身份 → 按上表执行用户可见提交。平台无法保证 no-replace 时必须 fail-closed；无论最终目标是 symlink、hardlink、junction/reparse point、device/FIFO/socket、其他特殊文件，还是并发创建或替换的普通文件，只要目标已经存在或身份不再匹配都必须返回冲突，绝不能覆盖或采用该对象。

用户可见提交之前任一步失败都必须保持 Plugin 行和现有引用不变；用户可见事务只能原子落成完整新状态或完整回滚，不得产生 Plugin 行、引用、revision 与 `referenced` 标记之间的部分状态。进入 `prepared` 后，内部 operation/GC 恢复记录必须耐久保留；用户事务已提交但响应中断时，重放按完整提交收敛。尚未发布且已持久化 staging identity 的临时文件通过受控句柄复验后删除并 fsync；若 staging 创建后、identity 耐久前崩溃，必须把 operation 置 `conflict`，原样保留未知对象并阻断 Store。已经发布但用户事务未提交的唯一对象不得由 operation 分支直接删除；必须先以同一 SQLite 事务登记受保护 GC 并把 operation 标记 `done`，之后只有独立 GC worker 能在零引用/零租约且文件身份精确匹配时清理。身份不同、目标被替换或无法绑定删除时不得触碰路径，GC 项保持 `blocked`。

替换文件不得覆盖旧制品键，也必须先按上述协议发布新的不可变唯一对象。发布前先持久化 `operation_kind=replace` 的 `prepared` 操作记录；`plugin_id`、`target_identifier`、新键/哈希/大小、旧键/哈希/大小/文件身份和预期 `row_revision` 全部非空。发布并 fsync 父目录后补记新文件身份和 `published`。随后在单个 SQLite 事务中以事务开始时读取的旧 `s3_key` 和精确 `row_revision` 作为并发条件，将引用切换到新键、把 revision 加一并将操作状态改为 `referenced`；条件不匹配必须返回并发冲突并保持旧引用，禁止 last-writer-wins。只有新对象发布、父目录持久化、最终身份复核和数据库引用事务提交全部成功后，才可清理旧对象。旧对象清理必须相对于仍打开的受控目录句柄，确认路径仍指向事务前固定的同一普通文件身份、link count=1 且哈希与旧记录一致，并使用能绑定该身份的受保护删除；身份变化或无法证明安全时保留旧对象并登记到持久 GC ledger，不得删除、移动或改写未知对象。清理成功后必须 fsync 父目录；清理失败不回滚已经提交的新引用。未选择文件时保留现有制品和 `s3_key`，其它元数据成功更新也必须递增 `row_revision`。

启动开放 Plugin Store 前必须重放所有非终态 operation。`prepared` 无 staging 时以单一 SQLite 事务标记 `done`；staging 已出现但 identity 未耐久时置 `conflict`、原样保留并阻断 Store。`staged` 相对同一受控父目录同时检查 staging/final：仅 staging 与记录 identity/size/hash 匹配时身份绑定清理、fsync 并完成 operation；仅 final 与 staging identity/size/hash 匹配时重新 fsync、复验并耐久推进 `new_identity/published`；二者均无时完成 operation；双重存在、任一不匹配或身份无法证明时置 `conflict` 并保留全部对象。`published` 只接受 staging 不存在且 final 与持久化 new identity/size/hash 精确匹配，其它组合置 `conflict` 并阻断 Store。仅 final 匹配不得直接补用户状态。create 只确认用户行精确引用 new tuple 的完整提交；无用户行或同 identifier 指向其它 tuple 时不得重建意图，只以“登记 owned new object GC + operation→done”同一事务收敛。replace 只确认当前已引用 new tuple；仍匹配 expected old tuple/revision 时不得重做 live CAS，也采用同一登记事务；其它引用/revision/identity 歧义置 `conflict`。`referenced` 是历史提交事实；create 完成 operation，replace 以“登记旧 tuple GC + operation→done”同事务完成。referenced 后 staging 重现不得回写历史 operation，只登记 incident/GC 并保留对象。operation 完成与 GC 删除严格解耦，operation 永远只允许 `prepared|staged|published|referenced|done|conflict`。GC 只允许 `pending|blocked`，worker 在启动、固定周期和引用/租约释放事件后按 artifact_key 稳定扫描；瞬态引用/租约消失且 identity 仍精确匹配时重试。只有零引用、零运行时租约且身份、link count、大小、哈希全匹配时可删除；目标缺失时重新 fsync 并确认缺失后删 ledger；identity 重现/不匹配或所有权无法证明持续 blocked，除显式人工处置外不得采用新 identity。operation/GC 是内部恢复状态，不进入可移植备份。

所有检查、发布、数据库切换和最终使用都依赖仍打开的根目录、父目录和文件句柄。新导入和替换必须覆盖并发普通文件创建、验证后普通文件替换、链接替换和特殊文件替换等冲突用例；任何冲突都必须 fail-closed，且不得覆盖并发对象、提交指向错误身份的数据库引用或删除不匹配对象。

## 7. 兼容性测试

同一个最小 WASM 与 manifest fixture 必须在 HiveWeb host adapter 和 HiveGUI host adapter 上通过相同成功、拒绝、未知 Capability、timeout、memory、output 和 WASI-denied 用例，并产生等价 envelope。HiveGUI 制品合同测试还必须注入新导入和替换期间的并发普通文件创建、普通文件身份替换、symlink/hardlink/reparse point 与特殊文件替换，证明 no-replace 发布和提交前身份复核稳定返回冲突，且旧引用、并发对象和不匹配对象均未被覆盖或删除。
