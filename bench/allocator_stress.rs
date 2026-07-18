#[path = "../src/backend/allocator.rs"]
mod allocator;

use allocator::{Allocation, AllocatorConfig, ShardedAllocator};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Instant;

#[derive(Clone, Copy)]
struct Config {
    threads: usize,
    shards: usize,
    iters: usize,
    alloc_size: usize,
    batch: usize,
    drain_every: usize,
}

impl Default for Config {
    fn default() -> Self {
        let threads = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .max(2);
        Self {
            threads,
            shards: threads,
            iters: 250_000,
            alloc_size: 64,
            batch: 32,
            drain_every: 64,
        }
    }
}

fn parse_args() -> Result<Config, String> {
    let mut cfg = Config::default();
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1usize;
    while i < args.len() {
        let flag = args[i].as_str();
        let needs_value = matches!(
            flag,
            "--threads" | "--shards" | "--iters" | "--size" | "--batch" | "--drain-every"
        );
        if !needs_value {
            return Err(format!("unknown argument: {}", args[i]));
        }
        if i + 1 >= args.len() {
            return Err(format!("missing value for {}", args[i]));
        }
        let value = args[i + 1]
            .parse::<usize>()
            .map_err(|_| format!("invalid integer for {}: {}", args[i], args[i + 1]))?;
        match flag {
            "--threads" => cfg.threads = value.max(1),
            "--shards" => cfg.shards = value.max(1),
            "--iters" => cfg.iters = value.max(1),
            "--size" => cfg.alloc_size = value.max(1),
            "--batch" => cfg.batch = value.max(1),
            "--drain-every" => cfg.drain_every = value.max(1),
            _ => unreachable!(),
        }
        i += 2;
    }
    Ok(cfg)
}

fn drain_inbox(allocator: &ShardedAllocator, shard: usize, inbox: &Mutex<Vec<Allocation>>) -> u64 {
    let mut drained = Vec::new();
    {
        let mut guard = inbox.lock().expect("inbox mutex poisoned");
        if guard.is_empty() {
            return 0;
        }
        std::mem::swap(&mut drained, &mut *guard);
    }
    let count = drained.len() as u64;
    for allocation in drained {
        allocator.deallocate_on(shard, allocation);
    }
    count
}

fn run(cfg: Config) -> Result<(), String> {
    let allocator = Arc::new(ShardedAllocator::new(AllocatorConfig {
        shard_count: cfg.shards,
        remote_batch_size: cfg.batch,
        size_classes: vec![16, 32, 64, 128, 256, 512, 1024, 2048, 4096],
    }));
    let inboxes: Arc<Vec<Mutex<Vec<Allocation>>>> = Arc::new(
        (0..cfg.threads)
            .map(|_| Mutex::new(Vec::with_capacity(cfg.batch * 2)))
            .collect(),
    );
    let producers_done = Arc::new(Barrier::new(cfg.threads));

    let start = Instant::now();
    let mut handles = Vec::with_capacity(cfg.threads);
    for thread_id in 0..cfg.threads {
        let allocator = Arc::clone(&allocator);
        let inboxes = Arc::clone(&inboxes);
        let producers_done = Arc::clone(&producers_done);
        let handle = thread::spawn(move || -> u64 {
            let shard = thread_id % cfg.shards;
            let next_idx = (thread_id + 1) % cfg.threads;
            let mut outbound = Vec::with_capacity(cfg.batch);
            let mut freed = 0u64;

            for iter in 0..cfg.iters {
                let allocation = allocator.allocate_on(shard, cfg.alloc_size);
                outbound.push(allocation);
                if outbound.len() >= cfg.batch {
                    let mut next = inboxes[next_idx].lock().expect("next inbox mutex poisoned");
                    next.append(&mut outbound);
                }

                if (iter + 1) % cfg.drain_every == 0 {
                    freed += drain_inbox(&allocator, shard, &inboxes[thread_id]);
                }
            }

            if !outbound.is_empty() {
                let mut next = inboxes[next_idx].lock().expect("next inbox mutex poisoned");
                next.append(&mut outbound);
            }

            producers_done.wait();
            freed += drain_inbox(&allocator, shard, &inboxes[thread_id]);
            freed
        });
        handles.push(handle);
    }

    let mut total_freed = 0u64;
    for handle in handles {
        total_freed += handle.join().map_err(|_| "worker thread panicked".to_string())?;
    }
    let elapsed = start.elapsed();

    for inbox in inboxes.iter() {
        let remaining = inbox.lock().expect("inbox mutex poisoned").len();
        if remaining != 0 {
            return Err(format!("allocator stress leaked {remaining} pending inbox entries"));
        }
    }

    let total_allocs = (cfg.threads as u64) * (cfg.iters as u64);
    if total_freed != total_allocs {
        return Err(format!(
            "allocator stress mismatch: allocs={total_allocs} frees={total_freed}"
        ));
    }

    let stats = allocator.stats();
    let elapsed_s = elapsed.as_secs_f64();
    let total_ops = total_allocs + total_freed;
    let ops_per_sec = if elapsed_s > 0.0 {
        (total_ops as f64) / elapsed_s
    } else {
        0.0
    };
    let avg_remote_batch = if stats.remote_flushes > 0 {
        stats.remote_flushed_blocks as f64 / stats.remote_flushes as f64
    } else {
        0.0
    };
    let remote_free_ratio = if total_freed > 0 {
        stats.remote_frees as f64 / total_freed as f64
    } else {
        0.0
    };

    println!(
        "threads={} shards={} batch={} size={} iters={} drain_every={} allocs={} frees={} elapsed_ms={:.3} ops_per_sec={:.3} fresh_allocs={} local_reuses={} remote_reuses={} remote_frees={} remote_flushes={} remote_flushed_blocks={} avg_remote_batch={:.3} remote_free_ratio={:.6}",
        cfg.threads,
        cfg.shards,
        cfg.batch,
        cfg.alloc_size,
        cfg.iters,
        cfg.drain_every,
        total_allocs,
        total_freed,
        elapsed_s * 1_000.0,
        ops_per_sec,
        stats.fresh_allocs,
        stats.local_reuses,
        stats.remote_reuses,
        stats.remote_frees,
        stats.remote_flushes,
        stats.remote_flushed_blocks,
        avg_remote_batch,
        remote_free_ratio
    );
    Ok(())
}

fn main() {
    if let Err(err) = parse_args().and_then(run) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
