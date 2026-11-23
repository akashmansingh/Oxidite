//! Append-only segmented session output log, compaction, and asciicast export.

use oxidite_proto::Offset;
use oxidite_proto::SessionId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogSegment {
    pub start_offset: Offset,
    pub bytes: Vec<u8>,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionLog {
    pub session_id: SessionId,
    pub segments: Vec<LogSegment>,
    pub segment_size_limit: usize,
    pub snapshot_checkpoint: Option<(Offset, Vec<u8>)>,
}

impl SessionLog {
    pub fn new(session_id: SessionId, segment_size_limit: usize) -> Self {
        Self {
            session_id,
            segments: Vec::new(),
            segment_size_limit,
            snapshot_checkpoint: None,
        }
    }

    /// Appends new output chunk into the segmented log.
    pub fn append(&mut self, offset: Offset, bytes: &[u8], timestamp_ms: u64) {
        if bytes.is_empty() {
            return;
        }

        // If last segment has space, append to it; otherwise allocate new segment
        if let Some(last) = self.segments.last_mut()
            && last.bytes.len() + bytes.len() <= self.segment_size_limit
        {
            last.bytes.extend_from_slice(bytes);
            return;
        }

        self.segments.push(LogSegment {
            start_offset: offset,
            bytes: bytes.to_vec(),
            timestamp_ms,
        });
    }

    /// Compacts segments up to a specified offset, replacing older raw chunks with a snapshot checkpoint.
    pub fn compact(&mut self, up_to_offset: Offset, snapshot_bytes: Vec<u8>) {
        self.snapshot_checkpoint = Some((up_to_offset, snapshot_bytes));
        self.segments.retain(|seg| {
            let end_offset = seg.start_offset + seg.bytes.len() as u64;
            end_offset > up_to_offset
        });
    }

    /// Replays logged bytes across a given offset range [from, to).
    pub fn replay_range(&self, from: Offset, to: Option<Offset>) -> Vec<u8> {
        let mut out = Vec::new();

        // If starting from before compacted segments, seed from snapshot
        if let Some((snap_offset, ref snap_bytes)) = self.snapshot_checkpoint
            && from <= snap_offset
        {
            out.extend_from_slice(snap_bytes);
        }

        for seg in &self.segments {
            let seg_end = seg.start_offset + seg.bytes.len() as u64;
            if seg_end <= from {
                continue;
            }
            if let Some(limit) = to
                && seg.start_offset >= limit
            {
                break;
            }

            let start_idx = if from > seg.start_offset {
                (from - seg.start_offset) as usize
            } else {
                0
            };

            let end_idx = if let Some(limit) = to {
                if limit < seg_end {
                    (limit - seg.start_offset) as usize
                } else {
                    seg.bytes.len()
                }
            } else {
                seg.bytes.len()
            };

            if start_idx < end_idx && start_idx < seg.bytes.len() {
                out.extend_from_slice(&seg.bytes[start_idx..end_idx]);
            }
        }

        out
    }

    /// Exports recorded session segments to asciicast v2 format.
    pub fn export_asciicast(&self, width: u16, height: u16) -> String {
        let mut lines = Vec::new();

        // Line 1: Header
        let first_ts = self.segments.first().map(|s| s.timestamp_ms).unwrap_or(0);
        let header = serde_json::json!({
            "version": 2,
            "width": width,
            "height": height,
            "timestamp": first_ts / 1000,
            "title": format!("Oxidite Session {}", self.session_id)
        });
        lines.push(header.to_string());

        // Subsequent lines: [time_seconds_relative, "o", string_content]
        for seg in &self.segments {
            let time_rel_secs = (seg.timestamp_ms.saturating_sub(first_ts)) as f64 / 1000.0;
            let text = String::from_utf8_lossy(&seg.bytes).to_string();
            let event = serde_json::json!([time_rel_secs, "o", text]);
            lines.push(event.to_string());
        }

        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_segmented_log_and_replay() {
        let sid = SessionId::new_v4();
        let mut log = SessionLog::new(sid, 16);

        log.append(0, b"Hello ", 1000);
        log.append(6, b"World!\r\n", 1100);
        log.append(14, b"Second chunk here", 1500);

        // Replay full range
        let replayed = log.replay_range(0, None);
        assert_eq!(replayed, b"Hello World!\r\nSecond chunk here");

        // Replay slice
        let slice = log.replay_range(6, Some(14));
        assert_eq!(slice, b"World!\r\n");
    }

    #[test]
    fn test_compaction() {
        let sid = SessionId::new_v4();
        let mut log = SessionLog::new(sid, 32);

        log.append(0, b"Old data line 1\r\n", 1000);
        log.append(17, b"Old data line 2\r\n", 1100);
        log.append(34, b"Recent data line\r\n", 1500);

        // Compact up to offset 34 with screen snapshot
        log.compact(34, b"[SNAPSHOT_AT_34]".to_vec());

        let replayed = log.replay_range(0, None);
        assert!(replayed.starts_with(b"[SNAPSHOT_AT_34]"));
        assert!(replayed.ends_with(b"Recent data line\r\n"));
    }

    #[test]
    fn test_asciicast_export() {
        let sid = SessionId::new_v4();
        let mut log = SessionLog::new(sid, 64);
        log.append(0, b"echo test\r\n", 1000);
        log.append(11, b"output\r\n", 1500);

        let cast = log.export_asciicast(80, 24);
        assert!(cast.contains("\"version\":2"));
        assert!(cast.contains("echo test"));
    }
}
