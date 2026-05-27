//! Perf bench stub: Plugin 上传（T153 / SC-001）
//!
//! Target: 1MB Plugin 上传（入库 + S3 PUT）p95 ≤ 5s
//!
//! TODO: 需要真实 S3 连接 + WASM 文件才能运行完整基准。

use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_plugin_upload_stub(c: &mut Criterion) {
    let mut group = c.benchmark_group("plugin_upload_stub");
    group.bench_function("stub", |b| b.iter(|| black_box(0)));
    group.finish();
}

criterion_group!(benches, bench_plugin_upload_stub);
criterion_main!(benches);
