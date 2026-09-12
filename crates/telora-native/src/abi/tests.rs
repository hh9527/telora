use super::*;

#[test]
fn nested_diagnostic_scopes_move_only_their_own_reports() {
    let mut context = CallContext::default();
    context.warn("outside".into(), Origin::default(), vec![]);
    let outer = context.diagnostics().len();
    context.warn("outer".into(), Origin::default(), vec![]);
    let inner = context.diagnostics().len();
    context.fail("inner failure");
    let inner = context.take_scoped_diagnostics(inner).unwrap().unwrap();
    assert_eq!(inner.len(), 1);
    assert_eq!(inner[0].message, "inner failure");
    assert!(!context.is_aborted());
    let outer = context.take_scoped_diagnostics(outer).unwrap().unwrap();
    assert_eq!(outer.len(), 1);
    assert_eq!(outer[0].message, "outer");
    assert_eq!(context.diagnostics()[0].message, "outside");
    assert_eq!(context.boundary(|_| Status::Success), Status::Success);
}

#[test]
fn execution_abort_cannot_be_captured_or_resume_execution() {
    for mode in 0..3 {
        let mut context = CallContext::default().with_fuel(0).with_call_depth_limit(1);
        context.warn("before abort".into(), Origin::default(), vec![]);
        let status = match mode {
            0 => context.consume_fuel(1, Origin::default()),
            1 => {
                assert_eq!(context.enter_call(Origin::default()), Status::Success);
                let status = context.enter_call(Origin::default());
                // Stack guards must still unwind after an unrecoverable abort.
                assert_eq!(context.leave_call(), Status::Success);
                assert_eq!(context.call_depth(), 0);
                status
            }
            _ => context.boundary(|_| panic!("host panic")),
        };
        assert_eq!(status, Status::Failed);
        assert!(context.is_aborted());
        assert!(context.take_scoped_diagnostics(0).unwrap().is_none());
        assert_eq!(context.diagnostics().len(), 2);
        assert_eq!(context.boundary(|_| panic!("must not run")), Status::Failed);
        assert_eq!(context.enter_call(Origin::default()), Status::Failed);
        assert_eq!(context.consume_fuel(0, Origin::default()), Status::Failed);
        assert_eq!(context.diagnostics().len(), 2);
    }
}
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
