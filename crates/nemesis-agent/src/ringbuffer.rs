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
mod tests;
