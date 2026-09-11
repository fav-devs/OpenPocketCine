//! Unpacking the length-prefixed blobs the facade returns.
//!
//! A single call can answer with several frames — tap-to-focus is three, and one
//! datagram can carry more than one reply — so those come back as
//! `[u16le count]([u16le length][bytes])…`. Reading that shape is plain arithmetic and
//! is tested on its own, without a camera or a core.

/// One DUML frame lifted out of a datagram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumlFrame {
    pub sender: u8,
    pub receiver: u8,
    pub seq: u16,
    pub flags: u8,
    pub cmd_set: u8,
    pub cmd_id: u8,
    pub payload: Vec<u8>,
}

impl DumlFrame {
    /// `[sender][receiver][seq lo][seq hi][flags][set][id][payload…]`.
    pub(crate) fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 7 {
            return None;
        }
        Some(Self {
            sender: bytes[0],
            receiver: bytes[1],
            seq: u16::from(bytes[2]) | (u16::from(bytes[3]) << 8),
            flags: bytes[4],
            cmd_set: bytes[5],
            cmd_id: bytes[6],
            payload: bytes[7..].to_vec(),
        })
    }

    /// True when this frame is a reply rather than a request.
    pub fn is_reply(&self) -> bool {
        self.flags & 0x80 != 0
    }
}

/// Splits `[u16le count]([u16le length][bytes])…` into its parts.
///
/// A blob that runs out mid-record yields what it could read rather than failing: the
/// caller asked the core for these bytes, so a short read is a bug to see in a test,
/// not a runtime error to handle.
pub(crate) fn split(blob: &[u8]) -> Vec<&[u8]> {
    if blob.len() < 2 {
        return Vec::new();
    }
    let count = usize::from(blob[0]) | (usize::from(blob[1]) << 8);
    let mut out = Vec::with_capacity(count);
    let mut at = 2;
    while out.len() < count && at + 2 <= blob.len() {
        let length = usize::from(blob[at]) | (usize::from(blob[at + 1]) << 8);
        at += 2;
        if at + length > blob.len() {
            break;
        }
        out.push(&blob[at..at + length]);
        at += length;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(records: &[&[u8]]) -> Vec<u8> {
        let mut out = vec![records.len() as u8, 0];
        for record in records {
            out.push(record.len() as u8);
            out.push(0);
            out.extend_from_slice(record);
        }
        out
    }

    #[test]
    fn records_come_back_in_order() {
        let packed = blob(&[&[1, 2, 3], &[4], &[5, 6]]);
        let split = split(&packed);
        assert_eq!(split, vec![&[1u8, 2, 3][..], &[4][..], &[5, 6][..]]);
    }

    #[test]
    fn an_empty_blob_yields_nothing() {
        assert!(split(&[0, 0]).is_empty());
        assert!(split(&[]).is_empty());
        assert!(split(&[7]).is_empty());
    }

    #[test]
    fn a_truncated_record_stops_rather_than_panicking() {
        let mut packed = blob(&[&[1, 2, 3], &[4, 5, 6]]);
        packed.truncate(packed.len() - 2);
        assert_eq!(split(&packed), vec![&[1u8, 2, 3][..]]);
    }

    #[test]
    fn a_frame_parses_its_header_and_payload() {
        let frame = DumlFrame::parse(&[0x0A, 0x02, 0x34, 0x12, 0x80, 0x02, 0xA5, 0x00, 0x01])
            .expect("a full frame");
        assert_eq!(frame.sender, 0x0A);
        assert_eq!(frame.receiver, 0x02);
        assert_eq!(frame.seq, 0x1234);
        assert_eq!(frame.cmd_set, 0x02);
        assert_eq!(frame.cmd_id, 0xA5);
        assert_eq!(frame.payload, vec![0x00, 0x01]);
        assert!(frame.is_reply());
    }

    #[test]
    fn a_frame_with_no_payload_is_still_a_frame() {
        let frame = DumlFrame::parse(&[1, 2, 0, 0, 0x40, 3, 4]).expect("a header-only frame");
        assert!(frame.payload.is_empty());
        assert!(!frame.is_reply());
    }

    #[test]
    fn a_short_frame_is_refused() {
        assert!(DumlFrame::parse(&[1, 2, 3]).is_none());
    }
}
