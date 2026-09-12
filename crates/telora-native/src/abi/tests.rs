use super::*;
use telora_core::{
    mir::{Mir, ResolvedType, TypeConstructor as T},
    source::{SourceDatabase, TextRange},
};
fn layouts() -> Layouts {
    let mut mir = Mir {
        symbols_closed: true,
        types_solved: true,
        types: [T::Int, T::String, T::Tuple, T::Never]
            .into_iter()
            .map(|constructor| ResolvedType {
                constructor,
                arguments: vec![],
            })
            .collect(),
        ..Mir::default()
    };
    mir.type_layouts.resize(mir.types.len(), None);
    Layouts::from_mir(&mir.seal().unwrap()).unwrap()
}
#[test]
fn mixed_width_frames_preserve_identity_origin_and_recursive_activations() {
    let l = layouts();
    let ids = (0..4).map(|i| l.type_at(i).unwrap()).collect::<Vec<_>>();
    let f = FrameLayout::new(&l, &ids[..3]).unwrap();
    assert_eq!(
        f.slots()
            .iter()
            .map(|s| s.offset_words())
            .collect::<Vec<_>>(),
        [0, 3, 7]
    );
    assert_eq!(f.words(), 9);
    assert!(FrameLayout::new(&l, &[ids[3]]).is_err());
    let mut sources = SourceDatabase::default();
    let source = sources.add("test", "123");
    let origin = Origin::from_loc(Some(Loc::new(source, TextRange::new(1, 3).unwrap())));
    let value = l.value(ids[0], origin, &[42]).unwrap();
    assert_eq!(value.origin(), origin);
    assert_eq!(value.type_key(), ids[0]);
    let mut parent = f.activate();
    let mut child = f.activate();
    assert!(child.read(0).is_err());
    parent.write(0, &value).unwrap();
    child
        .write(0, &l.value(ids[0], Origin::default(), &[99]).unwrap())
        .unwrap();
    assert_eq!(parent.read(0).unwrap(), value.words());
    assert_eq!(child.read(0).unwrap()[2], 99);
    assert!(parent.write(1, &value).is_err());
    assert!(parent.read(1).is_err());
    assert!(l.value(ids[3], Origin::default(), &[]).is_err());
    assert_eq!(
        l.value(ids[2], Origin::default(), &[])
            .unwrap()
            .words()
            .len(),
        2
    );
}
#[test]
fn world_handles_and_helper_failures_are_explicit() {
    for world in [World::Main, World::Work] {
        let h = HeapRef::new(world, (1 << 31) - 1).unwrap();
        assert_eq!(h.world(), world);
        assert_eq!(h.slot(), (1 << 31) - 1);
        assert!(HeapRef::new(world, 1 << 31).is_err());
    }
    let mut context = CallContext::default();
    assert_eq!(context.boundary(|ctx| ctx.fail("original")), Status::Failed);
    assert_eq!(context.boundary(|_| Status::Failed), Status::Failed);
    assert_eq!(context.diagnostics().len(), 1);
    assert_eq!(context.diagnostics()[0].message, "original");
    assert_eq!(context.boundary(|_| panic!("test panic")), Status::Failed);
    assert_eq!(context.diagnostics().len(), 2);
}
