//! Perf bench stub: Workflow DAG 校验（T155 / SC-003）
//!
//! Target: 50 节点 DAG 的 graph PUT（含 cycle detection + mapping 校验）p95 ≤ 1 秒
//!
//! TODO: 当前 service 层校验逻辑嵌入在 `put_graph` 异步函数中（需要 MySqlPool），
//! 无法直接从 bench 调用纯校验函数。需要重构 service 层将 validate_graph
//! 提取为独立 pub fn 以支持无 DB 依赖的基准测试。

use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_dag_cycle_detection(c: &mut Criterion) {
    let mut group = c.benchmark_group("dag_cycle_detection_stub");
    group.bench_function("stub", |b| {
        b.iter(|| {
            black_box(0)
        })
    });
    group.finish();
}

criterion_group!(benches, bench_dag_cycle_detection);
criterion_main!(benches);
