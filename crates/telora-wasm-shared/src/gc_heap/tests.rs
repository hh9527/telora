use super::*;
use alloc::{collections::BTreeSet, vec};

fn trace(heap: &mut Heap, roots: &[u32]) {
    let mut pending = roots.to_vec();
    while let Some(reference) = pending.pop() {
        if reference == 0 {
            continue;
        }
        if let Some(block) = heap.mark(reference).unwrap() {
            // Test descriptor is an explicit prefix of reference fields.
            for field in 0..block.descriptor {
                let bytes = heap.bytes(block.reference + field * 4, 4).unwrap();
                pending.push(u32::from_le_bytes(bytes.try_into().unwrap()));
            }
        }
    }
}

#[test]
fn marks_sharing_cycles_and_interior_roots_without_moving_live_objects() {
    let mut heap = Heap::new(4096, 1 << 20).unwrap();
    let a = heap.allocate(16, 1).unwrap();
    let b = heap.allocate(16, 1).unwrap();
    let dead = heap.allocate(16, 1).unwrap();
    heap.bytes_mut(a, 4)
        .unwrap()
        .copy_from_slice(&b.to_le_bytes());
    heap.bytes_mut(b, 4)
        .unwrap()
        .copy_from_slice(&a.to_le_bytes());
    heap.bytes_mut(dead, 4)
        .unwrap()
        .copy_from_slice(&dead.to_le_bytes());
    trace(&mut heap, &[a + 8, b]);
    let mut released = Vec::new();
    heap.sweep(|block| released.push(block.reference));
    assert_eq!(released, [dead]);
    assert_eq!(heap.block(a + 8).unwrap().reference, a);
    assert_eq!(heap.bytes(a, 4).unwrap(), b.to_le_bytes());
    assert_eq!(heap.allocate(16, 0).unwrap(), dead);
    assert_eq!(heap.bytes(dead, 16).unwrap(), [0; 16]);
    heap.sweep(|_| {});
    assert_eq!(heap.usage().objects, 0);
}

#[test]
fn repeated_long_work_reuses_committed_pages() {
    let mut heap = Heap::new(8, 256 * 1024).unwrap();
    let live = heap.allocate(24, 0).unwrap();
    heap.bytes_mut(live, 24).unwrap().fill(42);
    let mut committed = None;
    for _ in 0..200 {
        for _ in 0..1000 {
            heap.allocate(24, 0).unwrap();
        }
        trace(&mut heap, &[live]);
        heap.sweep(|_| {});
        assert_eq!(heap.usage().objects, 1);
        assert_eq!(heap.bytes(live, 24).unwrap(), [42; 24]);
        assert_eq!(*committed.get_or_insert(heap.usage()), heap.usage());
    }
}

#[test]
fn validates_extents_limits_and_releases_resources_once() {
    assert!(matches!(Heap::new(0, 100), Err(Error::InvalidReference)));
    let mut heap = Heap::new(8, 256 * 1024).unwrap();
    assert_eq!(heap.allocate(u32::MAX, 0), Err(Error::Overflow));
    assert_eq!(heap.allocate(512 * 1024, 0), Err(Error::Limit));
    let large = heap.allocate(20_003, 17).unwrap();
    heap.bytes_mut(large + 20_002, 1).unwrap()[0] = 7;
    assert_eq!(heap.bytes(large + 20_002, 2), Err(Error::InvalidReference));
    assert_eq!(heap.bytes(large + 20_003, 0), Err(Error::InvalidReference));
    assert_eq!(heap.mark(large + 20_003), Err(Error::InvalidReference));
    let mut released = Vec::new();
    heap.sweep(|block| released.push(block));
    heap.sweep(|block| released.push(block));
    assert_eq!(
        released,
        [Block {
            reference: large,
            bytes: 20_003,
            descriptor: 17
        }]
    );
    assert_eq!(heap.allocate(20_003, 0).unwrap(), large);
    let mut tiny = Heap::new(u32::MAX - 7, 1 << 20).unwrap();
    assert_eq!(tiny.allocate(16, 0), Err(Error::Overflow));
}

#[test]
fn graph_tracing_matches_an_independent_reachability_model() {
    let mut heap = Heap::new(8, 4 << 20).unwrap();
    let mut seed = 7u32;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for _ in 0..50 {
        let refs = (0..200)
            .map(|_| heap.allocate(24, 2).unwrap())
            .collect::<Vec<_>>();
        let edges = (0..200)
            .map(|_| [random() as usize % 200, random() as usize % 200])
            .collect::<Vec<_>>();
        for (reference, children) in refs.iter().zip(&edges) {
            for (field, &child) in children.iter().enumerate() {
                heap.bytes_mut(*reference + field as u32 * 4, 4)
                    .unwrap()
                    .copy_from_slice(&refs[child].to_le_bytes());
            }
        }
        let mut expected = BTreeSet::new();
        let mut work = vec![0usize, 13];
        while let Some(node) = work.pop() {
            if expected.insert(node) {
                work.extend(edges[node]);
            }
        }
        trace(&mut heap, &[refs[0], refs[13]]);
        heap.sweep(|_| {});
        for (id, &reference) in refs.iter().enumerate() {
            assert_eq!(heap.block(reference).is_some(), expected.contains(&id));
        }
        assert_eq!(heap.usage().objects as usize, expected.len());
        heap.sweep(|_| {});
    }
}

#[test]
fn vacant_pages_release_slots_and_reuse_backing_across_size_classes() {
    let mut heap = Heap::new(8, 128 * 1024).unwrap();
    for round in 0..1000 {
        let size = 20_000 + (round % 100) * 64;
        let value = heap.allocate(size, 0).unwrap();
        heap.bytes_mut(value + size - 1, 1).unwrap()[0] = 42;
        heap.sweep(|_| {});
        let usage = heap.usage();
        assert!(usage.committed >= u64::from(size));
        assert!(usage.committed + usage.metadata <= 128 * 1024);
        assert_eq!(usage.objects, 0);
        assert_eq!(usage.pages, 1);
        assert_eq!(
            usage.metadata,
            (heap.pages.capacity() * core::mem::size_of::<Page>()) as u64
        );
    }
}

#[test]
fn adopted_image_prefix_survives_sweep_and_dynamic_offsets_are_contiguous() {
    let mut heap = Heap::from_words(4096, 128 * 1024, vec![42; 8]).unwrap();
    let a = heap.allocate(16, 0).unwrap();
    assert_eq!(a, 4096 + 64);
    heap.bytes_mut(a, 16).unwrap().fill(7);
    let b = heap.allocate(20_000, 0).unwrap();
    heap.mark(a + 8).unwrap();
    heap.sweep(|_| {});
    assert_eq!(&heap.words()[..8], &[42; 8]);
    assert_eq!(heap.block(b), None);
    assert_eq!(heap.bytes(a, 16).unwrap(), [7; 16]);
    let image = heap.into_words();
    let restored = Heap::from_words(4096, 256 * 1024, image.clone()).unwrap();
    assert_eq!(restored.words(), image);
    assert_eq!(restored.usage().objects, 0);
    assert!(matches!(
        Heap::from_words(8, 8, vec![0; 2]),
        Err(Error::Limit)
    ));
}
