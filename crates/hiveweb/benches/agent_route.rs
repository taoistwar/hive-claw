//! Perf bench stub: Agent 路由决策（T156 / SC-006）
//!
//! Target: 含 1 次 LLM 决策调用的路由 p95 ≤ 1.5s（mock LLM 800ms）
//!
//! TODO: 需要 mock LLM provider + agent tree 初始化才能运行完整基准。

use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_agent_route_stub(c: &mut Criterion) {
    let mut group = c.benchmark_group("agent_route_stub");
    group.bench_function("stub", |b| b.iter(|| black_box(0)));
    group.finish();
}

criterion_group!(benches, bench_agent_route_stub);
criterion_main!(benches);
