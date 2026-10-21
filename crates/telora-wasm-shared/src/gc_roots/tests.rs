use super::*;

#[test]
fn relocated_pins_keep_identity_and_reset_invalidates_all_generations() {
    let mut roots = Roots::new(16);
    let a = roots.pin(value(8)).unwrap();
    let b = roots.pin(value(16)).unwrap();
    roots.rewrite_pins(|root| Root {
        reference: root.reference + 80,
        ..root
    });
    assert_eq!(roots.get(a).unwrap(), value(88));
    assert_eq!(roots.get(b).unwrap(), value(96));
    roots.invalidate_pins();
    assert_eq!(roots.get(a), Err(Error::StaleHandle));
    assert_eq!(roots.unpin(b), Err(Error::StaleHandle));
    for _ in 0..1000 {
        let c = roots.pin(value(24)).unwrap();
        assert!(c.generation() > b.generation());
        roots.invalidate_pins();
        assert_eq!(roots.get(c), Err(Error::StaleHandle));
    }
    assert_eq!(roots.iter().count(), 0);
    assert_eq!(roots.pins.len(), 2);
    assert_eq!(roots.free_pins.len(), 2);
}

fn value(reference: u32) -> Root {
    Root {
        reference,
        kind: Kind::Value { type_id: 7 },
    }
}

#[test]
fn caller_arguments_survive_reentrant_frames_and_failure_cleanup() {
    let mut roots = Roots::new(100);
    let caller = roots.enter(3).unwrap();
    roots.set(caller, 0, Some(value(8))).unwrap();
    roots.set(caller, 1, Some(value(16))).unwrap();
    let callee = roots.enter(2).unwrap();
    roots.set(callee, 0, Some(value(24))).unwrap();
    assert_eq!(
        roots.iter().collect::<Vec<_>>(),
        [value(8), value(16), value(24)]
    );
    assert_eq!(roots.leave(caller), Err(Error::InvalidFrame));
    roots.leave(callee).unwrap(); // Recoverable failure takes the same path.
    roots.set(caller, 1, None).unwrap();
    roots.set(caller, 2, Some(value(32))).unwrap();
    assert_eq!(roots.iter().collect::<Vec<_>>(), [value(8), value(32)]);
    roots.leave(caller).unwrap();
    assert_eq!(roots.iter().count(), 0);
    let recycled = roots.enter(3).unwrap();
    assert_eq!(
        roots.set(caller, 0, Some(value(8))),
        Err(Error::InvalidFrame)
    );
    assert_eq!(roots.iter().count(), 0);
    roots.leave(recycled).unwrap();
}

#[test]
fn tail_transfers_keep_root_depth_and_storage_bounded() {
    let mut roots = Roots::new(16);
    let mut frame = roots.enter(4).unwrap();
    for index in 1..100_000 {
        let previous = frame;
        frame = roots.tail(frame, &[value(index * 8), value(8)]).unwrap();
        assert_eq!(roots.depth(), 1);
        assert_eq!(roots.active_slots(), 2);
        assert_eq!(
            roots.iter().collect::<Vec<_>>(),
            [value(index * 8), value(8)]
        );
        assert_eq!(roots.leave(previous), Err(Error::InvalidFrame));
    }
    roots.leave(frame).unwrap();
    assert_eq!(roots.active_slots(), 0);
}

#[test]
fn handles_reject_release_and_reuse_and_reserve_unpin_capacity() {
    let mut roots = Roots::new(8);
    let first = roots.pin(value(8)).unwrap();
    let second = roots.pin(value(16)).unwrap();
    assert_eq!(roots.unpin(first), Ok(value(8)));
    let next = roots.pin(value(24)).unwrap();
    assert_eq!(roots.get(first), Err(Error::StaleHandle));
    assert_eq!(roots.unpin(first), Err(Error::StaleHandle));
    assert_eq!(roots.get(next), Ok(value(24)));
    assert_eq!(roots.get(second), Ok(value(16)));
    assert_eq!(roots.iter().count(), 2);
    roots.unpin(next).unwrap();
    roots.unpin(second).unwrap();
    assert_eq!(roots.iter().count(), 0);
    for _ in 0..10_000 {
        let handle = roots.pin(value(8)).unwrap();
        roots.unpin(handle).unwrap();
    }
    assert_eq!(roots.pins.len(), 2);
}

#[test]
fn failed_growth_preserves_live_roots_and_limits_empty_frames() {
    let mut roots = Roots::new(4);
    let frame = roots.enter(2).unwrap();
    roots.set(frame, 0, Some(value(8))).unwrap();
    assert_eq!(roots.tail(frame, &[value(16); 4]), Err(Error::Limit));
    assert_eq!(roots.enter(2), Err(Error::Limit));
    assert_eq!(roots.iter().collect::<Vec<_>>(), [value(8)]);
    assert_eq!(roots.set(frame, 2, None), Err(Error::InvalidSlot));
    let empty = roots.enter(0).unwrap();
    assert_eq!(roots.enter(0), Err(Error::Limit));
    roots.leave(empty).unwrap();
    roots.leave(frame).unwrap();
    roots.generation = u64::MAX;
    assert_eq!(roots.enter(0), Err(Error::Limit));
    assert_eq!(roots.depth(), 0);
}
