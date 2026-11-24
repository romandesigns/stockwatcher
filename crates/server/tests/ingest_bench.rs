//! The ingest bench (`stockwatcher_server::market::ingest_bench`) under the allocator production
//! runs, mimalloc: a lib unit test cannot set a global allocator, and the two allocators differ
//! exactly where the bench looks (freeing a whole market's state in one clock advance, and frees
//! from another thread, which mimalloc hands back to the owning thread). Manual: ignored, and a
//! no-op without its BENCH_* variables.

#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual ingest bench on a recorded day (BENCH_* variables)"]
async fn ingest_bench_under_mimalloc() {
    stockwatcher_server::market::ingest_bench::run().await;
}
