//! Opcode queue for buffering filesystem operations.
//!
//! This module provides a thread-safe queue for opcodes with:
//! - `VecDeque` storage (inspectable, persistence-ready)
//! - `Condvar` notification (wake-on-push for processor)
//! - Bounded capacity with backpressure
//!
//! # Example
//!
//! ```
//! use ize_lib::operations::{OpcodeQueue, Operation, Opcode};
//! use std::path::PathBuf;
//!
//! let queue = OpcodeQueue::new();
//! let sender = queue.sender();
//!
//! // Push an opcode
//! let op = Operation::FileCreate {
//!     path: PathBuf::from("test.txt"),
//!     mode: 0o644,
//!     content: vec![],
//! };
//! sender.send(Opcode::new(1, op));
//!
//! // Pop from queue
//! let opcode = queue.try_pop().unwrap();
//! ```

use log::debug;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use super::Opcode;

/// Default queue capacity
const DEFAULT_CAPACITY: usize = 10_000;

/// Thread-safe opcode queue with notification.
///
/// Uses a `VecDeque` for storage and a `Condvar` for wake-on-push.
/// This design allows inspection of queue contents and future persistence.
pub struct OpcodeQueue {
    /// The actual queue storage
    inner: Mutex<QueueInner>,
    /// Condition variable for waking the processor
    not_empty: Condvar,
}

struct QueueInner {
    queue: VecDeque<Opcode>,
    capacity: usize,
}

/// Handle for pushing opcodes to the queue.
///
/// Can be cloned and shared across threads. Each clone holds
/// an `Arc` reference to the underlying queue.
#[derive(Clone)]
pub struct OpcodeSender {
    queue: Arc<OpcodeQueue>,
}

impl OpcodeQueue {
    /// Create a new queue with default capacity (10,000).
    pub fn new() -> Arc<Self> {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// Create a new queue with specified capacity.
    ///
    /// The capacity is a soft limit - `try_push` will fail when at capacity,
    /// but `push` will always succeed (allowing temporary overflow).
    pub fn with_capacity(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(QueueInner {
                // Pre-allocate a reasonable amount, not the full capacity
                queue: VecDeque::with_capacity(capacity.min(1000)),
                capacity,
            }),
            not_empty: Condvar::new(),
        })
    }

    /// Create a sender handle for this queue.
    ///
    /// The sender can be cloned and shared with multiple producers.
    pub fn sender(self: &Arc<Self>) -> OpcodeSender {
        OpcodeSender {
            queue: Arc::clone(self),
        }
    }

    /// Push an opcode onto the queue (non-blocking).
    ///
    /// Returns `Err(opcode)` if queue is at capacity, allowing the caller
    /// to decide whether to drop, retry, or force push.
    pub fn try_push(&self, opcode: Opcode) -> Result<(), Opcode> {
        let mut inner = self.lock();
        if inner.queue.len() >= inner.capacity {
            debug!("OpcodeQueue::try_push: queue at capacity, rejecting opcode");
            return Err(opcode);
        }
        debug!(
            "OpcodeQueue::try_push: pushing opcode seq={}, queue_len={}",
            opcode.seq(),
            inner.queue.len() + 1
        );
        inner.queue.push_back(opcode);
        drop(inner); // Release lock before notify
        self.not_empty.notify_one();
        Ok(())
    }

    /// Push an opcode onto the queue (always succeeds).
    ///
    /// If the queue is at capacity, the opcode is still pushed,
    /// allowing temporary overflow. Use `try_push` for strict
    /// capacity enforcement.
    pub fn push(&self, opcode: Opcode) {
        let mut inner = self.lock();
        debug!(
            "OpcodeQueue::push: pushing opcode seq={}, queue_len={}",
            opcode.seq(),
            inner.queue.len() + 1
        );
        inner.queue.push_back(opcode);
        drop(inner); // Release lock before notify
        self.not_empty.notify_one();
    }

    /// Pop an opcode from the queue (non-blocking).
    ///
    /// Returns `None` if the queue is empty.
    pub fn try_pop(&self) -> Option<Opcode> {
        let mut inner = self.lock();
        let result = inner.queue.pop_front();
        if let Some(ref opcode) = result {
            debug!(
                "OpcodeQueue::try_pop: popped opcode seq={}, remaining={}",
                opcode.seq(),
                inner.queue.len()
            );
        }
        result
    }

    /// Pop an opcode from the queue (blocking).
    ///
    /// Blocks until an opcode is available.
    pub fn pop(&self) -> Opcode {
        let mut inner = self.lock();
        while inner.queue.is_empty() {
            inner = self.not_empty.wait(inner).unwrap();
        }
        inner.queue.pop_front().unwrap()
    }

    /// Drain all available opcodes (non-blocking).
    ///
    /// Returns all opcodes currently in the queue, leaving it empty.
    /// Useful for batch processing.
    pub fn drain(&self) -> Vec<Opcode> {
        let mut inner = self.lock();
        inner.queue.drain(..).collect()
    }

    /// Check if queue is empty.
    pub fn is_empty(&self) -> bool {
        self.lock().queue.is_empty()
    }

    /// Get current queue length.
    pub fn len(&self) -> usize {
        self.lock().queue.len()
    }

    /// Get queue capacity.
    pub fn capacity(&self) -> usize {
        self.lock().capacity
    }

    /// Peek at all queue contents (for debugging/inspection).
    ///
    /// Returns a clone of all opcodes without removing them.
    pub fn peek_all(&self) -> Vec<Opcode> {
        let inner = self.lock();
        inner.queue.iter().cloned().collect()
    }

    /// Helper to lock the inner mutex.
    fn lock(&self) -> MutexGuard<'_, QueueInner> {
        self.inner.lock().unwrap()
    }
}

impl Default for OpcodeQueue {
    fn default() -> Self {
        Self {
            inner: Mutex::new(QueueInner {
                queue: VecDeque::with_capacity(1000),
                capacity: DEFAULT_CAPACITY,
            }),
            not_empty: Condvar::new(),
        }
    }
}

impl OpcodeSender {
    /// Push an opcode onto the queue (non-blocking).
    ///
    /// Returns `Err(opcode)` if queue is at capacity.
    pub fn try_send(&self, opcode: Opcode) -> Result<(), Opcode> {
        self.queue.try_push(opcode)
    }

    /// Push an opcode onto the queue (always succeeds).
    pub fn send(&self, opcode: Opcode) {
        self.queue.push(opcode)
    }

    /// Check if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Get current queue length.
    pub fn len(&self) -> usize {
        self.queue.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::Operation;
    use std::path::PathBuf;
    use std::thread;
    use std::time::Duration;

    fn make_test_opcode(seq: u64, name: &str) -> Opcode {
        Opcode::new(
            seq,
            Operation::FileCreate {
                path: PathBuf::from(name),
                mode: 0o644,
                content: vec![],
            },
        )
    }

    #[test]
    fn test_queue_creation() {
        let queue = OpcodeQueue::new();
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
        assert_eq!(queue.capacity(), DEFAULT_CAPACITY);
    }

    #[test]
    fn test_queue_with_capacity() {
        let queue = OpcodeQueue::with_capacity(100);
        assert_eq!(queue.capacity(), 100);
    }

    #[test]
    fn test_push_and_pop() {
        let queue = OpcodeQueue::new();

        queue.push(make_test_opcode(1, "a.txt"));
        queue.push(make_test_opcode(2, "b.txt"));

        assert_eq!(queue.len(), 2);

        let op1 = queue.try_pop().unwrap();
        assert_eq!(op1.seq(), 1);

        let op2 = queue.try_pop().unwrap();
        assert_eq!(op2.seq(), 2);

        assert!(queue.try_pop().is_none());
        assert!(queue.is_empty());
    }

    #[test]
    fn test_try_push_at_capacity() {
        let queue = OpcodeQueue::with_capacity(2);

        assert!(queue.try_push(make_test_opcode(1, "a.txt")).is_ok());
        assert!(queue.try_push(make_test_opcode(2, "b.txt")).is_ok());

        // At capacity - should fail
        let result = queue.try_push(make_test_opcode(3, "c.txt"));
        assert!(result.is_err());

        // Force push should still work
        queue.push(make_test_opcode(4, "d.txt"));
        assert_eq!(queue.len(), 3); // Over capacity
    }

    #[test]
    fn test_drain() {
        let queue = OpcodeQueue::new();

        queue.push(make_test_opcode(1, "a.txt"));
        queue.push(make_test_opcode(2, "b.txt"));
        queue.push(make_test_opcode(3, "c.txt"));

        let drained = queue.drain();
        assert_eq!(drained.len(), 3);
        assert!(queue.is_empty());

        assert_eq!(drained[0].seq(), 1);
        assert_eq!(drained[1].seq(), 2);
        assert_eq!(drained[2].seq(), 3);
    }

    #[test]
    fn test_peek_all() {
        let queue = OpcodeQueue::new();

        queue.push(make_test_opcode(1, "a.txt"));
        queue.push(make_test_opcode(2, "b.txt"));

        let peeked = queue.peek_all();
        assert_eq!(peeked.len(), 2);

        // Queue should still have items
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn test_sender() {
        let queue = OpcodeQueue::new();
        let sender = queue.sender();

        sender.send(make_test_opcode(1, "a.txt"));
        assert_eq!(sender.len(), 1);
        assert!(!sender.is_empty());

        let op = queue.try_pop().unwrap();
        assert_eq!(op.seq(), 1);
    }

    #[test]
    fn test_sender_clone() {
        let queue = OpcodeQueue::new();
        let sender1 = queue.sender();
        let sender2 = sender1.clone();

        sender1.send(make_test_opcode(1, "a.txt"));
        sender2.send(make_test_opcode(2, "b.txt"));

        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn test_blocking_pop() {
        let queue = OpcodeQueue::new();
        let sender = queue.sender();

        // Spawn thread that will push after delay
        let sender_clone = sender.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            sender_clone.send(make_test_opcode(42, "delayed.txt"));
        });

        // This should block until the item is pushed
        let op = queue.pop();
        assert_eq!(op.seq(), 42);
    }

    #[test]
    fn test_fifo_ordering() {
        let queue = OpcodeQueue::new();

        for i in 0..100 {
            queue.push(make_test_opcode(i, &format!("file_{}.txt", i)));
        }

        for i in 0..100 {
            let op = queue.try_pop().unwrap();
            assert_eq!(op.seq(), i);
        }
    }

    #[test]
    fn test_concurrent_push_pop() {
        let queue = OpcodeQueue::new();
        let sender = queue.sender();

        // Producer thread
        let producer = {
            let sender = sender.clone();
            thread::spawn(move || {
                for i in 0..100 {
                    sender.send(make_test_opcode(i, &format!("file_{}.txt", i)));
                }
            })
        };

        // Consumer collects all
        let mut received = Vec::new();

        // Wait for producer to finish
        producer.join().unwrap();

        // Drain all
        received.extend(queue.drain());

        assert_eq!(received.len(), 100);
    }
}
