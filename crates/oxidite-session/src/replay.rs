//! Replay buffer implementation for output streams.

use oxidite_proto::Offset;

#[derive(Debug, Clone)]
pub struct ReplayBuffer {
    capacity: usize,
    buffer: Vec<u8>,
    start_offset: Offset,
    total_bytes: Offset,
}

impl ReplayBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            buffer: Vec::with_capacity(capacity.min(64 * 1024)),
            start_offset: 0,
            total_bytes: 0,
        }
    }

    pub fn append(&mut self, data: &[u8]) {
        self.buffer.extend_from_slice(data);
        self.total_bytes += data.len() as Offset;
        if self.buffer.len() > self.capacity {
            let excess = self.buffer.len() - self.capacity;
            self.buffer.drain(0..excess);
            self.start_offset += excess as Offset;
        }
    }

    pub fn head_offset(&self) -> Offset {
        self.total_bytes
    }

    pub fn tail_offset(&self) -> Offset {
        self.start_offset
    }

    pub fn get_from(&self, offset: Offset) -> Option<Vec<u8>> {
        if offset < self.start_offset || offset > self.total_bytes {
            None
        } else {
            let start = (offset - self.start_offset) as usize;
            Some(self.buffer[start..].to_vec())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replay_buffer_capacity_and_offsets() {
        let mut buf = ReplayBuffer::new(10);
        assert_eq!(buf.head_offset(), 0);
        assert_eq!(buf.tail_offset(), 0);

        buf.append(b"hello");
        assert_eq!(buf.head_offset(), 5);
        assert_eq!(buf.tail_offset(), 0);
        assert_eq!(buf.get_from(0).unwrap(), b"hello");
        assert_eq!(buf.get_from(2).unwrap(), b"llo");

        // Exceed capacity
        buf.append(b"world123");
        // total bytes = 13, capacity = 10 -> excess = 3 -> tail = 3, head = 13
        assert_eq!(buf.head_offset(), 13);
        assert_eq!(buf.tail_offset(), 3);

        // Before tail returns None
        assert!(buf.get_from(2).is_none());
        // At tail returns available slice
        assert_eq!(buf.get_from(3).unwrap(), b"loworld123");
        // At head returns empty
        assert_eq!(buf.get_from(13).unwrap(), b"");
        // Beyond head returns None
        assert!(buf.get_from(14).is_none());
    }
}
