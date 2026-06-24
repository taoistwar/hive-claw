//! Perf bench stub: Plugin 列表 + 三维检索（T154 / SC-002）
//!
//! Target: 500 条 Plugin 下 list + filter + FULLTEXT search p95 ≤ 1s
//!
//! TODO: 需要 DB 中预置 500 条 Plugin 数据 + FULLTEXT 索引才能运行完整基准。

use criterion::{Criterion, black_box, criterion_group, criterion_main};

fn bench_plugin_list_stub(c: &mut Criterion) {
    let mut group = c.benchmark_group("plugin_list_stub");
    group.bench_function("stub", |b| b.iter(|| black_box(0)));
    group.finish();
}

criterion_group!(benches, bench_plugin_list_stub);
criterion_main!(benches);
