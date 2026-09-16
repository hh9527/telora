use super::coordinates::{SourceCoordinates, LineIndex};

#[test]
fn full_width_coordinates_round_trip_without_bit_partitions() {
    assert_eq!(core::mem::size_of::<SourceCoordinates>(), 20);
    for source in [1, 70_000, u32::MAX] {
        for (start, end) in [(0, 0), (255u64 << 32, 256u64 << 32), (70_000u64 << 32, u64::MAX)] {
            let loc = SourceCoordinates::new(source, start, end).unwrap();
            assert_eq!((loc.source(), loc.start(), loc.end()), (source, start, end));
        }
    }
    assert!(SourceCoordinates::new(0, 0, 0).is_err());
    assert!(SourceCoordinates::new(1, 2, 1).is_err());
}

#[test]
fn sources_no_longer_have_sixteen_bit_line_limits() {
    let index = LineIndex::new(&"\n".repeat(70_000)).unwrap();
    assert_eq!(index.point(70_000), 70_000u64 << 32);
    assert_eq!(index.byte(70_000u64 << 32), Some(70_000));
}

#[test]
fn mixed_endings_and_crlf_interior_have_explicit_coordinates() {
    let index = LineIndex::new("é\r\nx\ry\n").unwrap();
    assert_eq!(index.point(2), 2);
    assert_eq!(index.point(3), 2);
    assert_eq!(index.point(4), 1 << 32);
    assert_eq!(index.point(6), 2 << 32);
    assert_eq!(index.point(8), 3 << 32);
    assert_eq!(index.byte((1 << 32) | 1), Some(5));
    assert_eq!(index.byte((1 << 32) | 2), None);
}
