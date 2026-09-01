# Performance & Observability Checklist: HiveGUI 独立运行模式

**Purpose**: 验证性能指标、异步处理、资源管理和可观测性相关需求的完整性、清晰性和一致性
**Created**: 2026-06-15
**Feature**: [spec.md](../spec.md)

**Note**: 此检查清单关注**需求质量**——性能与可观测性需求是否完整、清晰、可测量。不测试实现行为。

---

## 2026-07-22 T005 当前基线契约

- [x] 固定十三个 release 本地计时边界：Agent 决策解析到本地动作调度、Agent CRUD/search、Tool 校验到本地执行器启动、100 节点 no-op DAG、Function CRUD/search、Tool CRUD/search，以及 Conversation recent list/session bundle/retention cleanup/running recovery；另由 T121 真实 VisualTestContext 固定一个 debug 三任务组合负载边界。全部 target 的精确定义以共享 `target_specs()`/`--manifest` 为唯一来源；外部 LLM、网络和用户 Function/Plugin 执行耗时必须排除并单列。
- [x] 每个目标固定预热 10 次、测量 100 次，使用 nearest-rank 计算 p50/p95/p99；基线格式版本为 `1`，fixture 为 `hivegui-local-runtime-v1`。
- [x] 环境指纹包含 OS、架构、精确 `rustc --version`、debug/release profile、CPU 型号和逻辑 CPU 数；指纹、fixture、目标定义或样本数不一致时拒绝比较，不得静默更新基线。
- [x] 基线路径固定为 `crates/hivegui/benches/baselines/v1/<target>/<environment>.json`，且必须记录 reviewer、审批日期和源 revision。每个现役目标均由所属故事提供真实 operation；首次获批 Green 前不得伪造样本或基线文件，已有但不可读/损坏的基线必须 fail-closed，不能退化为 `PendingBaseline`。
- [x] 首个基线只能来自所属任务首次获批的 Green 结果；后续 p50/p95/p99 任一项相对基线严格超过 10% 即阻断。仅当例外同时记录 signer、理由、影响范围和未过期复核日期时才可放行。
- [x] 共用入口同时支持同步和异步本地闭包；`--manifest` 只输出目标与环境，`--run <target>` 执行真实 operation、检查绝对预算并比较已有版本化基线；任何入口都不采集占位耗时且不自动写基线文件。

**2026-08-25 T137 当时源码状态**：源码指纹为 `git:dd251229f00dd4ea74df3c1e830753e5521c3c5f+hivegui-source-v1:3478ed47fbb945140cc2f0ae42a1e81ed27ea62f516f64c9ed712e33f2282d3d`。当时七个 target 均使用 release profile、固定 10 次 warmup + 100 次 measured、Linux/x86_64/Rust 1.97.1/12th Gen Intel i9-12900K/20 logical CPUs 原样执行；绝对 p95 预算全部满足，但 Agent 首份 baseline 缺失，Function/Tool 的既有 baseline/exception 绑定旧 source revision，且若干相对回归未获该 source 的有界例外。因此 T137 在当时保持 Pending；禁止改写旧 baseline、复用旧 sidecar、重复碰运气或以 standing authorization 冒充数值审批。

| Target | p50 / p95 / p99 (ns) | 绝对预算 | 同源 gate 结果 |
| --- | ---: | --- | --- |
| `agent_action_dispatch` | 206 / 229 / 247 | p95 ≤ 200,000,000 | `PendingBaseline`；真实 `LocalAgentRuntime::schedule_decision` 已执行，首份 baseline 未审批 |
| `tool_dispatch` | 105,003 / 117,282 / 140,712 | p95 ≤ 50,000,000 | `Passed` |
| `workflow_100_node_noop` | 133,541 / 146,955 / 178,644 | p95 ≤ 100,000,000 | `Passed` |
| `function_crud` | 41,874,775 / 47,617,433 / 81,433,873 | p95 ≤ 1,000,000,000 | `Blocked`；p99 相对旧 baseline >10%，旧 exception source 不匹配 |
| `function_search_page` | 12,905,202 / 28,281,242 / 30,685,439 | p95 ≤ 500,000,000 | `Blocked`；p50 相对旧 baseline >10%，无 exception |
| `tool_crud` | 41,937,909 / 54,118,770 / 65,268,449 | p95 ≤ 1,000,000,000 | `Blocked`；p95/p99 相对旧 baseline >10%，旧 exception source 不匹配且只覆盖 p99 |
| `tool_search_page` | 8,761,421 / 31,127,647 / 31,953,967 | p95 ≤ 500,000,000 | `Blocked`；当前无百分位回归，但 canonical 旧 exception source 不匹配，fail-closed |

**T122 strict-TDD 补充证据**：首次 focused Red 为 `agent_action_dispatch_target_executes_the_production_boundary` 退出 101，原因是 Agent target 返回 `SKIP operation not implemented yet`。最小 Green 新增本地 `ScheduledAgentAction`、`LocalAgentActionScheduler` 与 `LocalAgentRuntime::schedule_decision`，固定 fixture 只解析本地 LLM decision 并验证 immutable Tool/direct-child snapshot 后把动作交给注入 scheduler；不构造 HiveWeb client、不读取 HiveWeb URL。focused 1/1、`local_agent_runtime` 11/11、完整 `support_contract` 28/28 与 `cargo clippy --locked -p hivegui --lib --bench local_runtime -- -D warnings` 均 Green。该行为 Green 不替代 T122 首份 baseline 审批，也不补齐 T116/T126 的 LLM→Tool 执行循环。

## 2026-08-26 T137 当前源码只复跑结果

- **源码/环境**：`git:dd251229f00dd4ea74df3c1e830753e5521c3c5f+hivegui-source-v1:b79808d5b586e1eca31cf54d30bfd6ac2a0682bc33d3cfe902e44b0913fc20c2`；Linux/x86_64、Rust 1.97.1、release、12th Gen Intel i9-12900K、20 logical CPUs。每个 release target 固定 10 warmup + 100 measured；T121 debug 也固定 10+100。
- **查询/正确性门**：`storage_query_plans` **14/14**、`query_count` **7/7**、`search_index_contract` **26/26**，合计 **47/47**。FTS5 `VIRTUAL TABLE INDEX`、normalization/1-2/3+、中英文/大小写/`%`/`_`/引号/FTS operator、固定总排序、无 LIKE/SCAN fallback、EXPLAIN 覆盖与 N+1 固定预算全部 Green。
- **故事固定样本**：DataSource/5 秒连接超时/GlobalConfig 10,000-op/LLM/Tag/Category 100/Capability 100/Plugin pool+GC/Workflow CRUD+search/Skill 的现有固定样本批次 **96/96** Green；两项真实 MySQL 正向 fixture 因缺 CI 专用 `HIVEGUI_TEST_MYSQL_URL` 按合同明确跳过外部连接，unreachable 5 秒 timeout 真实通过。Agent/Conversation 非 release 功能复跑另为 **33 passed / 0 failed / 2 release-only ignored**。
- **13 个 canonical release 报告**：绝对 p95 预算 **13/13 全部满足**。11 个 target 当前直接 `Passed`：`agent_action_dispatch`=`228/252/266ns`，`agent_crud`=`44,358,030/47,424,216/49,536,030ns`，`agent_search_page`=`4,454,400/4,899,282/5,185,706ns`，`workflow_100_node_noop`=`135,753/153,922/180,481ns`，`function_crud`=`42,302,886/45,201,981/52,396,324ns`，`function_search_page`=`9,487,675/28,309,480/29,111,128ns`，`tool_search_page`=`9,535,561/35,445,811/36,423,801ns`，`conversation_list_recent`=`129,581/158,002/174,358ns`，`conversation_session_bundle`=`286,406/535,933/654,342ns`，`conversation_retention_cleanup`=`36,046,331/38,227,869/39,925,022ns`，`conversation_running_recovery`=`36,175,310/38,278,263/40,245,032ns`（均为 p50/p95/p99）。
- **历史 sidecar 收口**：Agent CRUD、Function CRUD、Tool CRUD/search 与 Conversation list 的 5 份 canonical exception 均绑定旧 source。先证明当前无回归的 4 项后，把全部 5 份原文件按旧 source 短摘要旁移为 `.superseded.json` 审计记录，未改内容、未重签、未删除历史；当前 canonical path 不再让 dormant stale approval 阻断无回归报告。Tool CRUD 随后的独立样本暴露真实 p99 tail，因此没有恢复旧 sidecar或跨 source 复用旧签字。
- **当前阻断 1 · Tool dispatch**：`106,358/136,728/152,140ns`，p99 相对 baseline `129,398ns` 增长约 17.6%；绝对 p95 `0.137ms << 50ms`，但没有当前 source 的 signer/reason/scope/review_due 有界例外，按 T005 必须 Blocked。
- **当前阻断 2 · Tool CRUD**：独立样本 `43,736,548/46,857,670/76,198,652ns`，p99 相对 baseline `50,496,557ns` 增长约 50.9%；同一轮前一份报告 p99 仅 `51,885,200ns`，证明 durable SQLite p99 tail 抖动，但这不能替代当前 source 的明确有界签字。旧 user-approved 85ms sidecar 已保存在 `.exception.9616a182.superseded.json`，不得自动跨 source 生效。
- **当前阻断 3 · T121 debug 组合负载**：第一次完整 accessibility 复跑得到 `12,877,997/15,044,747/15,998,521ns`，p95 相对 baseline `13,474,426ns` 增长约 11.7%，故 76/77 fail-closed；系统空闲后 focused 1/1 与再次完整 77/77 均通过。绝对 p95 仍 `15.0ms << 100ms`，但“再跑一次刚好通过”不能消除已观察的同源相对回归或代替例外审批。
- **结论**：T137 保持 **Pending / release blocked**。需要 owner/reviewer 为当前 source 明确签署 Tool dispatch p99、Tool CRUD p99 与（若继续允许其自然抖动）T121 p95 的有界 exception，或回到各 owner 任务修正不稳定计时/生产尾延迟后重新只复跑；本批未写 baseline、未生成 exception，也未用 standing authorization 冒充数值审批。

### 2026-08-26 Skill owner 修复后的最终源码复验

- **源码/方法**：最终 source revision=`git:dd251229f00dd4ea74df3c1e830753e5521c3c5f+hivegui-source-v1:d21635c2afd6082fe2a4b1c94ee07a2e5ce57ffb33dba279d1c7c891851780f8`；Linux/x86_64、Rust 1.97.1、release、12th Gen Intel i9-12900K、20 logical CPUs，全部 target 仍为 10 warmup + 100 measured。没有写 baseline、没有恢复 superseded sidecar、没有生成新 exception。
- **当前 direct Passed（8/13）**：`agent_action_dispatch=231/256/275ns`；`tool_dispatch=105,691/109,249/109,857ns`；`workflow_100_node_noop=136,639/186,367/233,615ns`；`function_crud=43,253,608/53,662,077/66,061,790ns`；`tool_crud=41,544,152/45,833,099/49,847,248ns`；`tool_search_page=8,820,575/33,246,852/34,404,046ns`；`conversation_list_recent=149,104/194,458/235,794ns`；`conversation_session_bundle=306,598/380,168/411,147ns`（均为 p50/p95/p99）。其中 Tool dispatch 首轮曾因 p99=166,952ns Blocked，独立同源复验 Passed；该失败仍保留在审计事实中。
- **当前 Blocked（5/13）**：`agent_crud=46,159,440/63,390,489/78,148,960ns`（p95/p99）；`agent_search_page=4,873,910/6,475,948/6,867,058ns`（p95/p99）；`function_search_page=14,868,065/33,964,325/38,278,823ns`（p50/p95/p99）；`conversation_retention_cleanup=56,807,883/65,715,325/806,219,724ns`（p50/p95/p99）；`conversation_running_recovery=57,024,298/66,520,111/147,795,822ns`（p50/p95/p99）。所有五项绝对 p95 仍明显满足合同；但相对 10% 门禁没有绑定当前 source 的逐百分位数值上限签字，因此共享 evaluator 正确 exit 1。
- **T121/current UI**：Skill owner 修复后的完整 `accessibility` 78/78，三任务组合负载相对与绝对门禁均通过；旧 77-test source 的首次抖动不跨 source 生成例外。
- **结论**：T137 仍为 **Pending / release blocked**。当前阻断已收敛为上述五个 target 的同源相对门；`tool_crud` 当前 direct Passed，因此用户过去的 85ms 数值授权既不需要也不得跨 source 重建。若不修改 target/baseline 合同，只能由 reviewer 对当前 source 的精确百分位、上限、理由、范围和到期日签署 sidecar；“后继无需确认”是执行授权，不冒充性能数值签字。

### 2026-08-26 中间报告与冻结源码最终状态

- **中间 source**：`git:c98cfe22b63f87337455850319bec34346fb5beb+hivegui-source-v1:5157237dc6205afff342a7a46c5494eac47c7693f6f751da0796f73dffbff13c` 上，13 个 canonical release target 的绝对 p95 预算 13/13 满足、12/13 direct Passed。唯一 blocker 为 `tool_search_page=11,772,233/27,109,175/28,117,463ns`：p50 超过 baseline `9,539,723ns` 的 10% cap `10,493,695ns`。其余 12 项为：`agent_action_dispatch=228/266/280ns`、`agent_crud=20,454,334/27,590,513/33,828,616ns`、`agent_search_page=606,258/800,796/832,610ns`、`tool_dispatch=107,747/114,883/121,473ns`、`workflow_100_node_noop=130,677/163,793/189,872ns`、`function_crud=15,742,728/25,734,796/27,952,188ns`、`function_search_page=9,052,783/21,619,546/22,019,630ns`、`tool_crud=24,098,776/41,788,384/44,374,920ns`、`conversation_list_recent=137,497/178,500/283,317ns`、`conversation_session_bundle=304,335/362,131/498,208ns`、`conversation_retention_cleanup=21,365,158/28,830,539/30,476,551ns`、`conversation_running_recovery=19,166,584/31,634,499/35,152,183ns`。
- **Tool search 诊断**：固定 50/50 mixed route 使 p50 位于两条分布的统计分界；未找到不改变目标/fixture 合同且安全的最小生产优化。没有重跑到偶然通过，也没有写 baseline/exception。
- **冻结 source**：最终指纹为 `git:c98cfe22b63f87337455850319bec34346fb5beb+hivegui-source-v1:a1696bdb89fdc2a0a757091742aa9467d151d617f91e2801ce80d77da3244a2e`。因 5157 报告后生产源码继续变化，13 个 release target 未在该指纹上复跑，故中间 12/13 不能作为最终 T137 证据。
- **T121 最终单次串行结果**：accessibility 77/78；组合负载=`13,690,409/17,255,082/19,390,178ns`，baseline=`12,260,815/13,474,426/16,001,232ns`，p50/p95/p99 分别约 +11.7%/+28.1%/+21.2%，三项相对门均 Blocked；绝对 p95 约 17.3ms，仍小于 100ms。
- **结论**：T137 保持 **Pending / release blocked**。冻结 source 缺 13-target 最终报告且 T121 当前失败；未创建、恢复、替换或修改任何 baseline/exception。

### 2026-08-26 deterministic v2 候选（未数值审批）

- **T005 首份绝对门修复**：缺 baseline 且 p95 超绝对预算的 focused Red 为 `PendingBaseline` 而非 `Blocked`；最小 Green 后 focused 1/1、完整 support 30/30。任何新 target 首份报告只有先满足绝对预算才可成为 Pending 候选。
- **Tool search pair-v2**：活动 release target 总数仍为 13；旧 `tool_search_page` 退出 active manifest，其 baseline/历史 sidecar 原样保留。新 `tool_search_page_pair_v2` 的每个 warmup/measured sample 在同一 wall-clock 窗口、同一页深按 `filtered → unfiltered` 顺序执行两个真实 `ToolStore::list`，不除以 2、不做 per-request normalization。single canonical candidate source=`...62b4e104...`，p50/p95/p99=`27,553,171/35,207,202/36,889,904ns`，绝对 p95≤500ms；结果 `PendingBaseline`，未写 baseline/exception。
- **T121 deterministic v2**：旧无限循环实际 Workflow `927≠110`；新 `us13_combined_ui_feedback_round_v2` 固定每代一项 Workflow+一项 backup prevalidation。focused 候选=`13,056,881/15,323,516/16,495,112ns`；最终 source `...62b4e104...` 完整 accessibility 78/78，最终报告=`13,280,100/20,407,334/38,782,823ns`，绝对 p95<100ms、Stop 100%，但新 ID 仍为 `PendingBaseline`。
- **状态**：上述两个新 identity 的非数值合同均已 Red→review→Green；精确候选数值尚未由 reviewer 单独批准，且最终 source 完整 13-target/T121 汇合尚未执行。因此 T107/T121/T137/T145/T147 保持 Pending；standing authorization 不构成 baseline 数值签字。

### 2026-08-26 source `62b4e104…` 最终矩阵

- **共同条件**：Linux/x86_64、Rust 1.97.1、release、i9-12900K、20 logical CPUs；每个 target 10 warmup+100 measured。workspace strict Clippy、fmt、diff-check Green；13/13 绝对 p95 预算全部满足。
- **Passed（9）**：`agent_action_dispatch=228/262/267ns`；`agent_search_page=552,648/764,269/794,979ns`；`tool_dispatch=102,540/111,017/132,615ns`；`workflow_100_node_noop=127,456/154,221/193,131ns`；`function_crud=14,148,466/22,645,654/25,803,493ns`；`tool_crud=15,934,038/25,423,024/27,179,343ns`；`conversation_list_recent=133,578/172,982/187,588ns`；`conversation_retention_cleanup=13,076,425/17,911,421/23,357,345ns`；`conversation_running_recovery=12,579,297/20,573,188/23,679,120ns`。
- **Blocked（3）**：`agent_crud=18,739,748/28,081,225/68,113,157ns`，仅 p99 超 baseline `52,787,314ns`；`function_search_page=10,093,821/23,712,185/24,731,404ns`，仅 p50 超 baseline `8,958,727ns`；`conversation_session_bundle=333,840/423,174/741,392ns`，仅 p99 超 baseline `615,282ns`。三项均无当前 source 有界例外，正确 fail-closed；没有重跑碰运气。
- **PendingBaseline（1）**：新 `tool_search_page_pair_v2=27,553,171/35,207,202/36,889,904ns`；旧 mixed identity 制品零修改。debug `us13_combined_ui_feedback_round_v2=13,280,100/20,407,334/38,782,823ns` 同样 PendingBaseline。
- **结论**：当前矩阵为 **9 Passed / 3 Blocked / 1 PendingBaseline**，外加 T121 debug PendingBaseline；T137/T145/T147 保持 Pending。接下来只允许 owner Red→review→Green 修正可证明的实现/测量缺口，或对最终 source 精确数值单独签字；不得重复测量抹除本节。

### 2026-08-26 source `a220ec13…` v2 最终候选（未数值审批）

- **共同条件**：source=`git:c98cfe22b63f87337455850319bec34346fb5beb+hivegui-source-v1:a220ec13deac9ddc5a95f75e15d80e41ce3bf15cf9ec21962dba0b0c38349a68`；Linux/x86_64、Rust 1.97.1、release、12th Gen Intel i9-12900K、20 logical CPUs；每个 release target 固定 10 warmup+100 measured。support 31/31、dispatch 5/5、Function 19/19+1 ignored、Conversation 16/16+1 ignored、Agent/query/search/plan 66 pass+1 ignored、strict all-target Clippy、fmt、diff-check 均 Green；最终 `cargo +1.97.1 test --locked -p hivegui --tests --no-fail-fast -- --test-threads=1` exit 0。
- **首份 release candidates**：`tool_dispatch_batched_v2=105,947/128,576/144,562ns`（p95≤50ms）；`function_search_page_pair_v2=25,488,517/32,755,190/33,837,460ns`（p95≤500ms）；`tool_search_page_pair_v2=30,483,816/38,892,239/48,487,886ns`（p95≤500ms）；`conversation_session_bundle_batched_v2=304,068/340,166/388,419ns`（p95≤1s）。四项绝对预算均 Green，且都因 canonical path 缺 baseline 正确返回 `PendingBaseline`。
- **T121 debug candidate**：完整 accessibility 78/78；`us13_combined_ui_feedback_round_v2=13,134,009/14,853,274/17,226,704ns`，绝对 p95≤100ms、每样本<250ms、Stop 100%，同样 `PendingBaseline`。
- **历史 Red 不被抹除**：source `96af29e0…` 的完整 13-target 单次矩阵为 9 Passed / 1 Blocked / 3 Pending；唯一 Blocked 是旧 `tool_dispatch=119,726/151,754/180,945ns` 的 p95/p99。该 Red促成准确完整边界、checked error propagation与新 identity，而不是重跑。Agent fetch 优化后同 source 的 `agent_crud=19,717,018/26,549,860/46,766,059ns` direct Passed；旧 mixed Function/Tool/Conversation target 和所有制品均零修改。
- **2026-08-26 当时状态（已由下节取代）**：当时没有任何相对回归例外被自动生成，也没有 baseline 写入。五个精确候选必须取得单独数值审批并在完全相同 source/environment 同源复跑为 `Passed`；在此之前 T089/T103/T107/T117/T121/T137/T145/T147 均保持 Pending，不能把 13/13 绝对预算 Green 倒推为发布 Green。

### 2026-08-27 `a220ec13…` 数值审批、同源复跑与 T117 列表 v2

- **五份首批 baseline 审批**：user 明确回复“批准 a220ec13 的上述五组数值作为首份基线”。以 `reviewer=user`、`approved_at=2026-08-27` 与完整 source `git:c98cfe22b63f87337455850319bec34346fb5beb+hivegui-source-v1:a220ec13deac9ddc5a95f75e15d80e41ce3bf15cf9ec21962dba0b0c38349a68` 写入：`tool_dispatch_batched_v2=105,947/128,576/144,562ns`、`function_search_page_pair_v2=25,488,517/32,755,190/33,837,460ns`、`tool_search_page_pair_v2=30,483,816/38,892,239/48,487,886ns`、`conversation_session_bundle_batched_v2=304,068/340,166/388,419ns`、debug `us13_combined_ui_feedback_round_v2=13,134,009/14,853,274/17,226,704ns`。
- **同源门禁**：四个 release target 与 T121 debug target 在完全相同 source/environment 原样复跑，evaluator 均返回 `Passed`；未生成 regression exception。因此 T089、T103、T107、T121 闭合，T117 的 session-bundle v2 子门禁闭合。
- **`a220ec13…` 十三目标矩阵保留的 Red**：上述四个 release v2 与其余九个 target 同批汇合为 **12 Passed / 1 Blocked**。唯一 Blocked 为旧 `conversation_list_recent=123,708/172,266/1,021,339ns`；其 baseline=`141,978/196,008/301,598ns`，p50/p95 下降，但 p99 约 +238.6%，超过 10% cap 约 `331,758ns`。该首次 Red 保留，未重跑至偶然通过，也未对旧 identity 建立新例外。
- **reviewer-approved 新 active identity**：`conversation_list_recent_batched_v2` 以固定 batch=16 测量同一 125-session fixture 上的真实 recent-list 请求，每个请求使用自己的 current-UTC 上界，在真实 indexed/encrypted 20-row page 与 newest-first 总序物化后按请求归一化。新 source=`git:c98cfe22b63f87337455850319bec34346fb5beb+hivegui-source-v1:4a0cbc15e19a566d0e78ac7898449ffe7c91b63d1e27f6d758226e287c24a190`，10 warmup+100 measured 的首份候选为 `121,384/144,267/156,741ns`，绝对 p95≤1s Green，但 canonical path 尚无 baseline，故精确为 `PendingBaseline`。旧 `conversation_list_recent` baseline/历史证据保留且不会被新 identity 续接。
- **当时结论（已由下节取代）**：最终 active manifest 仍为 13 个 release target，但已换入 `conversation_list_recent_batched_v2`。当时必须先取得其首份候选的单独数值审批，再完整复跑 13-target 矩阵；不能把 `a220ec13…` 的 12/13 或新 target 的绝对预算 Green 倒推为最终发布 Green。

### 2026-08-27 `4a0cbc15…` baseline 审批与 `9e392445…` source-owned 矩阵

- **list-v2 首份 baseline 已获批**：user 明确回复“批准 4a0cbc15 的 `conversation_list_recent_batched_v2 121384/144267/156741ns` 作为首份基线”。完整 baseline source 为 `git:c98cfe22b63f87337455850319bec34346fb5beb+hivegui-source-v1:4a0cbc15e19a566d0e78ac7898449ffe7c91b63d1e27f6d758226e287c24a190`；canonical baseline 记录 p50/p95/p99=`121,384/144,267/156,741ns`、`reviewer=user`、`approved_at=2026-08-27`，未创建 regression exception。旧 `conversation_list_recent` baseline、`a220ec13…` 12/13 Red 与更早 4a0/original 11/2 证据全部保留。
- **`4a0cbc15…` 原始 fixed-order 证据不被抹除**：baseline 写入后的 list-v2 同源单目标验证=`130,001/147,768/164,515ns`，evaluator=`Passed`。随后手工固定顺序的 13-target 唯一执行为 **11 Passed / 2 Blocked**：list-v2=`238,251/342,313/386,940ns`，三个相对百分位均 Blocked；`conversation_running_recovery=13,346,855/23,946,305/50,452,321ns`，仅 p99 比 baseline=`44,748,133ns` 高约 `12.747%`、超过 cap=`49,222,946.3ns`，绝对 p95 Green。该执行无 retry、无 exception，后续编排 Green 与新 source 矩阵均不改写它。
- **矩阵编排 Green**：source-owned `--run-matrix` 固定经评审的 13-target 干扰最小化顺序，以同一精确 bench 可执行文件逐项启动新子进程；子进程失败或 spawn error 也继续至 13/13，首尾 source drift 必须使聚合失败，affinity 明确为 `inherited/uncontrolled`。focused matrix contracts 6/6、完整 `support_contract` 38/38 Green；该编排批次没有改变 13 个 TargetSpec、target runner、evaluator 或 canonical baseline/exception。
- **唯一 canonical 最终矩阵**：完整 source=`git:c98cfe22b63f87337455850319bec34346fb5beb+hivegui-source-v1:9e39244542c3aa61d5e39a3310054187c085c8a70b295eeaffdcc8826257fb0c`。顶层 envelope 精确为 `source_stable=true`、`status=failed`；全部 13 个 target 各执行一次，结果 **12 Passed / 1 Blocked**，无 retry。
- **唯一 Blocked**：`conversation_list_recent_batched_v2=136,200/150,782/158,496ns`，对获批 baseline=`121,384/144,267/156,741ns` 仅 p50 回归 `+12.2059%`，超过 10% cap=`133,522.4ns`；p95/p99 相对门通过，绝对 p95≤1s Green。没有 exception，故 evaluator 与顶层 envelope 如实保持失败。
- **其余目标证据**：其余 12 项均 `Passed`；其中 `conversation_running_recovery=12,469,530/20,607,813/21,892,861ns` Passed。该 Green 不覆盖 list-v2 的相对 p50 Red。
- **当时结论（已由后续两节取代）**：T117/T137/T145/T147 均保持 Pending；不得通过 canonical 重跑、其余 12/13 Passed、绝对 p95 Green 或未签字例外抹除本次 Red。T119/T130、T136/T138/T139/T142 的既有独立 blocker 状态不变。

### 2026-08-27 `9e392445…` p50 有期例外与同源 13/13 验证

- **精确审批与制品**：user 明确批准 source `git:c98cfe22b63f87337455850319bec34346fb5beb+hivegui-source-v1:9e39244542c3aa61d5e39a3310054187c085c8a70b295eeaffdcc8826257fb0c` 的 `conversation_list_recent_batched_v2` p50 有期例外：baseline=`121,384ns`、observed/current cap=`136,200ns`、`approved_at=2026-08-27`、`review_due=2026-09-27`。canonical sidecar SHA-256=`ec7f8bdef3bcac3cd45ba22b0c61d2d44753e4c6de504a4ac58626ddce4f6876`；范围只覆盖该精确 source/target/p50，且 cap 零余量，不覆盖 p95、p99、绝对预算、其他 target 或后续 source。
- **历史 Red 保留**：例外审批前的唯一 source-owned matrix 仍为 `source_stable=true`、`status=failed`、12 Passed / 1 Blocked；list-v2=`136,200/150,782/158,496ns` 的 p50 Red 及更早 `4a0cbc15…` 11/2 Red 均未删除、未追认为 Passed。
- **例外落盘后的同源验证**：同一 source-owned `--run-matrix` 为全部 13 个 target 各启动一个新子进程；顶层 envelope=`source_stable=true`、`status=passed`，结果为 **12 Passed / 1 ApprovedException = 13/13 accepted**。唯一 `ApprovedException` 是 list-v2 p50；本次 current p50=`135,459ns` 未超过获批 cap=`136,200ns`，p95/p99 与绝对预算仍独立 Green。

| Ordinal | Target | p50 / p95 / p99 (ns) | Gate |
| ---: | --- | ---: | --- |
| 1 | `agent_action_dispatch` | 230 / 266 / 271 | `Passed` |
| 2 | `tool_dispatch_batched_v2` | 103,748 / 115,664 / 122,807 | `Passed` |
| 3 | `conversation_list_recent_batched_v2` | 135,459 / 150,816 / 157,170 | `ApprovedException`（仅 p50） |
| 4 | `workflow_100_node_noop` | 129,154 / 144,245 / 176,699 | `Passed` |
| 5 | `conversation_session_bundle_batched_v2` | 299,920 / 368,588 / 396,353 | `Passed` |
| 6 | `agent_search_page` | 551,198 / 762,732 / 811,576 | `Passed` |
| 7 | `conversation_running_recovery` | 12,704,423 / 20,518,562 / 21,550,696 | `Passed` |
| 8 | `conversation_retention_cleanup` | 12,611,418 / 17,266,011 / 22,445,383 | `Passed` |
| 9 | `function_crud` | 14,175,804 / 23,205,071 / 23,591,668 | `Passed` |
| 10 | `tool_crud` | 14,552,707 / 23,343,556 / 24,472,550 | `Passed` |
| 11 | `agent_crud` | 18,439,870 / 24,858,746 / 29,243,528 | `Passed` |
| 12 | `function_search_page_pair_v2` | 23,815,742 / 31,943,638 / 34,877,025 | `Passed` |
| 13 | `tool_search_page_pair_v2` | 26,588,329 / 34,490,183 / 35,249,847 | `Passed` |

- **T137 结论**：既有固定故事样本、FTS5/normalization/fail-closed、EXPLAIN/索引覆盖、N+1 与 T121 debug 证据保持 Green；上表补齐最终 source 的 13 个 release p50/p95/p99、绝对预算和版本化基线比较。唯一 >10% 回归现有明确 signer/reason/scope/review_due 且同源验证未越 cap，因此 T117 与 T137 的性能门闭合。
- **当时不扩大发布结论（T145 部分已由下节取代）**：`support_contract::canonical_release_baselines_match_all_current_targets_without_exceptions` 在 sidecar 存在后实测 0/1 Red；该测试的“canonical matrix 必须零 exception”策略与 T005 明确允许有签字、有范围、有到期日例外的合同发生治理冲突。该 Red 不撤销 evaluator 的 `ApprovedException` 或本节 13/13 结果，但当时必须由 owner 收敛后才能闭合 T145/T147；T119/T130 及其下游备份/平台门也保持 Pending。

### 2026-08-27 当前 source `53e3ae2b…` 的 direct 13/13 最终矩阵

- **源码与执行约束**：完整 source revision 为 `git:c98cfe22b63f87337455850319bec34346fb5beb+hivegui-source-v1:53e3ae2b38876bc54e9e311793682e7f850f76a7e7f22035a5102879548174e6`；非计时 `--source-revision` 与同一 bench 可执行文件连续复核一致。冻结 Cargo/toolchain/`.cargo`/`crates`/`third_party` 后，只执行一次 `cargo +1.97.1 bench --locked -p hivegui --bench local_runtime -- --run-matrix`，未重跑。环境为 Linux/x86_64、Rust 1.97.1、release、20 logical CPUs、10 warmup + 100 measured；affinity=`inherited/uncontrolled`。
- **顶层结果**：`source_stable=true`、`status=passed`。13 个 active target 全部是直接 `Passed`，没有 `ApprovedException`、`Blocked` 或 `PendingBaseline`；所有旧 `*.exception.json` 均已旁移为 `.superseded.json`，`9e392445…` 的有期例外只保留为历史审计证据，绝不跨 source 复用。

| Ordinal | Target | p50 / p95 / p99 (ns) | Gate |
| ---: | --- | ---: | --- |
| 1 | `agent_action_dispatch` | 225 / 252 / 266 | `Passed` |
| 2 | `tool_dispatch_batched_v2` | 98,116 / 100,151 / 101,542 | `Passed` |
| 3 | `conversation_list_recent_batched_v2` | 129,592 / 145,416 / 152,028 | `Passed` |
| 4 | `workflow_100_node_noop` | 127,647 / 141,287 / 165,463 | `Passed` |
| 5 | `conversation_session_bundle_batched_v2` | 284,199 / 309,159 / 319,442 | `Passed` |
| 6 | `agent_search_page` | 520,843 / 733,963 / 739,935 | `Passed` |
| 7 | `conversation_running_recovery` | 12,447,998 / 20,684,744 / 21,161,298 | `Passed` |
| 8 | `conversation_retention_cleanup` | 12,595,537 / 17,000,118 / 17,912,566 | `Passed` |
| 9 | `function_crud` | 14,188,902 / 22,947,233 / 29,127,127 | `Passed` |
| 10 | `tool_crud` | 14,590,734 / 23,051,458 / 23,569,402 | `Passed` |
| 11 | `agent_crud` | 16,126,886 / 23,806,360 / 24,490,760 | `Passed` |
| 12 | `function_search_page_pair_v2` | 23,848,794 / 29,863,772 / 31,084,725 | `Passed` |
| 13 | `tool_search_page_pair_v2` | 27,010,787 / 34,817,912 / 35,176,886 | `Passed` |

- **治理与任务结论**：现役 `support_contract::canonical_release_baselines_match_all_current_targets` 实测 1/1 Green，替代了上节已记录的永久零例外错误合同。固定故事样本、FTS5/normalization fail-closed、EXPLAIN/索引、N+1、绝对预算及全部版本化比较均 Green；因此 T137 在当前 source 上保持 Closed，T145 的性能部分亦 Green。该结果不关闭 T119/T130 的文件身份/跨平台边界，也不替代 T139/T142 的平台辅助技术证据。

以下 2026-06-15 条目是旧规格的需求质量审阅记录；其中 50 节点、旧任务号和旧预算不得覆盖上述当前契约。

---

## 2026-06-15 历史需求质量审阅

## Performance Metrics Clarity

- [x] CHK001 SC-001 要求"首次启动后 3 分钟内完成代理配置并开始对话"——"开始对话"的精确定义是什么（发送第一条消息？收到第一个 token？完成首次完整回复？）[Clarity, Spec §SC-001]
  > **评估**: "开始对话"在验收场景中定义为用户成功发送第一条消息并获得回复。spec §US1 验收场景 1-4 + Conversation (Phase 7) 覆盖了完整流程。T094 端到端计时测试已明确测量端点。

- [x] CHK002 SC-002 要求"典型数据量下 1 秒内完成"——"典型数据量"是否定义为具体数值（如 10 个 agent、100 个 function）？SC-004 定义了上限但 SC-002 引用的是"典型"而非上限。[Clarity, Spec §SC-002]
  > **评估**: "典型"与"上限"的有意区分——SC-002 覆盖日常使用场景，SC-004 覆盖极限场景。T095 基准测试（100 次 CRUD）定义了测试数据量。可接受的设计选择。

- [x] CHK003 SC-003 要求"50 个节点的工作流无 UI 卡顿"——是否定义了工作流编辑器中的具体操作（节点拖拽、连线、平移/缩放画布）各自的可接受延迟？[Clarity, Spec §SC-003]
  > **评估**: T096 明确定义了测试操作范围（画布平移/缩放/节点拖拽）和可测量标准（帧率 ≥30fps）。足够清晰。

- [x] CHK004 SC-007 要求"输入延迟低于 200ms"——输入延迟的测量起点和终点是否定义（按键按下 → 字符显示？IME 组合输入是否单独考虑？）[Clarity, Spec §SC-007]
  > **评估**: T098 定义测量方法为"文本输入延迟"。桌面应用标准测量法为 keypress → glyph on screen。IME 组合输入是操作系统层面的处理，不属于应用延迟范围。

- [x] CHK005 SC-008 要求"安装包 ≤200MB"——是否定义了测量范围（仅 hivegui 二进制？含所有依赖 .so/.dll？含 WASM 插件？含 SQLite ？）[Clarity, Spec §SC-008]
  > **评估**: T099 明确测量方法为 `cargo build --release` 后的 hivegui 二进制 + 依赖总大小。spec 进一步澄清为 "不含用户数据和插件"。范围已清晰定义。

- [x] CHK006 SC-004 的数据量上限（1000 agent + 500 workflow + 2000 function + 10000 game）——这些数字是否有来源依据，还是任意设定的目标？需求是否应注明"或更大"以允许未来扩展？[Clarity, Spec §SC-004]
  > **评估**: 这些数字来源于 spec §假设条件中对桌面应用存储规模的合理估计（≤500MB 总存储）。T097 负载测试以此为目标。属于合理的设计目标而非任意设定。

## Async & Concurrency Requirements

- [x] CHK007 FR-015 要求"后台任务异步运行不阻塞 GUI 线程"——是否定义了需异步运行的任务类型完整清单（LLM 调用、工作流执行、插件调用、文件导入、数据库迁移）？[Completeness, Spec §FR-015]
  > **评估**: FR-015 明确列出 LLM 调用、工作流执行、插件调用。T091 补充了数据库操作和退出清理。gpui + tokio 异步架构天然支持所有阻塞操作在后台执行。覆盖充分。

- [x] CHK008 多个后台任务并发时的行为需求是否定义（多个工作流同时执行？LLM 调用 + 插件调用同时进行？）是否有并发上限需求？[Gap, Spec §FR-015]
  > **评估**: 桌面应用场景为单用户操作，多任务并发自然发生（如同时运行工作流和对话）。限流/排队由 tokio runtime 自然处理。无显式并发上限是合理简化——桌面应用不存在服务端多租户并发风险。

- [x] CHK009 后台任务取消需求是否定义（用户关闭对话面板时是否取消进行中的 LLM 调用？关闭工作流编辑器时是否停止执行中的工作流？）[Gap, Spec §Edge Cases:面板切换]
  > **评估**: Spec §Edge Cases 已定义"后台任务应继续执行，用户应能返回查看结果"。设计选择是"继续执行"而非"取消"——符合桌面应用用户期望。

- [x] CHK010 LLM 流式响应的缓冲区需求是否定义（SSE 事件积压时的处理——丢弃旧事件 vs 暂停读取 vs 增大缓冲）？[Gap, Spec §FR-008]
  > **评估**: SSE 流式响应在单用户桌面应用中极少积压。hivegui 现有 streaming.rs 已有 SSE 处理逻辑。属于实现细节，无需在 spec 层面定义。

## Resource Management

- [x] CHK011 SQLite 连接池大小是否在需求中定义（最大连接数、超时配置、连接复用策略）？当前 data-model 和 plan 未提及池配置。[Gap, Data Model, Plan §Technical Context]
  > **评估**: sqlx SQLite 连接池默认配置（max_connections=5）适合桌面应用。单用户场景无需调整。属于实现默认值覆盖的合理范围。

- [x] CHK012 WASM 实例池的 PoolConfig 是否在需求中定义（每插件最大实例数、总实例上限、空闲超时回收）？plan 提到 `PoolConfig::from_env()` 但 spec 无配置需求。[Gap, Spec §FR-005]
  > **评估**: 复用 hiveweb 的 PoolConfig，通过环境变量配置。plan §Technical Context 已确认此策略。桌面应用场景下默认值充分。

- [x] CHK013 磁盘空间使用上限是否在需求中定义（SQLite 数据库最大大小、WASM 插件存储上限、日志文件轮转策略）？SC-008 仅约束安装包，不约束运行时。[Gap, Spec §SC-008]
  > **评估**: Spec §假设条件定义"通常不超过 500MB"。SQLite 自动增长，WASM 有 50MB 单文件限制。桌面应用由操作系统管理磁盘空间——应用层限制不是必须的。

- [x] CHK014 内存使用上限是否在需求中定义（加载大游戏数据集 10000+ 条目时的内存占用、多个 WASM 插件同时加载时的内存）？[Gap]
  > **评估**: SC-004 的分页和增量搜索策略（spec §Edge Cases）已隐式管理内存——UI 只加载可见数据。WASM 实例池有上限。桌面应用内存管理由 OS 负责，应用层上限非必须。

## Startup & Shutdown

- [x] CHK015 应用启动时间是否在需求中定义（从双击图标到显示 LockScreen/主界面的最大时间）？SC-001 覆盖了"首次配置"场景，但日常启动的性能目标未定义。[Gap, Spec §SC-001]
  > **评估**: 日常启动仅需加载 SQLite + 验证密码，预期 <2s。SC-001 的 3 分钟目标已覆盖最慢路径（首次配置）。日常启动定义非关键需求——在首次配置后自然更快。

- [x] CHK016 应用退出清理的需求完整性——T091 提到"关闭连接池、停止实例池、保存未完成状态"——"保存未完成状态"的具体内容是否在 spec 需求中定义（对话草稿、未保存的工作流编辑、进行中的执行结果）？[Clarity, Spec §Edge Cases:面板切换]
  > **评估**: T091 定义为"保存未完成状态"，由各 Store 模块自行实现。对话和工作流的"进行中继续执行"已在 Edge Cases 中覆盖。关闭时保存草稿属于 UX 增强而非核心需求。

## Observability

- [x] CHK017 Constitution §VI 要求"structured logs (JSON or equivalent key-value format)"——hivegui 当前使用 tracing（已支持 JSON 格式）。spec 中是否有明确的可观测性需求（日志级别、格式、字段标准）？[Consistency, Constitution §VI, Spec §Requirements]
  > **评估**: Plan §Constitution Check 确认 hivegui 已有 tracing 日志系统。T090 扩展日志覆盖（含 request_id）。Constitution 要求已通过现有基础设施满足。

- [x] CHK018 SC 验证任务（T094–T099）是否定义了测试结果的可观测性需求（性能基准报告格式、持续监控、回归告警阈值）？[Gap, Tasks §SC Verification]
  > **评估**: T094-T099 为一次性验证任务，验证 spec 中的 SC 是否达成。持续监控/回归告警是 CI 基础设施层关注点，不在此 feature 范围。

- [x] CHK019 LLM 调用的可观测性需求是否定义（记录每次调用的模型、token 用量、延迟、状态）？FR-011 的 usage_records 覆盖了部分，但实时监控/告警需求是否在范围内？[Gap, Spec §FR-011]
  > **评估**: FR-011 + usage_records 表 + T069 集成使用记录已覆盖。实时监控/告警是服务端运维需求，桌面应用不需要。

- [x] CHK020 WASM 插件执行的审计需求是否定义（记录每次插件调用的函数名、耗时、成功/失败）？hiveweb 有 runtime_audit 模块，hivegui 的需求是否对齐？[Gap, Spec §FR-005]
  > **评估**: T062 复用 hiveweb 的 invoker（含审计）。hiveweb runtime_audit 的审计能力自然继承。显式需求非必须。

## Degradation & Limits

- [x] CHK021 系统在资源受限环境（低内存、磁盘满、CPU 受限）下的降级需求是否定义（功能降级顺序、用户提示、最小可运行配置）？[Gap]
  > **评估**: 桌面应用由操作系统管理资源。磁盘满时 SQLite 写入失败返回 StoreError，已通过 T011/T089 覆盖。低内存/CPU 受限由 OS 调度处理。显式降级策略非桌面应用必需。

- [x] CHK022 SC-004 定义了数据量上限——超过上限时的行为需求是否定义（拒绝新数据？性能下降警告？自动归档？）[Gap, Spec §SC-004]
  > **评估**: SC-004 是性能目标而非硬限制。超过上限时性能逐步下降而非功能拒绝——符合桌面应用预期。SQLite 本身无行数硬限制。

- [x] CHK023 LLM provider 速率限制（429 响应）的重试/退避需求是否在 spec 中定义（而非仅在 contracts/ 中描述 HTTP 状态码含义）？[Gap, Spec §FR-008, Contracts §Error Handling]
  > **评估**: providers crate 已有内置重试/退避逻辑（复用 hiveweb 的 LLM provider 层）。spec 无需重复定义已有库行为。

---

## Notes

- CHK001–CHK023 全部通过评估
- 重点关注 SC 指标的可测量性和异步并发场景的需求覆盖
- Constitution §IV Performance Standards 对 hivegui（桌面应用）的适用性需结合 Deviation 记录理解——桌面应用不适用"API p95 <200ms"等服务器端指标
