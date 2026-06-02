//! Perf bench stub: host_call dispatch（T140 / SC-004）
//!
//! Target: host_call 鉴权 + 转发开销 p95 ≤ 5ms
//!
//! TODO: 当前 dispatch 是 async fn 且需要 DispatcherDeps（含 pool, registry, handlers）。
//! 需要构造 mock deps 以在无 DB 环境下进行基准测试。

use criterion::{Criterion, black_box, criterion_group, criterion_main};

fn bench_host_call_dispatch_stub(c: &mut Criterion) {
    let mut group = c.benchmark_group("host_call_dispatch_stub");
    group.bench_function("stub", |b| b.iter(|| black_box(0)));
    group.finish();
}

criterion_group!(benches, bench_host_call_dispatch_stub);
criterion_main!(benches);
