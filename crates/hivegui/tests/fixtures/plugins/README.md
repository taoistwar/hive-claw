# Shared WASM plugin fixture

本目录保存 HiveWeb 与 HiveGUI 兼容性测试使用的同一份 WASM 黄金制品。两端共享
字节、`hive-extism/v1` manifest 和 host-call 语义，但分别在自己的 host adapter
中执行；HiveGUI 测试不得构造 HiveWeb client 或请求 HiveWeb。

## Canonical files

生成器最终写入以下文件：

```text
shared-smoke/plugin.wasm
shared-smoke/manifest.json
shared-smoke/artifact.json
```

源码唯一来源是仓库根目录的 `plugins/smoke-plugin/`。`manifest.json` 使用 canonical
JSON，内容等价于：

```json
{
  "abi_version": "hive-extism/v1",
  "exports": [
    {"input": "json", "name": "echo", "output": "json"},
    {"input": "json", "name": "fs_roundtrip", "output": "json"},
    {"input": "json", "name": "full_demo", "output": "json"},
    {"input": "json", "name": "http_get", "output": "json"},
    {"input": "json", "name": "ping", "output": "json"}
  ],
  "required_capabilities": [
    "fs.read",
    "fs.write",
    "log.emit",
    "network.http",
    "time.now"
  ]
}
```

`artifact.json` 记录且只记录可复核的制品事实：

```json
{
  "sha256": "64 lowercase hex characters",
  "size_bytes": 1,
  "source": "plugins/smoke-plugin",
  "wasm": "plugin.wasm"
}
```

实际 `size_bytes` 必须大于 0，并等于 `plugin.wasm` 的精确字节数；`sha256` 必须由
这些相同字节计算。README 中的示例值不是可提交 metadata，生成器必须写入真实值。

## Deterministic generation

1. 使用 workspace 精确固定的 Rust 1.97.1、提交的 `Cargo.lock`、
   `wasm32-unknown-unknown` target 和 release profile，从
   `plugins/smoke-plugin/` 执行等价于：

   ```text
   SOURCE_DATE_EPOCH=946684800 CARGO_TARGET_DIR=<private-temp-dir> \
     cargo build --frozen --target wasm32-unknown-unknown \
     --release -p smoke-plugin
   ```

   缺少 target 或依赖时失败并报告，不得静默换工具链、更新 lockfile 或下载不同版本。
2. 只接受该次构建产生的
   `<private-temp-dir>/wasm32-unknown-unknown/release/smoke_plugin.wasm`。生成器以流式
   读取计算 SHA-256 和 size，再由同一数据写入 `artifact.json`。
3. manifest 的对象键、Capability 和 export 按上例排序，以 UTF-8、LF、无多余空白
   的 canonical JSON 写入。不得从主机时间、绝对路径或用户环境生成字段。
4. 在私有临时目录完成 WASM、manifest、SHA、size 和 ABI 校验后，再原子替换
   `shared-smoke/`。失败时保留原黄金目录不变。
5. 不运行未经契约批准的 `wasm-opt`、strip、重签名或二进制补丁步骤。若 toolchain、
   Cargo.lock、源码或构建 profile 变化导致 SHA/size 改变，必须把源码差异、构建环境
   和两端兼容性结果作为同一 review 批次复核。

## Validation rules

fixture validator 必须在任何字节交给 Extism/runtime 之前完成：

1. 规范化相对路径，并确认路径仍位于 fixture/plugin 根目录；
2. 重算文件长度和 SHA-256，与 `artifact.json` 精确匹配；
3. 使用 `wasmparser` 校验 `\0asm` magic、可解析模块、声明的五个 export 存在、
   `host_call` import 符合 ABI，且没有 WASI import；
4. 使用共享 manifest validator 校验 ABI、去重/非空 Capability、export schema，
   并分别用完整、缺失、拒绝和未知 Capability registry 验证稳定错误；
5. 把完全相同的 `plugin.wasm` bytes 和 `manifest.json` 分别传入 HiveWeb 与 HiveGUI
   adapter，比较 success、denied、unknown、timeout、memory、output 和 WASI-denied
   的等价 envelope；不得让一个产品调用另一个产品完成测试；
6. 校验备份 fixture manifest 中引用该制品的 path、SHA-256 和 size 与这里一致。

`plugin.wasm` 是生成物，严禁手工编辑、hex patch 或从聊天/工单附件替换。需要测试
篡改、错误 magic、WASI import、超大输出或资源超限时，由测试生成私有临时变体或由
受审的文本源码生成独立负例；不得修改共享黄金二进制。
