use super::{content::*, *};
use alloc::vec;

#[test]
fn live_slices_trim_both_ends_keep_gaps_and_share_after_repeated_collection() {
    let mut source = Content::default();
    let bytes: vec::Vec<_> = (0..200).collect();
    let raw = source.insert(&bytes).unwrap();
    let narrow = source.slice(&raw, 50, 70).unwrap();
    let left = source.slice(&raw, 10, 40).unwrap();
    let right = source.slice(&raw, 100, 150).unwrap();
    let distinct = source.insert(&bytes[10..40]).unwrap();
    let mut values = [narrow, left, right, distinct];
    for _ in 0..3 {
        let expected: vec::Vec<_> = values
            .iter()
            .map(|v| source.view(v).unwrap().to_vec())
            .collect();
        let mut live = LiveRanges::default();
        for value in &values {
            live.observe(&source, value).unwrap();
        }
        let (next, relocation) = live.copy(&source).unwrap();
        for value in &mut values {
            *value = relocation.apply(*value).unwrap();
        }
        assert_eq!(next.len(), 140 + 30); // Includes the gap, excludes dead head/tail.
        for (value, expected) in values.iter().zip(expected) {
            assert_eq!(next.view(value).unwrap(), expected);
        }
        let raws: vec::Vec<_> = values
            .iter()
            .map(|v| match v {
                Bytes::Slice(s) => s.raw_start,
                _ => panic!("non-inline fixture"),
            })
            .collect();
        assert_eq!(raws[0], raws[1]);
        assert_eq!(raws[1], raws[2]);
        assert_ne!(raws[2], raws[3]);
        source = next;
    }
}

#[test]
fn short_views_release_large_allocations_and_content_resets() {
    let mut source = Content::default();
    for size in [0, 15] {
        assert!(matches!(
            source.insert(&vec![1; size]).unwrap(),
            Bytes::Inline { .. }
        ));
        assert_eq!(source.len(), 0);
    }
    let raw = source.insert(&vec![42; 2000]).unwrap();
    let short = source.slice(&raw, 100, 115).unwrap();
    let mut live = LiveRanges::default();
    live.observe(&source, &short).unwrap();
    let (mut next, relocation) = live.copy(&source).unwrap();
    assert!(next.is_empty());
    assert_eq!(
        next.view(&relocation.apply(short).unwrap()).unwrap(),
        [42; 15]
    );
    let sixteen = next.insert(&[3; 16]).unwrap();
    assert!(matches!(sixteen, Bytes::Slice(_)));
    next.seal_work().unwrap();
    for _ in 0..100 {
        next.insert(&vec![7; 1000]).unwrap();
        next.reset().unwrap();
        assert_eq!(next.len(), 16);
        assert_eq!(next.view(&sixteen).unwrap(), [3; 16]);
    }
    assert_eq!(next.slice(&sixteen, 10, 17), Err(Error::Bounds));
    assert_eq!(next.slice(&sixteen, 16, 0), Err(Error::Bounds));
}

#[test]
fn payload_tag_does_not_steal_the_fifteenth_inline_byte() {
    let mut content = Content::default();
    for size in [0, 1, 14, 15, 16, 100] {
        let text = vec![0xff; size];
        let value = content.insert(&text).unwrap();
        let restored = Bytes::decode(value.encode().unwrap()).unwrap();
        assert_eq!(content.view(&restored).unwrap(), text);
    }
    let utf8 = "中文中文中".as_bytes(); // Fifteen UTF-8 bytes, not five payload bytes.
    let value = content.insert(utf8).unwrap();
    assert!(matches!(value, Bytes::Inline { len: 15, .. }));
    let payload = value.encode().unwrap();
    assert_eq!(&payload[..15], utf8);
    assert_eq!(payload[15], 15);
    assert_eq!(Bytes::decode([255; 16]), Err(Error::Bounds));
}

#[test]
fn content_tail_append_preserves_inline_values_and_forks_historical_views() {
    let mut content = Content::default();
    let small = content.insert(b"small").unwrap();
    let still_inline = content.append(&small, b" value").unwrap();
    assert!(matches!(still_inline, Bytes::Inline { .. }));
    assert_eq!(content.view(&still_inline).unwrap(), b"small value");

    let first = content
        .append(&still_inline, b" grows past inline")
        .unwrap();
    let second = content.append(&first, b" at the raw tail").unwrap();
    assert_eq!(
        content.view(&first).unwrap(),
        b"small value grows past inline"
    );
    assert_eq!(
        content.view(&second).unwrap(),
        b"small value grows past inline at the raw tail"
    );
    let branch = content.append(&first, b" on a branch").unwrap();
    assert_eq!(
        content.view(&second).unwrap(),
        b"small value grows past inline at the raw tail"
    );
    assert_eq!(
        content.view(&branch).unwrap(),
        b"small value grows past inline on a branch"
    );
    let (Bytes::Slice(a), Bytes::Slice(b), Bytes::Slice(c)) = (first, second, branch) else {
        panic!("large content uses slices");
    };
    assert_eq!(a.raw_start, b.raw_start);
    assert_ne!(a.raw_start, c.raw_start);
}

#[test]
fn work_collection_keeps_unobserved_frozen_content_and_relocates_only_suffix() {
    let mut content = Content::default();
    let frozen = content.insert(&[42; 100]).unwrap();
    content.seal_work().unwrap();
    let frozen_slice = content.slice(&frozen, 20, 40).unwrap();
    let dead = content.insert(&[7; 1000]).unwrap();
    let raw = content.insert(&[3; 200]).unwrap();
    let slice = content.slice(&raw, 30, 70).unwrap();
    let mut ranges = LiveRanges::default();
    ranges.observe(&content, &slice).unwrap();
    let (mut next, relocation) = ranges.copy_work(&content).unwrap();
    assert_eq!(next.len(), 140);
    assert_eq!(next.view(&frozen).unwrap(), [42; 100]);
    assert_eq!(relocation.apply(frozen_slice).unwrap(), frozen_slice);
    assert_eq!(relocation.apply(dead), Err(Error::Bounds));
    let moved = relocation.apply(slice).unwrap();
    assert_eq!(next.view(&moved).unwrap(), [3; 40]);
    next.reset().unwrap();
    assert_eq!(next.len(), 100);
    assert_eq!(next.view(&frozen).unwrap(), [42; 100]);
    assert_eq!(next.view(&moved), Err(Error::Bounds));
}
