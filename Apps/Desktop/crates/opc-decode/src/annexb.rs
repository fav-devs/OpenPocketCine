//! Splitting a recorded elementary stream back into access units.
//!
//! The live path never needs this: the relay delivers one whole access unit per frame
//! message. It exists so a `--dump` file can be replayed through the decoder, which is
//! how the transport gets checked without a camera in the room.

/// A NAL unit and the offset its start code began at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Nal {
    start: usize,
    payload: usize,
    end: usize,
}

fn start_code_len(stream: &[u8], at: usize) -> Option<usize> {
    if stream[at..].starts_with(&[0, 0, 1]) {
        Some(3)
    } else if stream[at..].starts_with(&[0, 0, 0, 1]) {
        Some(4)
    } else {
        None
    }
}

fn nals(stream: &[u8]) -> Vec<Nal> {
    let mut found = Vec::new();
    let mut cursor = 0;
    let mut open: Option<(usize, usize)> = None;
    while cursor < stream.len() {
        if let Some(length) = start_code_len(stream, cursor) {
            if let Some((start, payload)) = open.take() {
                found.push(Nal {
                    start,
                    payload,
                    end: cursor,
                });
            }
            open = Some((cursor, cursor + length));
            cursor += length;
        } else {
            cursor += 1;
        }
    }
    if let Some((start, payload)) = open {
        found.push(Nal {
            start,
            payload,
            end: stream.len(),
        });
    }
    found
}

/// HEVC NAL kinds that lead an access unit: VPS, SPS, PPS, AUD, and prefix SEI.
fn leads_access_unit(kind: u8) -> bool {
    matches!(kind, 32..=35 | 39 | 40)
}

fn is_slice(kind: u8) -> bool {
    kind <= 31
}

/// Splits a recorded HEVC elementary stream into access units.
///
/// Parameter sets belong to the picture they precede, not the one they follow, so a
/// coded slice only opens a new unit once the current one already holds a slice.
pub fn access_units(stream: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut current: Option<usize> = None;
    let mut current_holds_slice = false;

    for nal in nals(stream) {
        let body = &stream[nal.payload..nal.end];
        let Some(header) = body.first() else { continue };
        let kind = (header >> 1) & 0x3F;

        let boundary = if is_slice(kind) {
            // `first_slice_segment_in_pic_flag` is the top bit of the first slice-header
            // byte, which follows the two-byte NAL header.
            current_holds_slice && body.get(2).is_some_and(|byte| byte & 0x80 != 0)
        } else {
            current_holds_slice && leads_access_unit(kind)
        };

        if boundary {
            if let Some(start) = current.replace(nal.start) {
                out.push(&stream[start..nal.start]);
            }
            current_holds_slice = false;
        } else if current.is_none() {
            current = Some(nal.start);
        }
        current_holds_slice |= is_slice(kind);
    }

    if let Some(start) = current {
        out.push(&stream[start..]);
    }
    out.retain(|unit| !unit.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `[0,0,0,1][header][third byte]` — kind and first-slice flag are all this needs.
    fn nal(kind: u8, first_slice: bool) -> Vec<u8> {
        vec![
            0,
            0,
            0,
            1,
            kind << 1,
            0x01,
            if first_slice { 0x80 } else { 0x00 },
        ]
    }

    #[test]
    fn parameter_sets_lead_the_picture_they_belong_to() {
        let mut stream = Vec::new();
        stream.extend(nal(32, false)); // VPS
        stream.extend(nal(33, false)); // SPS
        stream.extend(nal(34, false)); // PPS
        stream.extend(nal(19, true)); // IDR, first slice
        stream.extend(nal(1, true)); // next picture
        let units = access_units(&stream);
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].len(), 7 * 4);
        assert_eq!(units[1].len(), 7);
    }

    #[test]
    fn a_continuation_slice_stays_in_its_picture() {
        let mut stream = Vec::new();
        stream.extend(nal(1, true));
        stream.extend(nal(1, false));
        stream.extend(nal(1, true));
        let units = access_units(&stream);
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].len(), 14);
    }

    #[test]
    fn three_byte_start_codes_are_understood() {
        let mut stream = vec![0, 0, 1, 19 << 1, 0x01, 0x80];
        stream.extend([0, 0, 1, 1 << 1, 0x01, 0x80]);
        assert_eq!(access_units(&stream).len(), 2);
    }

    #[test]
    fn an_empty_stream_yields_nothing() {
        assert!(access_units(&[]).is_empty());
    }

    #[test]
    fn parameter_sets_with_no_picture_stay_one_unit() {
        let mut stream = Vec::new();
        stream.extend(nal(32, false));
        stream.extend(nal(33, false));
        assert_eq!(access_units(&stream).len(), 1);
    }

    #[test]
    fn a_new_picture_opens_at_its_leading_parameter_sets() {
        let mut stream = Vec::new();
        stream.extend(nal(1, true));
        stream.extend(nal(32, false)); // VPS for the next picture
        stream.extend(nal(19, true));
        let units = access_units(&stream);
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].len(), 7);
        assert_eq!(units[1].len(), 14);
    }
}
