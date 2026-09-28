//! A global allocator that gives every thread a Windows heap of its own for
//! blocks of `SHARDED_FROM` bytes and more.
//!
//! Rust's `System` allocator serves every thread from the one process heap.
//! Its low-fragmentation front end takes small blocks without the heap's lock,
//! but every larger block -- file contents, row text, the output of a deflate
//! -- is allocated and freed under that single lock. ERP УХ's base-free stage
//! on 48 workers spent 60 % of its sampled worker time waiting in
//! `RtlEnterCriticalSection` under `RtlFreeHeap`/`RtlAllocateHeap`.
//!
//! Here small blocks still go to `System`. A larger block comes from the heap
//! of the thread that allocates it (one of `SHARDS` serialized private heaps,
//! assigned round robin), with a 16-byte header naming that heap, so any
//! thread can free or grow it; contention on a heap is then only between its
//! own thread and the threads freeing its blocks. Only the binary installs it
//! (`#[global_allocator]` in `main.rs`); the library and its tests keep
//! `System`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

/// Blocks this large or larger come from the thread's own heap.
const SHARDED_FROM: usize = 1024;
/// Header ahead of every sharded block: the heap handle and the block's base.
const HEADER: usize = 16;
/// What `HeapAlloc` guarantees on 64-bit Windows.
const HEAP_ALIGN: usize = 16;
const SHARDS: usize = 64;
const HEAP_ZERO_MEMORY: u32 = 0x0000_0008;

static HEAPS: [AtomicIsize; SHARDS] = [const { AtomicIsize::new(0) }; SHARDS];
static NEXT_SHARD: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static SHARD: Cell<usize> = const { Cell::new(usize::MAX) };
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn HeapCreate(options: u32, initial: usize, maximum: usize) -> isize;
    fn HeapDestroy(heap: isize) -> i32;
    fn HeapAlloc(heap: isize, flags: u32, bytes: usize) -> *mut u8;
    fn HeapReAlloc(heap: isize, flags: u32, block: *mut u8, bytes: usize) -> *mut u8;
    fn HeapFree(heap: isize, flags: u32, block: *mut u8) -> i32;
}

pub struct ShardedHeap;

impl ShardedHeap {
    fn sharded(layout: &Layout) -> bool {
        layout.size() >= SHARDED_FROM
    }

    /// The calling thread's heap, created on first use; 0 when none can be.
    fn heap() -> isize {
        let shard = SHARD
            .try_with(|shard| {
                if shard.get() == usize::MAX {
                    shard.set(NEXT_SHARD.fetch_add(1, Ordering::Relaxed) % SHARDS);
                }
                shard.get()
            })
            .unwrap_or(0);
        let slot = &HEAPS[shard];
        let heap = slot.load(Ordering::Acquire);
        if heap != 0 {
            return heap;
        }
        // SAFETY: plain Win32 call; a serialized, growable heap.
        let created = unsafe { HeapCreate(0, 0, 0) };
        if created == 0 {
            return 0;
        }
        match slot.compare_exchange(0, created, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => created,
            Err(existing) => {
                // SAFETY: the heap was just created here and never handed out.
                unsafe { HeapDestroy(created) };
                existing
            }
        }
    }

    /// Allocates `layout` from the thread's heap, the header ahead of it.
    unsafe fn alloc_sharded(layout: Layout, flags: u32) -> *mut u8 {
        let heap = Self::heap();
        if heap == 0 {
            return std::ptr::null_mut();
        }
        let align = layout.align().max(HEAP_ALIGN);
        let Some(total) = layout
            .size()
            .checked_add(HEADER)
            .and_then(|size| size.checked_add(align - HEAP_ALIGN))
        else {
            return std::ptr::null_mut();
        };
        // SAFETY: a live heap handle and a non-zero size.
        let base = unsafe { HeapAlloc(heap, flags, total) };
        if base.is_null() {
            return base;
        }
        let user_address = (base as usize + HEADER).next_multiple_of(align);
        let user = user_address as *mut u8;
        // SAFETY: the header lies inside the block, at least HEADER bytes
        // after its base, and is 8-aligned.
        unsafe {
            let header = user.sub(HEADER) as *mut usize;
            header.write(heap as usize);
            header.add(1).write(base as usize);
        }
        user
    }

    /// The heap and base of a sharded block.
    unsafe fn header(user: *mut u8) -> (isize, *mut u8) {
        // SAFETY: written by `alloc_sharded` right before `user`.
        unsafe {
            let header = user.sub(HEADER) as *const usize;
            (header.read() as isize, header.add(1).read() as *mut u8)
        }
    }
}

unsafe impl GlobalAlloc for ShardedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if Self::sharded(&layout) {
            // SAFETY: forwarded contract.
            unsafe { Self::alloc_sharded(layout, 0) }
        } else {
            // SAFETY: forwarded contract.
            unsafe { System.alloc(layout) }
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if Self::sharded(&layout) {
            // SAFETY: forwarded contract.
            unsafe { Self::alloc_sharded(layout, HEAP_ZERO_MEMORY) }
        } else {
            // SAFETY: forwarded contract.
            unsafe { System.alloc_zeroed(layout) }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if Self::sharded(&layout) {
            // SAFETY: `ptr` came from `alloc_sharded` for this very layout.
            unsafe {
                let (heap, base) = Self::header(ptr);
                HeapFree(heap, 0, base);
            }
        } else {
            // SAFETY: forwarded contract.
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let Ok(new_layout) = Layout::from_size_align(new_size, layout.align()) else {
            return std::ptr::null_mut();
        };
        match (Self::sharded(&layout), Self::sharded(&new_layout)) {
            // SAFETY: forwarded contract.
            (false, false) => unsafe { System.realloc(ptr, layout, new_size) },
            (true, true) if layout.align() <= HEAP_ALIGN => {
                // SAFETY: `ptr` came from `alloc_sharded` with the header
                // right at the block's base (alignment 16); `HeapReAlloc` keeps
                // the header's bytes, and the base is written anew.
                unsafe {
                    let (heap, base) = Self::header(ptr);
                    let Some(total) = new_size.checked_add(HEADER) else {
                        return std::ptr::null_mut();
                    };
                    let moved = HeapReAlloc(heap, 0, base, total);
                    if moved.is_null() {
                        return moved;
                    }
                    let user = moved.add(HEADER);
                    (user.sub(HEADER) as *mut usize)
                        .add(1)
                        .write(moved as usize);
                    user
                }
            }
            _ => {
                // SAFETY: a fresh block of the new layout, the common prefix
                // copied, then the old block freed under its own layout.
                unsafe {
                    let fresh = self.alloc(new_layout);
                    if !fresh.is_null() {
                        std::ptr::copy_nonoverlapping(ptr, fresh, layout.size().min(new_size));
                        self.dealloc(ptr, layout);
                    }
                    fresh
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sharded_blocks_keep_their_bytes_through_growth_and_threads() {
        let heap = ShardedHeap;
        unsafe {
            for align in [1usize, 8, 16, 32, 4096] {
                let layout = Layout::from_size_align(4000, align).unwrap();
                let block = heap.alloc(layout);
                assert!(!block.is_null());
                assert_eq!(block as usize % align, 0);
                for index in 0..4000 {
                    block.add(index).write((index % 251) as u8);
                }
                let grown = heap.realloc(block, layout, 100_000);
                assert!(!grown.is_null());
                assert_eq!(grown as usize % align, 0);
                for index in 0..4000 {
                    assert_eq!(grown.add(index).read(), (index % 251) as u8);
                }
                let grown_layout = Layout::from_size_align(100_000, align).unwrap();
                let shrunk = heap.realloc(grown, grown_layout, 100);
                for index in 0..100 {
                    assert_eq!(shrunk.add(index).read(), (index % 251) as u8);
                }
                heap.dealloc(shrunk, Layout::from_size_align(100, align).unwrap());
            }
            let zeroed = heap.alloc_zeroed(Layout::from_size_align(8192, 16).unwrap());
            assert!((0..8192).all(|index| zeroed.add(index).read() == 0));
            heap.dealloc(zeroed, Layout::from_size_align(8192, 16).unwrap());
        }
        // A block allocated on one thread and freed on another.
        let address = std::thread::spawn(|| unsafe {
            ShardedHeap.alloc(Layout::from_size_align(50_000, 16).unwrap()) as usize
        })
        .join()
        .unwrap();
        unsafe {
            ShardedHeap.dealloc(
                address as *mut u8,
                Layout::from_size_align(50_000, 16).unwrap(),
            );
        }
    }
}
