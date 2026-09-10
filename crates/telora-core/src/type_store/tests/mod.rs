use super::*;
use crate::{ModuleId, TypeConstructorId};

fn constructor(local: u32) -> TypeConstructorId {
    TypeConstructorId {
        module: ModuleId::from_index(0),
        local,
    }
}

include!("part-01.rs");

#[test]
fn solved_ids_are_disjoint_from_runtime_interning_and_round_trip() {
    let mut store = TypeStore::default();
    let dynamic = store.intern_structural(TypeShape::Tuple(Box::new([])));
    for raw in [0, 1, 1024, (1 << 30) - 2] {
        let source = crate::mir::TypeId(raw);
        let id = TypeId::solved(source);
        assert_eq!(id.solved_id(), Some(source));
        assert_eq!(id.unchecked().solved_id(), Some(source));
        assert_eq!(TypeId::from_raw(id.raw()), Some(id));
        assert!(id.index().is_none());
        assert!(store.get(id).is_none());
        assert_ne!(id, dynamic);
    }
    assert_eq!(TypeId::INT.solved_id(), None);
    assert_eq!(dynamic.solved_id(), None);
    assert_eq!(TypeId::from_raw(1 << 30).unwrap().solved_id(), None);
}
