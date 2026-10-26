//! The allocator for threaded WebAssembly builds. wasi-libc's malloc takes
//! one lock that every thread waits on, which made a 16-worker optimizer run
//! over 20 times slower than one worker. Small blocks are recycled through
//! per-thread shards of size-class free lists, each behind a spin lock that
//! another thread rarely holds; everything else goes to the system allocator.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, UnsafeCell};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const CLASSES: [usize; 16] = [
    16, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024, 1536, 2048, 3072, 4096,
];
const ALIGN: usize = 16;
const SHARDS: usize = 32;
/// Bytes each shard keeps per size class before returning blocks.
const KEEP: usize = 128 * 1024;

struct Shard {
    locked: AtomicBool,
    /// Per class: the first free block (each holds the next) and the count.
    lists: UnsafeCell<[(*mut u8, usize); CLASSES.len()]>,
}

// Each shard's lists are only touched while its lock is held.
unsafe impl Sync for Shard {}

static SHARD_LIST: [Shard; SHARDS] = [const {
    Shard {
        locked: AtomicBool::new(false),
        lists: UnsafeCell::new([(std::ptr::null_mut(), 0); CLASSES.len()]),
    }
}; SHARDS];

static NEXT_SHARD: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// This thread's shard plus one; zero until first used. A constant
    /// initializer without a destructor never allocates.
    static SHARD: Cell<usize> = const { Cell::new(0) };
}

fn class(layout: Layout) -> Option<usize> {
    if layout.align() > ALIGN {
        return None;
    }
    CLASSES.iter().position(|&size| layout.size() <= size)
}

fn shard() -> &'static Shard {
    let index = SHARD
        .try_with(|shard| {
            if shard.get() == 0 {
                shard.set(NEXT_SHARD.fetch_add(1, Ordering::Relaxed) % SHARDS + 1);
            }
            shard.get() - 1
        })
        .unwrap_or(0);
    &SHARD_LIST[index]
}

impl Shard {
    fn with<R>(&self, f: impl FnOnce(&mut [(*mut u8, usize); CLASSES.len()]) -> R) -> R {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
        // The lock is held, so no other thread reads or writes these lists.
        let result = f(unsafe { &mut *self.lists.get() });
        self.locked.store(false, Ordering::Release);
        result
    }
}

pub(crate) struct Allocator;

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let Some(class) = class(layout) else {
            return unsafe { System.alloc(layout) };
        };
        let reused = shard().with(|lists| {
            let (head, count) = &mut lists[class];
            if head.is_null() {
                return None;
            }
            let block = *head;
            // A free block's first word holds the next free block.
            *head = unsafe { *(block as *mut *mut u8) };
            *count -= 1;
            Some(block)
        });
        match reused {
            Some(block) => block,
            None => unsafe {
                System.alloc(Layout::from_size_align_unchecked(CLASSES[class], ALIGN))
            },
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let Some(class) = class(layout) else {
            return unsafe { System.dealloc(ptr, layout) };
        };
        let kept = shard().with(|lists| {
            let (head, count) = &mut lists[class];
            if *count >= (KEEP / CLASSES[class]).max(16) {
                return false;
            }
            unsafe { *(ptr as *mut *mut u8) = *head };
            *head = ptr;
            *count += 1;
            true
        });
        if !kept {
            unsafe {
                System.dealloc(
                    ptr,
                    Layout::from_size_align_unchecked(CLASSES[class], ALIGN),
                )
            };
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        match (class(layout), class(new_layout)) {
            // The block already holds its whole class.
            (Some(old), Some(new)) if old == new => ptr,
            (None, None) => unsafe { System.realloc(ptr, layout, new_size) },
            _ => {
                let block = unsafe { self.alloc(new_layout) };
                if !block.is_null() {
                    let copied = layout.size().min(new_size);
                    unsafe { std::ptr::copy_nonoverlapping(ptr, block, copied) };
                    unsafe { self.dealloc(ptr, layout) };
                }
                block
            }
        }
    }
}
