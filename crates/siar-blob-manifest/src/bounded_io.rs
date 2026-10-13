//! §37 "Streaming I/O" and §40 "Backpressure": the sender-side
//! pipeline.
//!
//! ```text
//! disk reader → bounded chunk queue → consumer (encrypt / send)
//! ```
//!
//! "If network slows: reader slows. Memory remains bounded." and
//! "Never read entire 2 GB file into `Vec<u8>`."
//!
//! [`stream_chunks`] reads a file (or any `Read`) one chunk at a time
//! into buffers borrowed from a [`BufferPool`] and hands them to a
//! consumer through a **bounded** queue. Two independent bounds hold at
//! once:
//!
//! - the pool has `max_buffers` buffers in total, so no more than that
//!   many chunks exist anywhere (being read, queued, or being consumed);
//! - the queue holds at most `queue_depth` finished chunks, so the reader
//!   blocks on `send` the moment the consumer falls behind.
//!
//! So peak memory is `max_buffers × buffer_size` — a constant. The
//! pool's blocking `acquire` is what slows the reader when the pool is
//! empty; the queue bound is what slows it when the consumer is slow
//! *and* buffers happen to be plentiful. Either one alone would be
//! enough to bound memory; both together keep latency (chunks sitting
//! in the queue) bounded as well.
//!
//! The reader runs on its own thread; the consumer runs on the calling
//! thread. If the consumer returns an error the reader is stopped (the
//! channel closes) rather than left reading a 2 GB file to nowhere.

use std::io::Read;
use std::sync::mpsc::sync_channel;

use crate::buffer_pool::{BufferPool, PooledBuffer};
use crate::store_error::BlobError;

/// What a streaming run did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamSummary {
    pub chunks: u64,
    pub bytes: u64,
}

/// Reads `reader` to EOF in `pool.buffer_size()`-sized chunks, calling
/// `consume(index, chunk)` for each in order. See the module doc for the
/// memory bound.
///
/// `consume` receiving an error stops the pipeline and returns it.
pub fn stream_chunks<R, F>(
    reader: R,
    pool: &BufferPool,
    queue_depth: usize,
    mut consume: F,
) -> Result<StreamSummary, BlobError>
where
    R: Read + Send + 'static,
    F: FnMut(u64, &[u8]) -> Result<(), BlobError>,
{
    let (tx, rx) = sync_channel::<Result<PooledBuffer, std::io::ErrorKind>>(queue_depth.max(1));
    let pool = pool.clone();

    let producer = std::thread::spawn(move || {
        let mut reader = reader;
        loop {
            let mut buf = pool.acquire(); // blocks when the pool is empty
            match buf.fill_from(&mut reader) {
                Ok(0) => return,
                Ok(_) => {
                    if tx.send(Ok(buf)).is_err() {
                        return; // consumer gone: stop reading
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e.kind()));
                    return;
                }
            }
        }
    });

    let mut summary = StreamSummary {
        chunks: 0,
        bytes: 0,
    };
    let mut result = Ok(());
    for item in rx.iter() {
        match item {
            Ok(buf) => {
                if let Err(e) = consume(summary.chunks, &buf) {
                    result = Err(e);
                    break;
                }
                summary.chunks += 1;
                summary.bytes += buf.len() as u64;
            }
            Err(kind) => {
                result = Err(BlobError::Io {
                    kind,
                    op: "read source",
                });
                break;
            }
        }
    }
    drop(rx); // unblocks a producer stuck on `send`
    let _ = producer.join();
    result.map(|()| summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn every_byte_arrives_once_in_order_in_bounded_chunks() {
        let data: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let pool = BufferPool::new(3, 1024);
        let mut seen = Vec::new();
        let mut indices = Vec::new();
        let s = stream_chunks(Cursor::new(data.clone()), &pool, 2, |i, chunk| {
            assert!(chunk.len() <= 1024);
            indices.push(i);
            seen.extend_from_slice(chunk);
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, data);
        assert_eq!(s.bytes, 10_000);
        assert_eq!(s.chunks, 10, "ceil(10000/1024)");
        assert_eq!(indices, (0..10).collect::<Vec<_>>());
        assert_eq!(pool.available(), 3, "every buffer came home");
    }

    #[test]
    fn a_slow_consumer_slows_the_reader_and_memory_stays_bounded() {
        // A "file" that records how far it has been read.
        struct Probe {
            remaining: usize,
            read_so_far: Arc<AtomicUsize>,
        }
        impl Read for Probe {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                let n = out.len().min(self.remaining);
                out[..n].fill(1);
                self.remaining -= n;
                self.read_so_far.fetch_add(n, Ordering::SeqCst);
                Ok(n)
            }
        }
        const CHUNK: usize = 1000;
        let read_so_far = Arc::new(AtomicUsize::new(0));
        let pool = BufferPool::new(3, CHUNK); // total memory: 3 KB
        let probe = Probe {
            remaining: 1_000 * CHUNK,
            read_so_far: Arc::clone(&read_so_far),
        }; // "1 MB" source
        let max_ahead = AtomicUsize::new(0);
        let consumed = AtomicUsize::new(0);

        stream_chunks(probe, &pool, 1, |_, chunk| {
            // The consumer is slow, so the reader is as far ahead as the
            // bounds allow. Measure how far ahead of consumption it got.
            std::thread::sleep(Duration::from_millis(1));
            let c = consumed.fetch_add(chunk.len(), Ordering::SeqCst) + chunk.len();
            let ahead = read_so_far.load(Ordering::SeqCst) - c;
            max_ahead.fetch_max(ahead, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();

        // At most `max_buffers` chunks can exist beyond the one being
        // consumed: the reader is never more than ~pool-size ahead,
        // regardless of the 1 MB file.
        let bound = 3 * CHUNK;
        assert!(
            max_ahead.load(Ordering::SeqCst) <= bound,
            "reader ran {} bytes ahead; bound is {}",
            max_ahead.load(Ordering::SeqCst),
            bound
        );
    }

    #[test]
    fn a_consumer_error_stops_the_reader_instead_of_reading_to_the_end() {
        struct Endless(Arc<AtomicUsize>);
        impl Read for Endless {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                self.0.fetch_add(out.len(), Ordering::SeqCst);
                out.fill(0);
                Ok(out.len())
            }
        }
        let counter = Arc::new(AtomicUsize::new(0));
        let pool = BufferPool::new(2, 100);
        let r = stream_chunks(Endless(Arc::clone(&counter)), &pool, 1, |i, _| {
            if i == 2 {
                Err(BlobError::InvalidArgument("stop"))
            } else {
                Ok(())
            }
        });
        assert_eq!(r, Err(BlobError::InvalidArgument("stop")));
        // The source is endless; reaching here at all proves the
        // reader was stopped. It also read only a handful of chunks.
        assert!(counter.load(Ordering::SeqCst) < 100 * 20);
        assert_eq!(pool.available(), 2);
    }

    #[test]
    fn a_source_read_error_is_reported_with_its_kind() {
        struct Bad;
        impl Read for Bad {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            }
        }
        let pool = BufferPool::new(1, 10);
        let r = stream_chunks(Bad, &pool, 1, |_, _| Ok(()));
        assert_eq!(
            r,
            Err(BlobError::Io {
                kind: std::io::ErrorKind::PermissionDenied,
                op: "read source"
            })
        );
    }

    #[test]
    fn an_empty_source_yields_nothing() {
        let pool = BufferPool::new(1, 10);
        let s = stream_chunks(
            Cursor::new(Vec::<u8>::new()),
            &pool,
            1,
            |_, _| unreachable!(),
        )
        .unwrap();
        assert_eq!(
            s,
            StreamSummary {
                chunks: 0,
                bytes: 0
            }
        );
    }
}
