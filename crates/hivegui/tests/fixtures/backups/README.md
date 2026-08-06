# Encrypted backup fixtures

本目录保存 format 1、2、3 的认证加密备份黄金夹具。包结构和恢复语义以
`specs/011-hivegui-standalone-mode/contracts/backup-package.md` 为准。所有恢复测试
都在隔离 staging 中运行，绝不把夹具恢复到开发者的真实 HiveGUI 数据目录。

## Fixture matrix

| 文件 | 逻辑内容 | 预期结果 |
| --- | --- | --- |
| `format1-valid.age` | format 1 / schema v2；Function kind `1/2/3`；四个旧点号 Builtin；四个稳定 `*_node` | 在 staging 中升级到 format 3/schema v4；kind 和 Builtin 精确规范化，节点值不变 |
| `format1-builtin-collision.age` | 在 format 1 合法内容中同时放入四个点号 Builtin 与其下划线目标 | 返回冲突、销毁 staging；当前数据库和 Plugin 目录零修改 |
| `format2-valid.age` | format 2 / schema v3；三种稳定 Function kind、四个下划线 Builtin、四个 `*_node` | 在 staging 中升级到 format 3/schema v4，所有已有字段和关系不变 |
| `format3-valid.age` | format 3 / schema v4；完整实体清单、三种 Function kind、四个下划线 Builtin、四个 `*_node` 和共享 WASM | 直接恢复并用目标设备密钥重新加密敏感字段 |
| `format3-invalid-legacy-values.age` | format 3 中故意放入整数 kind、短 node type 或点号 Builtin | 预检失败并销毁 staging；不得把这些值当作别名或当前格式兼容值 |

format 1 的固定 Function 数据必须覆盖 `1 -> builtin`、`2 -> custom`、
`3 -> placeholder`。Placeholder 的 Plugin、export 和 Capability 字段为空。四个节点
固定为 `start_node`、`function_node`、`generate_answer_node`、`end_node`；当前
格式和旧格式均不使用 `start`、`function`、`generate_answer`、`end` 短名称。

旧点号 Builtin 只允许由 format 1/2 受信升级器处理。升级后唯一名称是
`format_template`、`json_parse`、`json_stringify`、`text_regex_match`，不保留
`format.template`、`json.parse`、`json.stringify`、`text.regex_match` 别名。

## Deterministic logical package generation

1. 每个格式必须由对应的版本化 serializer 从固定场景模型生成；禁止生成 format 3
   后只篡改 manifest 的 `format_version` 来冒充旧格式。
2. 固定 ID、UUID、记录顺序和 `exported_at=2000-01-01T00:00:00Z`。JSON 使用
   UTF-8、LF、排序对象键、稳定复数实体键和无多余空白的 canonical 表示；实体行
   按稳定 ID 排序。
3. tar 条目按 UTF-8 path 字节序排列；header 固定 `mtime=0`、`uid=0`、`gid=0`、
   空 user/group name，普通文件使用固定 mode。不得包含绝对路径、`..`、重复条目、
   symlink、hardlink、设备密钥、日志或诊断包。
4. manifest 中每个 entity file 的 path、count 和 SHA-256 都从最终 canonical bytes
   计算。Plugin 条目的 path、`size_bytes` 和 SHA-256 必须逐项匹配
   `../plugins/README.md` 的共享 WASM；禁止复写或手工修补二进制。
5. 外层必须调用生产使用的 `age` passphrase writer 和安全随机源。随机 salt/nonce
   意味着同一逻辑包的密文不要求逐字节相同，也不得为了可重复性固定密码学随机数。
   可重复性的判据是解密后的 canonical tar 摘要、manifest 和语义摘要相同。
6. 测试口令由 fixture support 中明确标为“非机密、仅测试”的常量注入；不得使用
   开发者凭据、环境中的真实 token 或设备密钥。生成过程只写私有临时目录，完整
   加密流关闭、fsync 和复核成功后才原子替换黄金文件。
7. 无效与碰撞包也由场景生成器产生。不得用 archive editor、hex editor 或手工
   修改密文来制造黄金负例；临时篡改测试应在测试私有副本或内存中完成。

生成入口由 `backup_restore` 测试提供为显式 ignored regeneration case；普通测试
只能读取黄金包。生成器输出逻辑 tar SHA-256 与语义摘要，review 时比较这些值，
而不是比较每次随机化的 age ciphertext SHA-256。

## Validation rules

生成后和日常测试都必须执行以下校验：

1. 使用测试口令流式解密，读取到认证流结尾；错误口令、截断和任意位置篡改必须在
   修改现有状态前失败；
2. 拒绝不安全/重复 archive path 和 link，逐条重算 entity SHA、artifact SHA 与
   size，并验证 manifest 没有缺失或多余条目；
3. format 3 必须列出契约要求的全部实体文件，即使 count 为 0；不得包含本地设备
   密钥、结构化日志、诊断包或未加密敏感临时文件；
4. format 1 断言整数 kind 集合恰为 `{1,2,3}`、四个 `*_node` 各出现一次、四个
   点号 Builtin 各出现一次；升级后断言三种字符串 kind 与四个下划线名称精确出现，
   点号 lookup/execute 失败；
5. format 2/3 断言 Function kind 仅为 `builtin|custom|placeholder`、node type 仅为
   四个 `*_node`、Builtin 仅为四个下划线名称；
6. 碰撞和非法当前格式用例必须断言 staging 被销毁，目标数据库、Plugin 目录及其
   SHA/size 摘要不变，不发生记录合并或别名注册；
7. 成功恢复必须在不同设备密钥下逐实体、逐关系、逐制品核对，并证明敏感明文等价、
   目标密文已重加密且源设备密钥未进入包内。

黄金 `.age` 是生成物，不得手工编辑。格式变化必须先更新版本化 serializer、升级器
和契约测试，再完整再生并复核解密后的逻辑摘要。
