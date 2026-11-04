//! Criterion benchmarks for the RDBX buffer pool.

use std::path::PathBuf;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use rdbx_buffer::BufferPool;
use rdbx_core::{DatabaseOptions, EvictionPolicyKind, PageId};
use rdbx_storage::PageManager;
use rdbx_storage::PageType;

fn unique_path(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "rdbx-bench-{tag}-{}-{}.rdbx",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    p
}

fn prep(num_pages: usize, capacity: usize) -> (PageManager, BufferPool, PathBuf) {
    let path = unique_path("bp");
    let opts = DatabaseOptions::default();
    let mut mgr = PageManager::create(&path, &opts).unwrap();
    for _ in 0..num_pages {
        mgr.allocate_page(PageType::Data).unwrap();
    }
    let pool = BufferPool::new(capacity, mgr.page_size(), EvictionPolicyKind::Lru);
    (mgr, pool, path)
}

fn bench_fetch_hit(c: &mut Criterion) {
    let mut group = c.benchmark_group("buffer_pool/fetch_hit");
    for capacity in [64usize, 256, 1024] {
        group.bench_with_input(
            BenchmarkId::from_parameter(capacity),
            &capacity,
            |b, &cap| {
                let (mut mgr, mut pool, path) = prep(16, cap);
                let pid = PageId(2);
                // Warm up.
                let fid = pool.fetch(&mut mgr, pid).unwrap();
                pool.unpin(fid).unwrap();
                b.iter(|| {
                    let fid = pool.fetch(&mut mgr, pid).unwrap();
                    pool.unpin(fid).unwrap();
                });
                drop(pool);
                drop(mgr);
                let _ = std::fs::remove_file(&path);
            },
        );
    }
    group.finish();
}

fn bench_fetch_miss(c: &mut Criterion) {
    let mut group = c.benchmark_group("buffer_pool/fetch_miss");
    group.sample_size(20);
    let path = unique_path("miss");
    let opts = DatabaseOptions::default();
    let mut mgr = PageManager::create(&path, &opts).unwrap();
    for _ in 0..256 {
        mgr.allocate_page(PageType::Data).unwrap();
    }
    let mut pool = BufferPool::new(1, mgr.page_size(), EvictionPolicyKind::Lru);
    group.bench_function("rotating_pages", |b| {
        let mut next: u64 = 2;
        b.iter(|| {
            let pid = PageId(next);
            next = 2 + ((next - 2 + 1) % 256);
            let fid = pool.fetch(&mut mgr, pid).unwrap();
            pool.unpin(fid).unwrap();
        });
    });
    drop(pool);
    drop(mgr);
    let _ = std::fs::remove_file(&path);
    group.finish();
}

fn bench_new_page_eviction(c: &mut Criterion) {
    let mut group = c.benchmark_group("buffer_pool/new_page_with_eviction");
    group.sample_size(20);
    group.bench_function("capacity_16", |b| {
        b.iter_batched(
            || prep(64, 16),
            |(mut mgr, mut pool, path)| {
                // Allocate 32 pages through the pool — forces evictions.
                for _ in 0..32 {
                    let fid = pool.new_page(&mut mgr, PageType::Data).unwrap();
                    pool.unpin(fid).unwrap();
                }
                pool.flush_all(&mut mgr).unwrap();
                drop(pool);
                drop(mgr);
                let _ = std::fs::remove_file(&path);
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_fetch_hit,
    bench_fetch_miss,
    bench_new_page_eviction,
);
criterion_main!(benches);
