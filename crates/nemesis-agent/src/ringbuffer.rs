//! Generic ring buffer: a thread-safe, fixed-capacity circular buffer.
//!
//! When the buffer is full, new `Push` calls overwrite the oldest entry.
//! Items are returned in insertion order (oldest first).

use std::sync::RwLock;

/// A thread-safe, generic circular buffer.
///
/// When the buffer is full, new `push` calls overwrite the oldest entry.
/// Items are returned in insertion order (oldest first).
pub struct RingBuffer<T> {
    buf: RwLock<Vec<Option<T>>>,
    size: usize,
    head: RwLock<usize>,  // index of next write position
    count: RwLock<usize>, // number of items currently in the buffer
}

impl<T> RingBuffer<T> {
    /// Create a new RingBuffer with the given fixed capacity.
    /// Capacity must be positive; a capacity of 0 is clamped to 1.
    pub fn new(size: usize) -> Self {
        let size = if size == 0 { 1 } else { size };
        let buf: Vec<Option<T>> = (0..size).map(|_| None).collect();
        Self {
            buf: RwLock::new(buf),
            size,
            head: RwLock::new(0),
            count: RwLock::new(0),
        }
    }

    /// Add an item to the ring buffer. If the buffer is full,
    /// the oldest entry is overwritten.
    pub fn push(&self, item: T) {
        let mut buf = self.buf.write().unwrap();
        let mut head = self.head.write().unwrap();
        let mut count = self.count.write().unwrap();

        buf[*head] = Some(item);
        *head = (*head + 1) % self.size;
        if *count < self.size {
            *count += 1;
        }
    }

    /// Returns all items in insertion order (oldest first).
    /// The returned slice is a copy; modifications do not affect the buffer.
    pub fn get_all(&self) -> Vec<T>
    where
        T: Clone,
    {
        let buf = self.buf.read().unwrap();
        let head = self.head.read().unwrap();
        let count = self.count.read().unwrap();

        if *count == 0 {
            return Vec::new();
        }

        let mut result = Vec::with_capacity(*count);
        for i in 0..*count {
            // Calculate the index of the i-th oldest item.
            // When the buffer is full, the oldest item is at head.
            // Otherwise, it starts at index 0.
            let idx = (self.size + *head - *count + i) % self.size;
            if let Some(ref val) = buf[idx] {
                result.push(val.clone());
            }
        }
        result
    }

    /// Returns the last n items in insertion order (oldest first).
    /// If n is greater than the buffer length, all items are returned.
    /// If n is 0, returns an empty vector.
    pub fn get_last(&self, n: usize) -> Vec<T>
    where
        T: Clone,
    {
        if n == 0 {
            return Vec::new();
        }

        let buf = self.buf.read().unwrap();
        let head = self.head.read().unwrap();
        let count = self.count.read().unwrap();

        if *count == 0 {
            return Vec::new();
        }

        // Clamp n to available count
        let n = n.min(*count);
        let start_idx = *count - n;

        let mut result = Vec::with_capacity(n);
        for i in 0..n {
            let idx = (self.size + *head - *count + start_idx + i) % self.size;
            if let Some(ref val) = buf[idx] {
                result.push(val.clone());
            }
        }
        result
    }

    /// Returns the number of items currently in the buffer.
    pub fn len(&self) -> usize {
        *self.count.read().unwrap()
    }

    /// Returns true if the buffer contains no items.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Removes all items from the buffer.
    pub fn clear(&self) {
        let mut buf = self.buf.write().unwrap();
        let mut head = self.head.write().unwrap();
        let mut count = self.count.write().unwrap();

        // Drop all entries
        for slot in buf.iter_mut() {
            *slot = None;
        }
        *head = 0;
        *count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_buffer_has_zero_length() {
        let rb: RingBuffer<i32> = RingBuffer::new(5);
        assert_eq!(rb.len(), 0);
        assert!(rb.is_empty());
    }

    #[test]
    fn zero_capacity_clamped_to_one() {
        let rb: RingBuffer<i32> = RingBuffer::new(0);
        rb.push(1);
        rb.push(2);
        // Should only keep the last item
        assert_eq!(rb.len(), 1);
        assert_eq!(rb.get_all(), vec![2]);
    }

    #[test]
    fn push_and_get_all() {
        let rb = RingBuffer::new(5);
        rb.push(10);
        rb.push(20);
        rb.push(30);

        assert_eq!(rb.len(), 3);
        assert_eq!(rb.get_all(), vec![10, 20, 30]);
    }

    #[test]
    fn overwrite_oldest_when_full() {
        let rb = RingBuffer::new(3);
        rb.push(1);
        rb.push(2);
        rb.push(3);
        rb.push(4); // overwrites 1
        rb.push(5); // overwrites 2

        assert_eq!(rb.len(), 3);
        assert_eq!(rb.get_all(), vec![3, 4, 5]);
    }

    #[test]
    fn get_last_returns_recent_items() {
        let rb = RingBuffer::new(5);
        rb.push(1);
        rb.push(2);
        rb.push(3);
        rb.push(4);
        rb.push(5);

        assert_eq!(rb.get_last(3), vec![3, 4, 5]);
        assert_eq!(rb.get_last(10), vec![1, 2, 3, 4, 5]); // more than count
        assert_eq!(rb.get_last(0), Vec::<i32>::new());
    }

    #[test]
    fn get_last_on_empty_buffer() {
        let rb: RingBuffer<i32> = RingBuffer::new(5);
        assert_eq!(rb.get_last(3), Vec::<i32>::new());
    }

    #[test]
    fn clear_empties_buffer() {
        let rb = RingBuffer::new(5);
        rb.push(1);
        rb.push(2);
        rb.push(3);
        assert_eq!(rb.len(), 3);

        rb.clear();
        assert_eq!(rb.len(), 0);
        assert!(rb.is_empty());
        assert_eq!(rb.get_all(), Vec::<i32>::new());
    }

    #[test]
    fn clear_and_push_again() {
        let rb = RingBuffer::new(3);
        rb.push(1);
        rb.push(2);
        rb.clear();
        rb.push(10);
        rb.push(20);

        assert_eq!(rb.len(), 2);
        assert_eq!(rb.get_all(), vec![10, 20]);
    }

    #[test]
    fn string_items() {
        let rb = RingBuffer::new(3);
        rb.push("hello".to_string());
        rb.push("world".to_string());
        rb.push("foo".to_string());
        rb.push("bar".to_string()); // overwrites "hello"

        assert_eq!(rb.get_all(), vec!["world", "foo", "bar"]);
        assert_eq!(rb.get_last(2), vec!["foo", "bar"]);
    }

    #[test]
    fn single_capacity_buffer() {
        let rb = RingBuffer::new(1);
        rb.push(1);
        rb.push(2);
        rb.push(3);

        assert_eq!(rb.len(), 1);
        assert_eq!(rb.get_all(), vec![3]);
    }

    #[test]
    fn get_all_returns_empty_for_empty_buffer() {
        let rb: RingBuffer<i32> = RingBuffer::new(5);
        assert_eq!(rb.get_all(), Vec::<i32>::new());
    }

    #[test]
    fn test_large_buffer() {
        let rb = RingBuffer::new(1000);
        for i in 0..1000 {
            rb.push(i);
        }
        assert_eq!(rb.len(), 1000);
        let all = rb.get_all();
        assert_eq!(all[0], 0);
        assert_eq!(all[999], 999);
    }

    #[test]
    fn test_large_buffer_overflow() {
        let rb = RingBuffer::new(100);
        for i in 0..200 {
            rb.push(i);
        }
        assert_eq!(rb.len(), 100);
        let all = rb.get_all();
        assert_eq!(all[0], 100);
        assert_eq!(all[99], 199);
    }

    #[test]
    fn test_get_last_n_equals_count() {
        let rb = RingBuffer::new(5);
        rb.push(1);
        rb.push(2);
        rb.push(3);

        assert_eq!(rb.get_last(3), vec![1, 2, 3]);
    }

    #[test]
    fn test_get_last_after_overflow() {
        let rb = RingBuffer::new(3);
        rb.push(1);
        rb.push(2);
        rb.push(3);
        rb.push(4);

        assert_eq!(rb.get_last(2), vec![3, 4]);
        assert_eq!(rb.get_last(3), vec![2, 3, 4]);
    }

    #[test]
    fn test_clear_then_push_cycle() {
        let rb = RingBuffer::new(3);
        for cycle in 0..3 {
            rb.push(cycle * 10);
            rb.push(cycle * 10 + 1);
            assert_eq!(rb.len(), 2);
            assert_eq!(rb.get_all(), vec![cycle * 10, cycle * 10 + 1]);
            rb.clear();
            assert!(rb.is_empty());
        }
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let rb = Arc::new(RingBuffer::new(100));
        let mut handles = vec![];

        for i in 0..10 {
            let rb_clone = rb.clone();
            handles.push(thread::spawn(move || {
                for j in 0..100 {
                    rb_clone.push(i * 100 + j);
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(rb.len(), 100);
    }

    #[test]
    fn test_get_all_ordering_after_wrap() {
        let rb = RingBuffer::new(4);
        rb.push(1);
        rb.push(2);
        rb.push(3);
        rb.push(4);
        rb.push(5); // wraps, overwrites 1
        rb.push(6); // wraps, overwrites 2

        let all = rb.get_all();
        assert_eq!(all, vec![3, 4, 5, 6]);
    }

    #[test]
    fn test_get_last_after_wrap() {
        let rb = RingBuffer::new(4);
        rb.push(1);
        rb.push(2);
        rb.push(3);
        rb.push(4);
        rb.push(5);
        rb.push(6);

        assert_eq!(rb.get_last(2), vec![5, 6]);
    }

    #[test]
    fn test_option_values() {
        let rb: RingBuffer<Option<i32>> = RingBuffer::new(3);
        rb.push(Some(1));
        rb.push(None);
        rb.push(Some(3));
        let all = rb.get_all();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0], Some(1));
        assert_eq!(all[1], None);
        assert_eq!(all[2], Some(3));
    }

    #[test]
    fn test_ring_buffer_with_vec() {
        let rb: RingBuffer<Vec<i32>> = RingBuffer::new(3);
        rb.push(vec![1, 2]);
        rb.push(vec![3, 4]);
        rb.push(vec![5, 6]);

        let all = rb.get_all();
        assert_eq!(all[0], vec![1, 2]);
        assert_eq!(all[2], vec![5, 6]);
    }

    // --- Benchmark-style throughput test ---
    #[test]
    fn test_ring_buffer_push_throughput() {
        let rb = RingBuffer::new(10_000);
        let count = 100_000;

        let start = std::time::Instant::now();
        for i in 0..count {
            rb.push(i);
        }
        let elapsed = start.elapsed();

        assert_eq!(rb.len(), 10_000);
        // Should push 100k items in under 500ms
        assert!(elapsed < std::time::Duration::from_millis(500), "RingBuffer push too slow: {:?}", elapsed);
    }

    #[test]
    fn test_ring_buffer_get_all_throughput() {
        let rb = RingBuffer::new(10_000);
        for i in 0..10_000 {
            rb.push(i);
        }

        let start = std::time::Instant::now();
        for _ in 0..1000 {
            let _ = rb.get_all();
        }
        let elapsed = start.elapsed();

        // Should read 1000 times in under 500ms
        assert!(elapsed < std::time::Duration::from_millis(500), "RingBuffer get_all too slow: {:?}", elapsed);
    }
}
