//! Perf bench stub: Plugin 调用命中池（T141 / SC-005）
//!
//! Target: 命中池 p95 ≤ 50ms，冷启动 ≤ 300ms
//!
//! TODO: 需要真实 Extism plugin WASM 文件 + pool 初始化才能运行完整基准。
//! 当前 plugin_invoke 需要 pool.acquire + extism invoke 链路。

use criterion::{Criterion, black_box, criterion_group, criterion_main};

fn bench_plugin_invoke_stub(c: &mut Criterion) {
    let mut group = c.benchmark_group("plugin_invoke_stub");
    group.bench_function("stub", |b| b.iter(|| black_box(0)));
    group.finish();
}

criterion_group!(benches, bench_plugin_invoke_stub);
criterion_main!(benches);
