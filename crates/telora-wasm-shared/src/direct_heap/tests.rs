use super::*;

#[test]
fn injected_registration_and_storage_failures_preserve_owned_allocations() {
    for point in [FailurePoint::Registration, FailurePoint::Storage] {
        let mut heap = Heap::new(1 << 20);
        let root = heap.allocate(17, 7).unwrap();
        heap.bytes_mut(root, 1).unwrap()[0] = 42;
        while heap.allocations.len() < heap.allocations.capacity() {
            heap.allocate(8, 0).unwrap();
        }
        let before = heap.usage();
        heap.fail_next = Some(point);
        assert_eq!(heap.allocate(4096, 9), Err(Error::Allocation));
        let after = heap.usage();
        assert_eq!(
            (after.allocated, after.storage, after.objects),
            (before.allocated, before.storage, before.objects)
        );
        assert_eq!(heap.bytes(root, 1).unwrap(), [42]);
        heap.allocate(4096, 9).unwrap();
        let mut freed = 0;
        heap.sweep(|_| freed += 1);
        assert_eq!(freed, before.objects + 1);
        assert_eq!(heap.usage().storage, 0);
        heap.sweep(|_| panic!("double release after failed allocation"));
    }
}

#[test]
fn independent_storage_keeps_addresses_and_zero_initializes() {
    let mut heap = Heap::new(1 << 20);
    let value = heap.allocate(17, 9).unwrap();
    assert_eq!(value % 8, 0);
    assert_eq!(heap.bytes(value, 17).unwrap(), [0; 17]);
    heap.bytes_mut(value + 16, 1).unwrap()[0] = 42;
    for _ in 0..4096 {
        heap.allocate(9, 0).unwrap();
    }
    assert_eq!(heap.bytes(value + 16, 1).unwrap(), [42]);
    assert_eq!(
        heap.block(value + 16).unwrap(),
        Block {
            address: value,
            bytes: 17,
            descriptor: 9
        }
    );
}

#[test]
fn exact_interior_marks_and_sweep_release_resources_once() {
    let mut heap = Heap::new(1024);
    let live = heap.allocate(17, 1).unwrap();
    let dead = heap.allocate(16, 2).unwrap();
    assert!(heap.mark(live + 16).unwrap().is_some());
    assert_eq!(heap.mark(live), Ok(None));
    let mut released = Vec::new();
    heap.sweep(|block| released.push(block));
    assert_eq!(
        released,
        [Block {
            address: dead,
            bytes: 16,
            descriptor: 2
        }]
    );
    assert_eq!(heap.usage().allocated, 17);
    assert_eq!(heap.usage().storage, 24);
    heap.sweep(|block| released.push(block));
    heap.sweep(|_| panic!("double resource release"));
    assert_eq!(released.len(), 2);
    assert_eq!(heap.usage().objects, 0);
    assert_eq!(heap.usage().storage, 0);
}

#[test]
fn interval_index_handles_allocations_after_collection_and_reuse() {
    let mut heap = Heap::new(1 << 20);
    let roots: Vec<_> = (0..200)
        .map(|i| heap.allocate(i + 1, i as u32).unwrap())
        .collect();
    for (i, &root) in roots.iter().enumerate() {
        if i % 3 == 0 {
            heap.mark(root + i).unwrap();
        }
    }
    heap.sweep(|_| {});
    for _ in 0..200 {
        heap.allocate(33, 0).unwrap();
    }
    for (i, &root) in roots.iter().enumerate() {
        if i % 3 == 0 {
            assert_eq!(heap.block(root + i).unwrap().address, root);
            heap.mark(root).unwrap();
        }
    }
    heap.sweep(|_| {});
    assert_eq!(heap.usage().objects, 67);
}

#[test]
fn ordinary_actor_and_property_roots_survive_repeated_calls() {
    let mut heap = Heap::new(4096);
    let actor = heap.allocate(24, 0).unwrap();
    let property = heap.allocate(32, 0).unwrap();
    heap.bytes_mut(property, 1).unwrap()[0] = 7;
    for _ in 0..100 {
        heap.allocate(128, 0).unwrap();
        heap.mark(actor).unwrap();
        heap.mark(property).unwrap();
        heap.sweep(|_| {});
        assert_eq!(heap.usage().allocated, 56);
        assert_eq!(heap.bytes(property, 1).unwrap(), [7]);
    }
    heap.sweep(|_| {});
    assert_eq!(heap.usage().allocated, 0);
}

#[test]
fn padding_one_past_and_overflow_are_not_valid_references() {
    let mut heap = Heap::new(4096);
    let value = heap.allocate(17, 0).unwrap();
    // Offsets 17..24 belong to alignment padding, never a different allocation.
    assert_eq!(heap.mark(value + 17), Err(Error::InvalidReference));
    assert_eq!(heap.bytes(value + 16, 2), Err(Error::InvalidReference));
    assert_eq!(heap.bytes(value, usize::MAX), Err(Error::InvalidReference));
    assert_eq!(heap.mark(0), Err(Error::InvalidReference));
    assert_eq!(heap.allocate(usize::MAX, 0), Err(Error::Overflow));
    let zero = heap.allocate(0, 0).unwrap();
    assert_eq!(heap.block(zero).unwrap().bytes, 1);
}

#[test]
fn budget_covers_registry_and_layouts_and_rejection_preserves_objects() {
    let mut heap = Heap::new(0);
    assert_eq!(heap.allocate(1, 0), Err(Error::Limit));
    assert_eq!(heap.usage(), Usage::default());
    heap.set_limit(4096).unwrap();
    let value = heap.allocate(17, 0).unwrap();
    let usage = heap.usage();
    let exact = usage.storage + usage.metadata;
    heap.set_limit(exact).unwrap();
    assert_eq!(heap.allocate(1, 0), Err(Error::Limit));
    assert_eq!(heap.set_limit(exact - 1), Err(Error::Limit));
    assert_eq!(heap.usage(), usage);
    heap.mark(value).unwrap();
    heap.sweep(|_| panic!("live allocation released"));
    assert_eq!(heap.usage(), usage);
    heap.sweep(|_| {});
    assert_eq!(heap.usage().allocated, 0);
    assert_eq!(heap.usage().metadata, usage.metadata);
}

#[test]
fn tracing_sharing_and_cycles_matches_an_independent_graph_oracle() {
    use alloc::collections::BTreeSet;
    let mut heap = Heap::new(1 << 20);
    let mut seed = 7u32;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as usize
    };
    let width = core::mem::size_of::<usize>();
    for _ in 0..50 {
        let refs: Vec<_> = (0..200)
            .map(|_| heap.allocate(2 * width, 0).unwrap())
            .collect();
        let edges: Vec<_> = (0..200).map(|_| [random() % 200, random() % 200]).collect();
        for (&reference, children) in refs.iter().zip(&edges) {
            for (field, &child) in children.iter().enumerate() {
                heap.bytes_mut(reference + field * width, width)
                    .unwrap()
                    .copy_from_slice(&refs[child].to_le_bytes());
            }
        }
        let mut expected = BTreeSet::new();
        let mut work = alloc::vec![0, 13];
        while let Some(node) = work.pop() {
            if expected.insert(node) {
                work.extend(edges[node]);
            }
        }
        let mut work = alloc::vec![refs[0] + 1, refs[13]];
        while let Some(reference) = work.pop() {
            if let Some(block) = heap.mark(reference).unwrap() {
                for field in 0..2 {
                    let child = usize::from_le_bytes(
                        heap.bytes(block.address + field * width, width)
                            .unwrap()
                            .try_into()
                            .unwrap(),
                    );
                    work.push(child);
                }
            }
        }
        heap.sweep(|_| {});
        for (id, &reference) in refs.iter().enumerate() {
            assert_eq!(heap.block(reference).is_ok(), expected.contains(&id));
        }
        assert_eq!(heap.usage().objects, expected.len());
        heap.sweep(|_| {});
    }
}
