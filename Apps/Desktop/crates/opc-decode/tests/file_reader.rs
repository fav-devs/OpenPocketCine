//! The file reader on the checked-in test stream: the player's source.

use std::path::PathBuf;

use opc_decode::{FileReader, OwnedPicture};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/testsrc.h265")
}

#[test]
fn a_clip_opens_reports_its_shape_and_decodes_to_the_end() {
    let mut reader = FileReader::open(&fixture()).expect("the fixture opens");
    let info = reader.info();
    assert!(info.width > 0 && info.height > 0, "{info:?}");
    let mut frames = 0;
    let mut last_pts = -1;
    while let Some((picture, pts)) = reader.next_picture().expect("decodes") {
        assert_eq!((picture.width, picture.height), (info.width, info.height));
        assert!(pts >= last_pts, "presentation times go forwards");
        last_pts = pts;
        frames += 1;
    }
    assert!(frames > 1, "a clip has more than one picture");
    // A second read past the end stays at the end.
    assert!(reader.next_picture().unwrap().is_none());
}

#[test]
fn seeking_back_to_the_start_decodes_again() {
    let mut reader = FileReader::open(&fixture()).unwrap();
    let first = reader.next_picture().unwrap().unwrap().0;
    while reader.next_picture().unwrap().is_some() {}
    reader.seek(0).unwrap();
    let again = reader
        .next_picture()
        .unwrap()
        .expect("a picture after the seek")
        .0;
    assert_eq!((again.width, again.height), (first.width, first.height));
}

#[test]
fn a_missing_file_is_an_error_not_a_crash() {
    assert!(FileReader::open(&PathBuf::from("/nowhere/clip.mp4")).is_err());
}

#[test]
fn stills_convert_to_limited_range_video_levels() {
    let white = OwnedPicture::from_rgba(2, 2, &[255; 16]);
    let picture = white.picture();
    assert!(picture.luma.iter().all(|y| *y == 235), "{:?}", picture.luma);
    assert_eq!(picture.chroma_blue, [128]);
    let black = OwnedPicture::black(4, 2);
    assert_eq!(black.picture().luma, [16; 8]);
    assert_eq!(black.picture().chroma_size(), (2, 1));
}
