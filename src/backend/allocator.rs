#![allow(dead_code)]

use std::alloc::{Layout, alloc, dealloc, handle_alloc_error};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ptr::NonNull;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::thread;

const DEFAULT_SIZE_CLASSES: &[usize] = &[
    16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536,
];

#[derive(Clone, Debug)]
pub struct AllocatorConfig {
    pub shard_count: usize,
    pub remote_batch_size: usize,
    pub size_classes: Vec<usize>,
}

impl Default for AllocatorConfig {
    fn default() -> Self {
        Self {
            shard_count: 8,
            remote_batch_size: 32,
            size_classes: DEFAULT_SIZE_CLASSES.to_vec(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ShardStats {
    pub fresh_allocs: u64,
    pub local_reuses: u64,
    pub remote_reuses: u64,
    pub remote_frees: u64,
    pub remote_flushes: u64,
    pub remote_flushed_blocks: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AllocatorStats {
    pub fresh_allocs: u64,
    pub local_reuses: u64,
    pub remote_reuses: u64,
    pub remote_frees: u64,
    pub remote_flushes: u64,
    pub remote_flushed_blocks: u64,
}

#[derive(Debug)]
pub struct Allocation {
    ptr: NonNull<u8>,
    class_index: Option<usize>,
    owner_shard: usize,
    size: usize,
    align: usize,
}

impl Allocation {
    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr.as_ptr()
    }

    pub fn size(&self) -> usize {
        self.size
    }
}

// SAFETY: `Allocation` is an owning handle to raw allocated memory metadata.
// Moving it across threads is safe because no thread-affine invariants are stored;
// ownership is transferred and deallocation remains synchronized by allocator internals.
unsafe impl Send for Allocation {}

#[derive(Default)]
struct ShardCounters {
    fresh_allocs: AtomicU64,
    local_reuses: AtomicU64,
    remote_reuses: AtomicU64,
    remote_frees: AtomicU64,
    remote_flushes: AtomicU64,
    remote_flushed_blocks: AtomicU64,
}

impl ShardCounters {
    fn snapshot(&self) -> ShardStats {
        ShardStats {
            fresh_allocs: self.fresh_allocs.load(Ordering::Relaxed),
            local_reuses: self.local_reuses.load(Ordering::Relaxed),
            remote_reuses: self.remote_reuses.load(Ordering::Relaxed),
            remote_frees: self.remote_frees.load(Ordering::Relaxed),
            remote_flushes: self.remote_flushes.load(Ordering::Relaxed),
            remote_flushed_blocks: self.remote_flushed_blocks.load(Ordering::Relaxed),
        }
    }
}

struct Shard {
    hot_local: Vec<AtomicUsize>,
    local_free: Vec<Mutex<Vec<NonNull<u8>>>>,
    remote_free: Vec<Mutex<Vec<NonNull<u8>>>>,
    counters: ShardCounters,
}

impl Shard {
    fn new(class_count: usize) -> Self {
        let hot_local = (0..class_count)
            .map(|_| AtomicUsize::new(0))
            .collect::<Vec<_>>();
        let local_free = (0..class_count)
            .map(|_| Mutex::new(Vec::new()))
            .collect::<Vec<_>>();
        let remote_free = (0..class_count)
            .map(|_| Mutex::new(Vec::new()))
            .collect::<Vec<_>>();
        Self {
            hot_local,
            local_free,
            remote_free,
            counters: ShardCounters::default(),
        }
    }
}

pub struct ShardedAllocator {
    size_classes: Vec<usize>,
    class_layouts: Vec<Layout>,
    shards: Vec<Shard>,
    remote_batch_size: usize,
}

// SAFETY: all shared mutable state is synchronized via `Mutex` and atomics in shard counters.
unsafe impl Send for ShardedAllocator {}
// SAFETY: internal mutation is synchronized; sharing references across threads is safe.
unsafe impl Sync for ShardedAllocator {}

impl ShardedAllocator {
    pub fn new(config: AllocatorConfig) -> Self {
        let mut size_classes = config.size_classes;
        size_classes.retain(|size| *size > 0);
        size_classes.sort_unstable();
        size_classes.dedup();
        if size_classes.is_empty() {
            size_classes.extend_from_slice(DEFAULT_SIZE_CLASSES);
        }

        let class_layouts = size_classes
            .iter()
            .map(|size| {
                Layout::from_size_align(*size, class_align(*size)).expect("valid size class layout")
            })
            .collect::<Vec<_>>();

        let shard_count = config.shard_count.max(1);
        let shards = (0..shard_count)
            .map(|_| Shard::new(size_classes.len()))
            .collect::<Vec<_>>();

        Self {
            size_classes,
            class_layouts,
            shards,
            remote_batch_size: config.remote_batch_size.max(1),
        }
    }

    pub fn allocate(&self, size: usize) -> Allocation {
        self.allocate_on(self.current_shard(), size)
    }

    pub fn allocate_on(&self, shard_hint: usize, size: usize) -> Allocation {
        let size = size.max(1);
        let owner_shard = shard_hint % self.shards.len();

        if let Some(class_index) = self.size_class_index(size) {
            if let Some(ptr) = self.pop_local(owner_shard, class_index) {
                self.shards[owner_shard]
                    .counters
                    .local_reuses
                    .fetch_add(1, Ordering::Relaxed);
                return Allocation {
                    ptr,
                    class_index: Some(class_index),
                    owner_shard,
                    size: self.size_classes[class_index],
                    align: self.class_layouts[class_index].align(),
                };
            }

            let moved = self.flush_remote_batch_into_local(owner_shard, class_index);
            if moved > 0 {
                if let Some(ptr) = self.pop_local(owner_shard, class_index) {
                    self.shards[owner_shard]
                        .counters
                        .remote_reuses
                        .fetch_add(1, Ordering::Relaxed);
                    return Allocation {
                        ptr,
                        class_index: Some(class_index),
                        owner_shard,
                        size: self.size_classes[class_index],
                        align: self.class_layouts[class_index].align(),
                    };
                }
            }

            let ptr = alloc_raw(self.class_layouts[class_index]);
            self.shards[owner_shard]
                .counters
                .fresh_allocs
                .fetch_add(1, Ordering::Relaxed);
            return Allocation {
                ptr,
                class_index: Some(class_index),
                owner_shard,
                size: self.size_classes[class_index],
                align: self.class_layouts[class_index].align(),
            };
        }

        let align = class_align(size);
        let layout = Layout::from_size_align(size, align).expect("valid large allocation layout");
        let ptr = alloc_raw(layout);
        self.shards[owner_shard]
            .counters
            .fresh_allocs
            .fetch_add(1, Ordering::Relaxed);
        Allocation {
            ptr,
            class_index: None,
            owner_shard,
            size,
            align,
        }
    }

    pub fn deallocate(&self, allocation: Allocation) {
        self.deallocate_on(self.current_shard(), allocation);
    }

    pub fn deallocate_on(&self, shard_hint: usize, allocation: Allocation) {
        let deallocating_shard = shard_hint % self.shards.len();
        let Allocation {
            ptr,
            class_index,
            owner_shard,
            size,
            align,
        } = allocation;

        if let Some(class_index) = class_index {
            if deallocating_shard == owner_shard {
                self.push_local(owner_shard, class_index, ptr);
                return;
            }

            self.shards[owner_shard]
                .counters
                .remote_frees
                .fetch_add(1, Ordering::Relaxed);

            let mut remote = self.shards[owner_shard].remote_free[class_index]
                .lock()
                .expect("remote free list mutex poisoned");
            remote.push(ptr);
            if remote.len() < self.remote_batch_size {
                return;
            }
            let mut drained = Vec::new();
            std::mem::swap(&mut drained, &mut *remote);
            drop(remote);
            self.flush_drained_batch(owner_shard, class_index, drained);
            return;
        }

        let layout = Layout::from_size_align(size, align).expect("valid large deallocation layout");
        // SAFETY: `ptr` was allocated with the same `layout` in `allocate_on`.
        unsafe { dealloc(ptr.as_ptr(), layout) };
    }

    pub fn shard_stats(&self, shard: usize) -> ShardStats {
        self.shards[shard % self.shards.len()].counters.snapshot()
    }

    pub fn stats(&self) -> AllocatorStats {
        let mut out = AllocatorStats::default();
        for shard in &self.shards {
            let snap = shard.counters.snapshot();
            out.fresh_allocs += snap.fresh_allocs;
            out.local_reuses += snap.local_reuses;
            out.remote_reuses += snap.remote_reuses;
            out.remote_frees += snap.remote_frees;
            out.remote_flushes += snap.remote_flushes;
            out.remote_flushed_blocks += snap.remote_flushed_blocks;
        }
        out
    }

    fn size_class_index(&self, size: usize) -> Option<usize> {
        let idx = self
            .size_classes
            .partition_point(|class_size| *class_size < size);
        if idx < self.size_classes.len() {
            Some(idx)
        } else {
            None
        }
    }

    fn current_shard(&self) -> usize {
        let mut hasher = DefaultHasher::new();
        thread::current().id().hash(&mut hasher);
        (hasher.finish() as usize) % self.shards.len()
    }

    fn pop_local(&self, owner_shard: usize, class_index: usize) -> Option<NonNull<u8>> {
        let hot = self.shards[owner_shard].hot_local[class_index].swap(0, Ordering::AcqRel);
        if hot != 0 {
            // SAFETY: hot-local cache stores only non-null pointers previously allocated by this allocator.
            return NonNull::new(hot as *mut u8);
        }
        self.shards[owner_shard].local_free[class_index]
            .lock()
            .expect("local free list mutex poisoned")
            .pop()
    }

    fn push_local(&self, owner_shard: usize, class_index: usize, ptr: NonNull<u8>) {
        let ptr_val = ptr.as_ptr() as usize;
        if self.shards[owner_shard].hot_local[class_index]
            .compare_exchange(0, ptr_val, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return;
        }
        self.shards[owner_shard].local_free[class_index]
            .lock()
            .expect("local free list mutex poisoned")
            .push(ptr);
    }

    fn flush_remote_batch_into_local(&self, owner_shard: usize, class_index: usize) -> usize {
        let mut remote = self.shards[owner_shard].remote_free[class_index]
            .lock()
            .expect("remote free list mutex poisoned");
        if remote.is_empty() {
            return 0;
        }
        let mut drained = Vec::new();
        std::mem::swap(&mut drained, &mut *remote);
        drop(remote);
        let moved = drained.len();
        self.flush_drained_batch(owner_shard, class_index, drained);
        moved
    }

    fn flush_drained_batch(
        &self,
        owner_shard: usize,
        class_index: usize,
        drained: Vec<NonNull<u8>>,
    ) {
        if drained.is_empty() {
            return;
        }
        let moved = drained.len() as u64;
        self.shards[owner_shard].local_free[class_index]
            .lock()
            .expect("local free list mutex poisoned")
            .extend(drained);
        self.shards[owner_shard]
            .counters
            .remote_flushes
            .fetch_add(1, Ordering::Relaxed);
        self.shards[owner_shard]
            .counters
            .remote_flushed_blocks
            .fetch_add(moved, Ordering::Relaxed);
    }
}

impl Drop for ShardedAllocator {
    fn drop(&mut self) {
        for shard in &self.shards {
            for (class_index, layout) in self.class_layouts.iter().enumerate() {
                let hot = shard.hot_local[class_index].swap(0, Ordering::AcqRel);
                if hot != 0 {
                    // SAFETY: pointers in hot-local cache were allocated with this class layout.
                    unsafe { dealloc(hot as *mut u8, *layout) };
                }

                let mut local = shard.local_free[class_index]
                    .lock()
                    .expect("local free list mutex poisoned");
                for ptr in local.drain(..) {
                    // SAFETY: pointers in local free lists were allocated with this class layout.
                    unsafe { dealloc(ptr.as_ptr(), *layout) };
                }
                drop(local);

                let mut remote = shard.remote_free[class_index]
                    .lock()
                    .expect("remote free list mutex poisoned");
                for ptr in remote.drain(..) {
                    // SAFETY: pointers in remote free lists were allocated with this class layout.
                    unsafe { dealloc(ptr.as_ptr(), *layout) };
                }
            }
        }
    }
}

fn class_align(size: usize) -> usize {
    size.checked_next_power_of_two().unwrap_or(64).clamp(8, 64)
}

fn alloc_raw(layout: Layout) -> NonNull<u8> {
    // SAFETY: allocation/deallocation pairing is handled by `deallocate`/`Drop`.
    let ptr = unsafe { alloc(layout) };
    match NonNull::new(ptr) {
        Some(ptr) => ptr,
        None => handle_alloc_error(layout),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn local_free_list_reuses_same_block() {
        let allocator = ShardedAllocator::new(AllocatorConfig {
            shard_count: 2,
            remote_batch_size: 8,
            size_classes: vec![64, 128],
        });

        let first = allocator.allocate_on(0, 33);
        let first_ptr = first.as_ptr() as usize;
        allocator.deallocate_on(0, first);

        let second = allocator.allocate_on(0, 33);
        assert_eq!(first_ptr, second.as_ptr() as usize);
        allocator.deallocate_on(0, second);

        let stats = allocator.shard_stats(0);
        assert!(stats.local_reuses >= 1);
    }

    #[test]
    fn remote_free_batch_flushes_and_reuses() {
        let allocator = ShardedAllocator::new(AllocatorConfig {
            shard_count: 2,
            remote_batch_size: 2,
            size_classes: vec![64],
        });

        let a = allocator.allocate_on(0, 48);
        let b = allocator.allocate_on(0, 48);
        let ptr_a = a.as_ptr() as usize;
        let ptr_b = b.as_ptr() as usize;

        allocator.deallocate_on(1, a);
        assert_eq!(allocator.shard_stats(0).remote_flushes, 0);
        allocator.deallocate_on(1, b);

        let stats = allocator.shard_stats(0);
        assert_eq!(stats.remote_frees, 2);
        assert_eq!(stats.remote_flushes, 1);
        assert_eq!(stats.remote_flushed_blocks, 2);

        let c = allocator.allocate_on(0, 48);
        let d = allocator.allocate_on(0, 48);
        let got = BTreeSet::from([c.as_ptr() as usize, d.as_ptr() as usize]);
        let expected = BTreeSet::from([ptr_a, ptr_b]);
        assert_eq!(got, expected);
        allocator.deallocate_on(0, c);
        allocator.deallocate_on(0, d);
    }

    #[test]
    fn large_allocation_bypasses_size_classes() {
        let allocator = ShardedAllocator::new(AllocatorConfig {
            shard_count: 2,
            remote_batch_size: 8,
            size_classes: vec![64, 128],
        });
        let block = allocator.allocate_on(0, 1 << 20);
        assert_eq!(block.size(), 1 << 20);
        allocator.deallocate_on(1, block);
    }
}
