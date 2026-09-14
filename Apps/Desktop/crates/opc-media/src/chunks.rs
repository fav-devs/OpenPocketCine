//! Reassembling `0x00/0x27` chunks, and the cursor arithmetic for paging.
//!
//! Transcribed from `MediaChunkAssembler` and `MediaListCommand` in the core.

use std::collections::BTreeMap;

use opc_camera::DumlFrame;

use crate::model::VIDEO_HANDLE_BASE;

pub const PAGE_SIZE: usize = 45;
pub const NEWEST_SD: u32 = 0x0000_0001;
pub const NEWEST_INTERNAL: u32 = 0x4000_0001;
pub const SD_COUNTER: u8 = 1;
pub const INTERNAL_COUNTER: u8 = 2;

/// Collects one page's chunks, keyed by the counter the camera echoes.
#[derive(Debug, Default, Clone)]
pub struct ChunkAssembler {
    by_counter: BTreeMap<u8, Vec<u8>>,
    chunk_count: usize,
    saw_end: bool,
}

impl ChunkAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Accepts a frame. Only `0x00/0x27` contributes; true when it did.
    pub fn ingest(&mut self, frame: &DumlFrame) -> bool {
        if (frame.cmd_set, frame.cmd_id) != (0x00, 0x27) {
            return false;
        }
        self.ingest_payload(&frame.payload)
    }

    pub fn ingest_payload(&mut self, payload: &[u8]) -> bool {
        if payload.len() < 10 || payload[0] != 0x4A {
            return false;
        }
        let subtype = payload[1];
        if subtype == 0x03 {
            self.saw_end = true;
            return true;
        }
        if subtype != 0x01 || payload.len() <= 10 {
            return false;
        }
        let counter = payload[4];
        self.by_counter
            .entry(counter)
            .or_default()
            .extend_from_slice(&payload[10..]);
        self.chunk_count += 1;
        true
    }

    pub fn assembled(&self, counter: u8) -> &[u8] {
        self.by_counter.get(&counter).map_or(&[], Vec::as_slice)
    }

    /// Every chunk, in counter order.
    pub fn assembled_merged(&self) -> Vec<u8> {
        self.by_counter.values().flatten().copied().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.chunk_count == 0
    }

    pub fn chunk_count(&self) -> usize {
        self.chunk_count
    }

    pub fn saw_end(&self) -> bool {
        self.saw_end
    }
}

/// Oldest video handle on a page — seeds the cursor after the newest page.
pub fn oldest_video_handle(handles: &[u32]) -> Option<u32> {
    handles
        .iter()
        .copied()
        .filter(|handle| *handle >= VIDEO_HANDLE_BASE)
        .min()
}

/// The cursor for the page after the one listed with `current`: from a newest marker,
/// the page's oldest video handle; from a handle, the oldest video handle strictly
/// older than it. None at the end of the library.
pub fn next_cursor(handles: &[u32], current: u32) -> Option<u32> {
    if current == NEWEST_SD || current == NEWEST_INTERNAL {
        return oldest_video_handle(handles);
    }
    handles
        .iter()
        .copied()
        .filter(|handle| *handle >= VIDEO_HANDLE_BASE && *handle < current)
        .min()
}

pub fn has_older_page(record_count: usize, cursor: Option<u32>) -> bool {
    matches!(cursor, Some(cursor) if cursor > 0) && record_count >= PAGE_SIZE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(payload: &[u8]) -> DumlFrame {
        DumlFrame {
            sender: 0x01,
            receiver: 0x02,
            seq: 1,
            flags: 0xC0,
            cmd_set: 0x00,
            cmd_id: 0x27,
            payload: payload.to_vec(),
        }
    }

    #[test]
    fn chunks_are_keyed_by_counter_and_the_end_is_noted() {
        let mut assembler = ChunkAssembler::new();
        let mut first = vec![0x4A, 0x01, 0, 0, 1, 0, 0, 0, 0, 0];
        first.extend_from_slice(b"abc");
        let mut second = vec![0x4A, 0x01, 0, 0, 2, 0, 1, 0, 0, 0];
        second.extend_from_slice(b"xyz");
        assert!(assembler.ingest(&frame(&first)));
        assert!(assembler.ingest(&frame(&second)));
        assert!(assembler.ingest(&frame(&[0x4A, 0x03, 0, 0, 1, 0, 0, 0, 0, 0])));
        assert!(!assembler.ingest(&frame(&[0x4A, 0x04, 0, 0, 1, 0, 0, 0, 0, 0])));
        assert_eq!(assembler.assembled(SD_COUNTER), b"abc");
        assert_eq!(assembler.assembled(INTERNAL_COUNTER), b"xyz");
        assert_eq!(assembler.assembled_merged(), b"abcxyz");
        assert!(assembler.saw_end());
        assert_eq!(assembler.chunk_count(), 2);
    }

    #[test]
    fn only_the_media_reply_contributes() {
        let mut assembler = ChunkAssembler::new();
        let mut status = frame(&[0x4A, 0x01, 0, 0, 1, 0, 0, 0, 0, 0, 1]);
        status.cmd_id = 0x99;
        assert!(
            !assembler.ingest(&status),
            "4A 01 also prefixes parameter pushes"
        );
        assert!(assembler.is_empty());
    }

    #[test]
    fn paging_walks_video_handles_down() {
        let handles = [0x4010_4480, 0x0004_0010, 0x4010_4400, 0x4010_44C0];
        assert_eq!(next_cursor(&handles, NEWEST_INTERNAL), Some(0x4010_4400));
        assert_eq!(next_cursor(&handles, NEWEST_SD), Some(0x4010_4400));
        assert_eq!(next_cursor(&handles, 0x4010_4480), Some(0x4010_4400));
        assert_eq!(next_cursor(&handles, 0x4010_4400), None);
        assert_eq!(oldest_video_handle(&[0x0004_0010]), None);
        assert!(has_older_page(45, Some(0x4010_4400)));
        assert!(!has_older_page(44, Some(0x4010_4400)));
        assert!(!has_older_page(45, None));
    }
}
