//! Byte accumulation for a stream socket.
//!
//! Holds received bytes until the core reports a complete message and then drops what
//! was consumed. Compaction is amortised so a steady 25 fps feed does not memmove the
//! whole buffer on every frame.

/// A grow-and-drain byte buffer for framed stream reads.
#[derive(Debug, Default)]
pub struct ReceiveBuffer {
    data: Vec<u8>,
    start: usize,
}

impl ReceiveBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends freshly read bytes.
    pub fn extend(&mut self, bytes: &[u8]) {
        if self.start > 0 && self.start == self.data.len() {
            self.data.clear();
            self.start = 0;
        }
        self.data.extend_from_slice(bytes);
    }

    /// The bytes not yet consumed.
    pub fn readable(&self) -> &[u8] {
        &self.data[self.start..]
    }

    pub fn len(&self) -> usize {
        self.data.len() - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drops `count` bytes from the front.
    pub fn consume(&mut self, count: usize) {
        self.start = (self.start + count).min(self.data.len());
        if self.start == self.data.len() {
            self.data.clear();
            self.start = 0;
        } else if self.start >= 64 * 1024 && self.start * 2 >= self.data.len() {
            self.data.drain(..self.start);
            self.start = 0;
        }
    }

    /// Drops everything, keeping the allocation for the next join.
    pub fn reset(&mut self) {
        self.data.clear();
        self.start = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consuming_everything_reuses_the_allocation() {
        let mut buffer = ReceiveBuffer::new();
        buffer.extend(&[1, 2, 3, 4]);
        assert_eq!(buffer.readable(), &[1, 2, 3, 4]);
        buffer.consume(4);
        assert!(buffer.is_empty());
        assert_eq!(buffer.readable(), &[] as &[u8]);
    }

    #[test]
    fn partial_consumption_keeps_the_remainder_in_order() {
        let mut buffer = ReceiveBuffer::new();
        buffer.extend(&[1, 2, 3]);
        buffer.consume(1);
        buffer.extend(&[4, 5]);
        assert_eq!(buffer.readable(), &[2, 3, 4, 5]);
        assert_eq!(buffer.len(), 4);
        buffer.consume(2);
        assert_eq!(buffer.readable(), &[4, 5]);
    }

    #[test]
    fn over_consuming_clamps_instead_of_panicking() {
        let mut buffer = ReceiveBuffer::new();
        buffer.extend(&[1, 2]);
        buffer.consume(99);
        assert!(buffer.is_empty());
    }

    #[test]
    fn a_long_stream_does_not_grow_without_bound() {
        let mut buffer = ReceiveBuffer::new();
        let chunk = vec![7u8; 8 * 1024];
        for _ in 0..256 {
            buffer.extend(&chunk);
            buffer.consume(chunk.len());
        }
        assert!(buffer.is_empty());
        assert!(buffer.readable().is_empty());
    }

    #[test]
    fn reset_drops_a_partial_message() {
        let mut buffer = ReceiveBuffer::new();
        buffer.extend(&[1, 2, 3]);
        buffer.reset();
        assert!(buffer.is_empty());
    }
}
