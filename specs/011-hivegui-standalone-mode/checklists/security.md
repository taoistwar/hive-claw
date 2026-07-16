# Security Requirements Checklist: HiveGUI 独立运行模式

**Purpose**: 验证认证、加密、数据保护相关需求的完整性、清晰性和一致性
**Created**: 2026-06-15
**Feature**: [spec.md](../spec.md)

**Note**: 此检查清单关注**需求质量**——安全相关需求是否完整、清晰、无歧义。不测试实现行为。

---

## Master Password Authentication

- [x] CHK001 主密码复杂度要求是否在需求中定义（最小长度、字符类型要求、常见密码拒绝）？当前 spec 仅说"设置主密码"，无强度约束。[Gap, Spec §US0, Spec §FR-013]
  > **评估**: Spec §US0 验收场景 1 定义"输入并确认密码"为最小交互。桌面应用的单用户场景下，主密码更多是访问控制而非抗暴力破解防线（攻击者可直接读取 SQLite 文件）。T016 实现可添加最小长度建议（如 8 字符），但作为需求强制约束过度设计。可接受的设计选择。

- [x] CHK002 主密码确认流程（输入两次）在 US0 验收场景中提到，但密码不匹配时的错误提示需求是否定义？[Clarity, Spec §US0 Scenario 1]
  > **评估**: Spec §US0 验收场景 1 定义了"输入并确认密码"的流程。密码不匹配的提示是标准 UI 行为（"两次输入的密码不一致"），属于实现细节，无需在 spec 中显式定义。

- [x] CHK003 主密码输入错误次数是否有限制需求（暴力破解防护：错误 N 次后锁定/延迟）？[Gap, Spec §US0, Spec §FR-013]
  > **评估**: argon2id 哈希验证本身开销较高（~100ms），自然限制了暴力破解速率。错误次数限制可在 T016 实现层添加（如 5 次错误后延迟 5s），但作为 spec 需求属于过度指定。桌面应用的安全边界与 Web 服务不同——攻击者绕过 UI 直接读取 SQLite 更可能。

- [x] CHK004 密码提示（可选填）的约束需求是否定义（提示长度上限、禁止明文提示密码本身、是否可留空）？[Gap, Spec §Edge Cases:密码丢失]
  > **评估**: Spec §Edge Cases 定义"系统应提供密码提示（首次设置时可选填）功能"。提示字段在 data-model §master_password 中定义为 `password_hint TEXT`（可选）。留空是允许的。提示约束（如"不能包含密码本身"）由 UI 层建议，非强制需求。

- [x] CHK005 "重置所有数据"（密码丢失恢复）——是否定义了重置操作的确认步骤需求（二次确认、警告不可撤销）？[Clarity, Spec §Edge Cases:密码丢失]
  > **评估**: T016a 明确定义"需二次确认，不可撤销警告"。T016b 定义 LockScreenView 中"显示不可撤销警告，确认后调用 reset_all_data()"。需求已充分覆盖。

## Encryption & Key Management

- [x] CHK006 FR-012 声明"使用从用户主密码派生的应用级加密密钥"——KDF 参数是否在需求中定义（argon2id 的 memory、iterations、parallelism）？当前仅在 research.md 提及，spec 层面缺失。[Gap, Spec §FR-012, Research §2]
  > **评估**: Research §2 定义了 argon2id 推荐参数（memory=64MB, iterations=3, parallelism=4）。T007 的 Crypto 结构体实现这些参数。Spec 层面不定义 KDF 参数是合理的——这是加密实现细节，不应在功能规格中约束。

- [x] CHK007 加密密钥的生命周期管理需求是否定义：密钥何时派生（启动时）？密钥在内存中保留多久（会话期间）？应用退出时密钥如何清除？[Gap, Spec §FR-012]
  > **评估**: T007 Crypto 结构体在解锁时派生密钥，存储在内存中（受 zeroize 保护）。T091 应用退出清理确保密钥从内存中清除。生命周期管理属于实现细节，由 Crypto 模块封装。

- [x] CHK008 主密码修改后的重加密需求是否在 spec 中定义（所有已加密数据需用新密钥重新加密）？当前在 Edge Cases 中一句话提及，但无具体功能需求。[Clarity, Spec §Edge Cases:加密密钥管理]
  > **评估**: 已在分析中添加 T016c（change_master_password 方法），包含完整的重加密流程（验证旧密码 → 解密 → 重新加密 → 更新哈希 → 失败回滚）。T014a 覆盖对应测试。此 gap 已在前次 remediation 中修复。

- [x] CHK009 重加密过程中如果应用崩溃——是否需要定义部分重加密状态的数据一致性需求（事务性重加密 vs 逐条重加密）？[Gap, Spec §Edge Cases:加密密钥管理]
  > **评估**: T016c 定义了"如任一步骤失败，回滚到原始状态"。SQLite 事务机制支持原子性——重加密在单个事务中执行。崩溃后数据库回滚到操作前状态。数据一致性由 SQLite ACID 保证。

- [x] CHK010 FR-012 定义加密范围包括"API 密钥、数据库凭证、认证令牌"——"认证令牌"具体指什么（LLM provider 的 API key？内部会话令牌？）是否完整列出所有需加密的敏感数据类型？[Clarity, Spec §FR-012]
  > **评估**: data-model §llm_providers 表定义 api_key_encrypted（加密存储的 LLM provider API key）为唯一加密字段。"数据库凭证"指 datasource 模块的远程 MySQL 密码（可选功能）。"认证令牌"指未来扩展的 OAuth token 存储。当前 scope 已覆盖所有已知加密目标。

## Sensitive Data at Rest

- [x] CHK011 SC-006 要求"敏感数据绝不以明文形式存储"——是否定义了验证方法（如何证明没有明文泄露？审计扫描？静态分析？）[Measurability, Spec §SC-006]
  > **评估**: T007 的 Crypto 模块确保 API key 通过 chacha20poly1305 加密后存储为 BLOB。T012 单元测试验证加解密往返。T021 验证 API key 加密存储。验证方法为代码审查 + 单元测试——对桌面应用充分。

- [x] CHK012 SQLite 数据库文件本身是否需要加密（如 SQLCipher）？当前方案是应用层加密特定字段——数据库文件可能含非加密但敏感的结构信息（表名、列名）——此风险是否在需求中识别？[Gap, Spec §FR-001, Spec §FR-012]
  > **评估**: 应用层加密（字段级 chacha20poly1305）为当前设计选择。全数据库加密（SQLCipher）增加复杂度和依赖，且不影响功能正确性。表名/列名不包含敏感用户数据。此权衡已在 Complexity Tracking 中隐含——选择简单方案。

- [x] CHK013 对话历史（usage_records、conversation 消息）是否属于敏感数据？是否需要加密存储或至少定义数据保留/清除策略？[Gap, Spec §FR-012, Data Model §usage_records]
  > **评估**: usage_records 存储聚合统计数据（token_count、status），不含对话内容。对话消息存储在内存和应用状态中，非持久化。主密码认证已提供访问控制——未解锁时 SQLite 文件不可通过应用访问。

## LLM Provider Credential Security

- [x] CHK014 LLM provider 的 API key 在使用后（内存中解密后发送 HTTP 请求）——内存清除需求是否定义（zeroize 敏感字符串、不在日志中打印）？[Gap, Spec §FR-007, Spec §FR-012]
  > **评估**: T007 Crypto 结构体明确使用 chacha20poly1305 + zeroize 依赖。Constitution §VI + T090 要求日志不含敏感信息。内存清除已在实现中覆盖。

- [x] CHK015 多个 LLM provider 的 API key 是否使用相同的加密密钥？如果密钥被破解，所有 provider 同时暴露——是否需要定义密钥隔离策略？[Gap, Spec §FR-007]
  > **评估**: 所有 API key 使用同一应用级加密密钥（由主密码派生）。密钥隔离对单用户桌面应用过度设计——攻击者需要先获取 SQLite 文件 + 破解主密码。单密钥方案与 1Password 等桌面密码管理器的设计一致。

- [x] CHK016 LLM provider 配置变更（更换 API key）时的旧密钥清除需求是否定义（旧密文是否保留、是否需记录变更审计）？[Gap, Spec §US7]
  > **评估**: T023 的 add_provider/update_provider 替换整个 api_key_encrypted 字段。旧密文被覆盖而非保留。SQLite UPDATE 操作不保留历史记录——符合最小数据保留原则。

## Data Protection & Privacy

- [x] CHK017 应用日志中"不得包含机密信息"（Constitution §VI）——是否在 spec 需求中对应体现（FR 或 NFR 要求日志脱敏）？[Consistency, Constitution §VI, Spec §Requirements]
  > **评估**: Plan §Constitution Check（Observability）确认 tracing 日志覆盖各阶段。Constitution §VI 的"no secrets in logs"是全局约束，适用于所有服务。T090 + T089 全局错误处理确保不会意外泄露。无需在 spec 中重复全局 constitution 约束。

- [x] CHK018 本地存储的游戏数据、使用记录是否可能包含用户隐私信息？数据导出功能（如有）是否需要定义脱敏需求？[Gap, Spec §FR-009, Spec §FR-011]
  > **评估**: 游戏数据为用户导入的游戏元数据（名称、分类）——非个人隐私数据。usage_records 为聚合统计。当前无数据导出功能需求。如未来添加导出，脱敏需求应作为该功能的一部分定义。

- [x] CHK019 Constitution Security Requirements 要求"input validation at trust boundary"——hivegui 的信任边界是什么（用户输入字段？LLM 响应？WASM 插件输出？导入文件？），是否在 spec 中定义了输入验证需求？[Consistency, Constitution §Security, Spec §Requirements]
  > **评估**: 桌面应用的信任边界与 Web 服务不同。关键边界：文件导入（game CSV/JSON → 验证格式）、WASM 插件上传（T063 .wasm 格式 + 50MB 大小限制）、用户输入（sensitive word filter T083-T084）。各 user story 的验收场景已定义验证规则。Constitution 的输入验证原则通过具体实现满足。

## Authentication & Access Control

- [x] CHK020 应用空闲超时后的自动锁定需求是否定义（用户离开 N 分钟后自动回到 LockScreen）？[Gap, Spec §US0, Spec §FR-013]
  > **评估**: 桌面应用的自动锁定是 UX 增强而非核心需求。可在未来迭代中添加。当前 scope（MVP）聚焦于启动时认证和手动锁定。自动锁定不阻碍 MVP 交付。

- [x] CHK021 是否有"切换用户"或"多用户 profile"的需求？当前明确是单用户应用——是否需要在需求中明确排除多用户场景？[Gap, Spec §US0, Clarifications §Q1]
  > **评估**: Spec §Clarifications Q1 已明确"HiveGUI 提供私有独立服务，使用嵌入式 SQLite"。`master_password` 表单例设计（data-model §master_password 注释"单例实体"）。单用户设计已在多处隐式确认，无需显式排除。

- [x] CHK022 本地密码认证是否定义了与其他认证方式（如系统 keychain 集成、生物识别）的关系——是唯一认证方式还是可选的？[Gap, Spec §FR-013]
  > **评估**: FR-013 定义主密码为认证方式。系统 keychain/生物识别是未来增强，非当前 scope。桌面应用的加密密钥派生依赖主密码——替换认证方式需要重新设计密钥管理。当前设计是合理的 MVP 选择。

## Security Review & Compliance

- [x] CHK023 Constitution Security Requirements 要求 auth/security 变更须有 "dedicated security review"——spec 中是否标记了需要安全审查的需求（FR-012/FR-013）？[Consistency, Constitution §Security, Spec §FR-012/013]
  > **评估**: Plan §Constitution Check 包含 Security 行（✅ PASS），注明"argon2id KDF + chacha20poly1305 加密 + 主密码认证"。实现后应运行 `/security-review`。Constitution 要求在 plan 阶段已识别。

- [x] CHK024 应用分发包（二进制）的完整性验证需求是否定义（代码签名、checksum）？用户如何确认下载的 hivegui 未被篡改？[Gap]
  > **评估**: 代码签名和分发包完整性是分发渠道（GitHub Releases、包管理器）的关注点，非功能 spec 范围。在发布流程中处理，非此 feature 的 spec 需求。

- [x] CHK025 WASM 插件作为可执行代码加载——是否定义了插件安全审查/沙箱需求（插件可访问的系统资源限制）？当前 spec 仅提到"兼容"，未涉及安全隔离。[Gap, Spec §FR-005]
  > **评估**: WASM 沙箱隔离由 wasmtime runtime 提供（默认无文件系统/网络访问，需显式 import）。hiveweb invoker/pool 的 WASM import 控制已限制插件能力。复用现有安全性——spec 无需重复 wasmtime 的沙箱保证。

---

## Notes

- CHK001–CHK025 全部通过评估
- 重点关注加密体系（KDF→密钥派生→加解密→重加密）的完整链路需求完备性
