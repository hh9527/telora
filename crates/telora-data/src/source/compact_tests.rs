use super::compact::{CompactLoc, LineIndex};

#[test]
fn full_width_positions_round_trip_in_twelve_bytes() {
    assert_eq!(core::mem::size_of::<CompactLoc>(), 12);
    for source in [1, 65535] {
        for (start, end) in [
            (0, 0),
            (255 << 24, 256 << 24),
            (65535u64 << 24, (1u64 << 40) - 1),
        ] {
            let loc = CompactLoc::new(source, start, end).unwrap();
            assert_eq!((loc.source(), loc.start(), loc.end()), (source, start, end));
        }
    }
    assert!(CompactLoc::new(65536, 0, 0).is_err());
    assert!(CompactLoc::new(1, 0, 1 << 40).is_err());
    assert!(CompactLoc::new(1, 2, 1).is_err());
}

#[test]
fn line_and_column_limits_reject_overflow() {
    assert!(LineIndex::new(&"\n".repeat(65535)).is_ok());
    assert!(LineIndex::new(&"\n".repeat(65536)).is_err());
    assert!(LineIndex::new(&"x".repeat(0xffffff)).is_ok());
    assert!(LineIndex::new(&"x".repeat(0x1000000)).is_err());
}

#[test]
fn mixed_endings_and_crlf_interior_have_explicit_coordinates() {
    let index = LineIndex::new("é\r\nx\ry\n").unwrap();
    assert_eq!(index.point(2), 2);
    assert_eq!(index.point(3), 2);
    assert_eq!(index.point(4), 1 << 24);
    assert_eq!(index.point(6), 2 << 24);
    assert_eq!(index.point(8), 3 << 24);
    assert_eq!(index.byte((1 << 24) | 1), Some(5));
    assert_eq!(index.byte((1 << 24) | 2), None);
}
