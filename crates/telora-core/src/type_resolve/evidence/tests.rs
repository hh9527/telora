use super::*;

fn ty(mir: &mut Mir, constructor: TypeConstructor, arguments: Vec<TypeId>) -> TypeId {
    let id = TypeId(mir.types.len() as u32);
    mir.types.push(ResolvedType {
        constructor,
        arguments,
    });
    id
}

#[test]
fn deep_shared_type_matching_occurs_and_substitution_use_bounded_stack() {
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            let mut mir = Mir::default();
            let parameter = SymbolId(0);
            let param = ty(&mut mir, TypeConstructor::Parameter(parameter), vec![]);
            let int = ty(&mut mir, TypeConstructor::Int, vec![]);
            let mut template = param;
            let mut concrete = int;
            for _ in 0..4_000 {
                template = ty(&mut mir, TypeConstructor::Tuple, vec![template, template]);
                concrete = ty(&mut mir, TypeConstructor::Tuple, vec![concrete, concrete]);
            }
            let mut canonical = mir
                .types
                .iter()
                .enumerate()
                .map(|(i, ty)| {
                    (
                        (ty.constructor.clone(), ty.arguments.clone()),
                        TypeId(i as u32),
                    )
                })
                .collect();
            let mut solver = Solver::new(&mut mir);
            let mut substitutions = BTreeMap::new();
            assert!(solver.match_type(template, concrete, &mut substitutions));
            assert_eq!(substitutions[&parameter], int);
            assert!(solver.contains_parameter(template));
            assert!(!solver.contains_parameter(concrete));
            assert!(solver.pattern_occurs(parameter, template, &BTreeMap::new()));
            assert!(!solver.pattern_occurs(SymbolId(1), template, &BTreeMap::new()));
            assert_eq!(
                solver.substitute_resolved(template, &substitutions, &mut canonical),
                concrete
            );
            assert_eq!(
                solver.mir.type_substitution.nodes.len(),
                4_001,
                "shared children have one operation slot"
            );
            assert!(
                solver
                    .mir
                    .type_substitution
                    .nodes
                    .iter()
                    .all(|node| node.result.is_some())
            );
            substitutions.insert(parameter, param);
            assert_eq!(
                solver.substitute_resolved(template, &substitutions, &mut canonical),
                template,
                "a different substitution context must not reuse the previous result"
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn parameter_scan_retains_binder_context_for_shared_types() {
    let mut mir = Mir::default();
    let bound = ty(&mut mir, TypeConstructor::Bound(0), vec![]);
    let closed = ty(&mut mir, TypeConstructor::Quantified(1), vec![bound]);
    let mixed = ty(&mut mir, TypeConstructor::Tuple, vec![bound, closed]);
    let solver = Solver::new(&mut mir);
    assert!(!solver.contains_parameter(closed));
    assert!(solver.contains_parameter(mixed));
}

#[test]
fn deeply_nested_literal_compatibility_does_not_reenter_equal() {
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            let mut mir = Mir::default();
            let mut solver = Solver::new(&mut mir);
            let mut left = solver.structure(TypeConstructor::Int, vec![]);
            let mut right = solver.structure(TypeConstructor::Int, vec![]);
            for _ in 0..2_000 {
                left = solver.structure(TypeConstructor::ArrayLiteral, vec![left]);
                right = solver.structure(TypeConstructor::ArrayLiteral, vec![right]);
            }
            solver.equal(left, right, None);
            assert!(solver.mir.type_conflicts.is_empty());
            solver.finalize();
            assert!(matches!(
                solver.mir.ty_slots[left.index()],
                TypeState::Known(_)
            ));
            assert_eq!(
                solver.mir.ty_slots[left.index()],
                solver.mir.ty_slots[right.index()]
            );
        })
        .unwrap()
        .join()
        .unwrap();
}
