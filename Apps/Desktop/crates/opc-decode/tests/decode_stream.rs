//! Decodes a committed synthetic stream end to end.
//!
//! `tests/fixtures/testsrc.h265` is 20 frames of FFmpeg's `testsrc2` pattern at
//! 320x240, keyed every 10 frames. It is synthetic on purpose — no camera footage
//! belongs in the repository.

use opc_decode::{annexb, Codec, Decoder, Picture};

const STREAM: &[u8] = include_bytes!("fixtures/testsrc.h265");

fn decode_all<F: FnMut(&Picture<'_>)>(mut observe: F) -> usize {
    let mut decoder = Decoder::new(Codec::Hevc).expect("an HEVC decoder");
    let mut count = 0;
    for unit in annexb::access_units(STREAM) {
        decoder
            .send(unit)
            .expect("the decoder should accept an access unit");
        while let Some(picture) = decoder.receive().expect("decoding should not fail") {
            observe(&picture);
            count += 1;
        }
    }
    count
}

#[test]
fn the_stream_splits_into_one_access_unit_per_picture() {
    assert_eq!(annexb::access_units(STREAM).len(), 20);
}

#[test]
fn every_picture_decodes_at_the_expected_raster() {
    let mut sizes = Vec::new();
    let decoded = decode_all(|picture| sizes.push((picture.width, picture.height)));
    assert_eq!(decoded, 20);
    assert!(sizes.iter().all(|size| *size == (320, 240)));
}

#[test]
fn keyframes_are_reported_where_the_encoder_put_them() {
    let mut keyframes = 0;
    decode_all(|picture| {
        if picture.is_keyframe {
            keyframes += 1;
        }
    });
    // Encoded with keyint 10 across 20 frames.
    assert_eq!(keyframes, 2);
}

#[test]
fn planes_are_addressable_at_the_strides_reported() {
    let mut checked = false;
    decode_all(|picture| {
        if checked {
            return;
        }
        let (chroma_width, chroma_height) = picture.chroma_size();
        assert_eq!((chroma_width, chroma_height), (160, 120));
        assert!(picture.luma_stride >= picture.width as usize);
        assert!(picture.chroma_stride >= chroma_width as usize);
        assert_eq!(
            picture.luma.len(),
            picture.luma_stride * picture.height as usize
        );
        assert_eq!(
            picture.chroma_blue.len(),
            picture.chroma_stride * chroma_height as usize
        );
        assert_eq!(picture.chroma_red.len(), picture.chroma_blue.len());
        checked = true;
    });
    assert!(checked, "at least one picture should have been inspected");
}

#[test]
fn the_picture_is_not_blank() {
    let mut spread = 0u8;
    decode_all(|picture| {
        let row = &picture.luma[..picture.width as usize];
        let low = row.iter().copied().min().unwrap_or(0);
        let high = row.iter().copied().max().unwrap_or(0);
        spread = spread.max(high - low);
    });
    assert!(
        spread > 32,
        "a test pattern should vary across a row, saw spread {spread}"
    );
}

#[test]
fn a_flush_lets_decoding_start_again() {
    let mut decoder = Decoder::new(Codec::Hevc).expect("an HEVC decoder");
    let units = annexb::access_units(STREAM);
    decoder.send(units[0]).unwrap();
    while decoder.receive().unwrap().is_some() {}
    decoder.flush();

    let mut after = 0;
    for unit in &units {
        decoder.send(unit).unwrap();
        while decoder.receive().unwrap().is_some() {
            after += 1;
        }
    }
    assert_eq!(after, 20);
}

#[test]
fn an_empty_access_unit_is_ignored_rather_than_an_error() {
    let mut decoder = Decoder::new(Codec::Hevc).expect("an HEVC decoder");
    assert!(decoder.send(&[]).is_ok());
}

#[test]
fn an_avc_decoder_is_also_available_for_nano() {
    assert!(Decoder::new(Codec::H264).is_ok());
}

#[test]
fn an_owned_copy_packs_rows_tight_and_reads_back_the_same() {
    use opc_decode::OwnedPicture;

    let mut decoder = Decoder::new(Codec::Hevc).expect("an HEVC decoder");
    let units = annexb::access_units(STREAM);
    decoder
        .send(units[0])
        .expect("the decoder should accept an access unit");
    let picture = decoder
        .receive()
        .expect("decoding should not fail")
        .expect("the first access unit is a keyframe");

    let expected_row: Vec<u8> = picture.luma[..picture.width as usize].to_vec();
    let owned = OwnedPicture::copy_from(&picture);
    assert_eq!((owned.width, owned.height), (320, 240));
    assert!(owned.is_keyframe);

    let borrowed = owned.picture();
    // Padding is gone, so the stride is exactly the width.
    assert_eq!(borrowed.luma_stride, 320);
    assert_eq!(borrowed.chroma_stride, 160);
    assert_eq!(borrowed.luma.len(), 320 * 240);
    assert_eq!(borrowed.chroma_blue.len(), 160 * 120);
    assert_eq!(&borrowed.luma[..320], expected_row.as_slice());
}
