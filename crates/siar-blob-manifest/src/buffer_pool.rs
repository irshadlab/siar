//! §39 "Buffer Pool" and the allocation half of §38 "Low-Copy Data
//! Path" / §40 "Backpressure".
//!
//! ```text
//! BufferPool
//! ├── max buffers
//! ├── chunk size classes
//! └── backpressure
//! ```
//!
//! "Do not create unbounded pools." [`BufferPool`] holds a *fixed*
//! number of buffers of a *fixed* size, allocated once. Handing one out
//! never allocates; getting one back is just dropping the
//! [`PooledBuffer`]. When all are out, [`BufferPool::try_acquire`]
//! returns `None` and [`BufferPool::acquire`] **blocks** — and that
//! blocking *is* the backpressure of §40: a disk reader that needs a
//! buffer to read into cannot run ahead of a slow consumer, so total
//! memory is `max_buffers × buffer_size`, fixed, whatever the file size
//! or network speed.
//!
//! ## Size classes
//!
//! A pool is one size. [`BufferPools`] bundles one pool per
//! [`crate::chunking::ChunkSizeClass`] so a small file does not pin
//! 4 MiB buffers and a large one does not thrash tiny ones; each class
//! has its own independent bound.
//!
//! ## Why a `Condvar` and not `async`
//!
//! The wait is for *another thread to finish with a buffer*; a blocking
//! primitive is the honest model for a pool shared by reader and
//! consumer threads and it keeps this crate free of an async-runtime
//! dependency. An async caller wraps [`BufferPool::acquire`] in its
//! runtime's blocking-call facility, or uses `try_acquire` and yields.
//!
//! The pool never gives out the *same* buffer twice (an `Arc`'d free
//! list guarded by a mutex) and a returned buffer is cleared of its
//! length but its capacity is retained; contents are not zeroed on the
//! way out (they are overwritten before being read — see
//! [`PooledBuffer::fill_from`]) — but are **never exposed beyond
//! `len()`**, so stale bytes from a previous chunk cannot leak.

use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Condvar, Mutex};

use crate::chunking::ChunkSizeClass;

#[derive(Debug)]
struct Inner {
    free: Mutex<Vec<Vec<u8>>>,
    available: Condvar,
    buffer_size: usize,
    max_buffers: usize,
}

#[derive(Debug, Clone)]
pub struct BufferPool {
    inner: Arc<Inner>,
}

impl BufferPool {
    /// Allocates all `max_buffers` buffers of `buffer_size` up front.
    ///
    /// # Panics
    /// If either argument is zero — a pool that can hold nothing is a
    /// programming error, not a runtime condition.
    pub fn new(max_buffers: usize, buffer_size: usize) -> Self {
        assert!(max_buffers > 0 && buffer_size > 0, "pool must be non-empty");
        let free = (0..max_buffers)
            .map(|_| Vec::with_capacity(buffer_size))
            .collect();
        Self {
            inner: Arc::new(Inner {
                free: Mutex::new(free),
                available: Condvar::new(),
                buffer_size,
                max_buffers,
            }),
        }
    }

    pub fn buffer_size(&self) -> usize {
        self.inner.buffer_size
    }

    pub fn max_buffers(&self) -> usize {
        self.inner.max_buffers
    }

    /// Buffers currently idle in the pool.
    pub fn available(&self) -> usize {
        self.inner.free.lock().expect("pool poisoned").len()
    }

    /// Non-blocking. `None` means the pool is exhausted *right now*.
    pub fn try_acquire(&self) -> Option<PooledBuffer> {
        let buf = self.inner.free.lock().expect("pool poisoned").pop()?;
        Some(PooledBuffer {
            buf: Some(buf),
            pool: Arc::clone(&self.inner),
        })
    }

    /// Blocks until a buffer is free — the §40 backpressure point.
    pub fn acquire(&self) -> PooledBuffer {
        let mut free = self.inner.free.lock().expect("pool poisoned");
        loop {
            if let Some(buf) = free.pop() {
                return PooledBuffer {
                    buf: Some(buf),
                    pool: Arc::clone(&self.inner),
                };
            }
            free = self.inner.available.wait(free).expect("pool poisoned");
        }
    }
}

/// A buffer on loan. Derefs to the *initialized* bytes only
/// (`0..len`); dropping it returns it to the pool.
#[derive(Debug)]
pub struct PooledBuffer {
    buf: Option<Vec<u8>>,
    pool: Arc<Inner>,
}

impl PooledBuffer {
    /// Capacity limit of this buffer (the pool's `buffer_size`).
    pub fn capacity_limit(&self) -> usize {
        self.pool.buffer_size
    }

    /// Replaces the contents by reading **up to** `capacity_limit()`
    /// bytes from `reader` (a short read at EOF is normal). Returns the
    /// number of bytes now held. Never grows past the pool's buffer
    /// size, so a hostile or huge source cannot force a larger
    /// allocation.
    pub fn fill_from(&mut self, reader: &mut impl std::io::Read) -> std::io::Result<usize> {
        let limit = self.pool.buffer_size;
        let buf = self.buf.as_mut().expect("buffer present until drop");
        buf.clear();
        buf.resize(limit, 0);
        let mut filled = 0;
        while filled < limit {
            match reader.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    buf.clear();
                    return Err(e);
                }
            }
        }
        buf.truncate(filled);
        Ok(filled)
    }
}

impl Deref for PooledBuffer {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.buf.as_deref().expect("buffer present until drop")
    }
}

impl DerefMut for PooledBuffer {
    fn deref_mut(&mut self) -> &mut [u8] {
        self.buf.as_deref_mut().expect("buffer present until drop")
    }
}

impl Drop for PooledBuffer {
    fn drop(&mut self) {
        if let Some(mut buf) = self.buf.take() {
            buf.clear(); // length 0: nothing stale is readable
            self.pool.free.lock().expect("pool poisoned").push(buf);
            self.pool.available.notify_one();
        }
    }
}

/// One bounded pool per §13 size class.
#[derive(Debug, Clone)]
pub struct BufferPools {
    small: BufferPool,
    medium: BufferPool,
    large: BufferPool,
}

impl BufferPools {
    /// `buffers_per_class` buffers in each class; the class sizes are
    /// [`ChunkSizeClass::default_chunk_size`]'s medium/large values and
    /// a 64 KiB small buffer (small files are one chunk of at most the
    /// small threshold; they are read in 64 KiB pieces).
    pub fn new(buffers_per_class: usize) -> Self {
        Self {
            small: BufferPool::new(buffers_per_class, 64 * 1024),
            medium: BufferPool::new(buffers_per_class, 512 * 1024),
            large: BufferPool::new(buffers_per_class, 4 * 1024 * 1024),
        }
    }

    pub fn for_class(&self, class: ChunkSizeClass) -> &BufferPool {
        match class {
            ChunkSizeClass::Small => &self.small,
            ChunkSizeClass::Medium => &self.medium,
            ChunkSizeClass::Large => &self.large,
        }
    }

    /// Total bytes this bundle can ever hold — the fixed memory bound.
    pub fn total_capacity_bytes(&self) -> usize {
        [&self.small, &self.medium, &self.large]
            .iter()
            .map(|p| p.max_buffers() * p.buffer_size())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn the_pool_is_bounded_and_buffers_return_on_drop() {
        let pool = BufferPool::new(2, 16);
        assert_eq!(pool.available(), 2);
        let a = pool.try_acquire().unwrap();
        let b = pool.try_acquire().unwrap();
        assert!(
            pool.try_acquire().is_none(),
            "exhausted: no third buffer is ever created"
        );
        assert_eq!(pool.available(), 0);
        drop(a);
        assert_eq!(pool.available(), 1);
        assert!(pool.try_acquire().is_some());
        drop(b);
    }

    #[test]
    fn fill_never_exceeds_the_buffer_size_and_short_reads_are_exact() {
        let pool = BufferPool::new(1, 8);
        let mut buf = pool.acquire();
        let n = buf.fill_from(&mut Cursor::new(vec![7u8; 100])).unwrap();
        assert_eq!(
            (n, buf.len()),
            (8, 8),
            "an oversize source is cut at the limit"
        );
        let n = buf.fill_from(&mut Cursor::new(vec![9u8; 3])).unwrap();
        assert_eq!(
            (n, &buf[..]),
            (3, &[9u8, 9, 9][..]),
            "short read: exactly 3 bytes visible"
        );
        let n = buf.fill_from(&mut Cursor::new(Vec::<u8>::new())).unwrap();
        assert_eq!((n, buf.len()), (0, 0));
    }

    #[test]
    fn stale_bytes_from_a_previous_chunk_are_never_visible() {
        let pool = BufferPool::new(1, 8);
        {
            let mut b = pool.acquire();
            b.fill_from(&mut Cursor::new(vec![0xAA; 8])).unwrap();
        }
        let mut b = pool.acquire();
        assert_eq!(b.len(), 0, "a recycled buffer starts empty");
        b.fill_from(&mut Cursor::new(vec![0xBB; 2])).unwrap();
        assert_eq!(&b[..], &[0xBB, 0xBB], "no 0xAA tail leaks past len");
    }

    #[test]
    fn read_errors_clear_the_buffer_and_propagate() {
        struct Failing;
        impl std::io::Read for Failing {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("boom"))
            }
        }
        let pool = BufferPool::new(1, 8);
        let mut b = pool.acquire();
        assert!(b.fill_from(&mut Failing).is_err());
        assert_eq!(b.len(), 0);
    }

    #[test]
    fn acquire_blocks_until_a_buffer_is_returned_that_is_the_backpressure() {
        let pool = BufferPool::new(1, 8);
        let held = pool.acquire();
        let got = Arc::new(AtomicUsize::new(0));
        let (p2, g2) = (pool.clone(), Arc::clone(&got));
        let t = std::thread::spawn(move || {
            let _b = p2.acquire(); // blocks
            g2.store(1, Ordering::SeqCst);
        });
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(
            got.load(Ordering::SeqCst),
            0,
            "still waiting while the pool is empty"
        );
        drop(held);
        t.join().unwrap();
        assert_eq!(got.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn size_classes_have_independent_fixed_memory_bounds() {
        let pools = BufferPools::new(4);
        assert_eq!(
            pools.total_capacity_bytes(),
            4 * (64 * 1024 + 512 * 1024 + 4 * 1024 * 1024)
        );
        let _held: Vec<_> = (0..4)
            .map(|_| pools.for_class(ChunkSizeClass::Large).acquire())
            .collect();
        assert!(pools
            .for_class(ChunkSizeClass::Large)
            .try_acquire()
            .is_none());
        assert!(
            pools
                .for_class(ChunkSizeClass::Small)
                .try_acquire()
                .is_some(),
            "other classes unaffected"
        );
    }
}
