use super::*;

#[test]
fn seal_requires_a_record_for_every_construction_check() {
    let mut mir = graph(&[(
        "@src/main",
        "@check(fn(value) {Ok(())}) type Checked = struct(Int); export {Checked};",
    )]);
    resolve(&mut mir);
    mir.seal()
        .unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    mir.construction_checks.clear();
    assert!(
        mir.seal().is_err(),
        "a solved checker must not disappear before codegen"
    );

    let mut mir = graph(&[(
        "@src/main",
        r#"
        def verify: Fn(Int) -> Result((), Never) = fn(value) {Ok(())};
        type Choice = enum { @check(verify) Empty };
        export {Choice};
    "#,
    )]);
    resolve(&mut mir);
    assert!(!mir.diagnostics.is_empty());
    assert!(
        mir.type_conflicts.is_empty(),
        "the invalid placement has a well-typed checker"
    );
    assert!(
        mir.type_unknowns.is_empty(),
        "checker types are fully determined"
    );
    mir.diagnostics.clear();
    assert!(
        mir.seal().is_err(),
        "unsupported check sites cannot bypass seal by clearing diagnostics"
    );
}

#[test]
fn invalid_check_signatures_keep_the_original_conflict_and_contract_context() {
    for expression in [
        "fn(value) {True}",
        "fn(value) {Ok(value)}",
        "fn(value) {Err(\"bad\")}",
        "fn(value) {None}",
    ] {
        let source = format!(
            "@check({expression}) type Checked = struct(Int); export def independent: Int = 42;"
        );
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        let diagnostics = mir
            .diagnostics
            .iter()
            .filter(|d| d.message.starts_with("invalid @check function:"))
            .collect::<Vec<_>>();
        assert_eq!(diagnostics.len(), 1, "{}", mir.dump());
        assert!(diagnostics[0].message.contains("Result((), BlameError)"));
        assert!(diagnostics[0].message.contains("type mismatch"));
        assert!(!diagnostics[0].labels.is_empty());
        assert!(matches!(
            symbol_type(&mir, "independent"),
            TypeState::Known(_)
        ));
        assert!(mir.seal().is_err());
    }
    let mut mir = graph(&[(
        "@src/main",
        "@check(missing) type Checked = struct(Int); export def independent: Int = 42;",
    )]);
    let count = mir.diagnostics.len();
    resolve(&mut mir);
    assert_eq!(mir.diagnostics.len(), count);
    assert!(
        !mir.diagnostics
            .iter()
            .any(|d| d.message.starts_with("invalid @check"))
    );
    assert!(mir.seal().is_err());
}

#[test]
fn deferred_check_conflicts_retain_language_fixture_context() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/language/src/check");
    for case in [
        "empty-body",
        "err-payload",
        "input",
        "legacy-none",
        "legacy-some",
        "ok-payload",
        "result",
        "result-statement",
        "return-unit",
        "warning-tail",
    ] {
        let source =
            std::fs::read_to_string(directory.join(format!("diag-check-{case}/testee.telora")))
                .unwrap();
        let mut sources = vec![("@src/main", source.as_str())];
        sources.extend_from_slice(crate::static_sources::BUILTINS);
        let mut mir = graph(&sources);
        resolve(&mut mir);
        assert!(
            mir.diagnostics
                .iter()
                .any(|d| d.message.starts_with("invalid @check function:")),
            "{case}: {:?}",
            mir.diagnostics
        );
        assert!(mir.seal().is_err());
    }
}

#[test]
fn decorators_on_aliases_are_diagnosed_and_cannot_be_silently_dropped() {
    for declaration in [
        "type Prop = Int;",
        "type Base = struct {value: Int}; @property(PropertyTarget.Type) type Prop = Base;",
    ] {
        let source = if declaration.starts_with("type Prop") {
            format!(
                "@property(PropertyTarget.Type) {declaration} export def independent: Int = 42;"
            )
        } else {
            format!("{declaration} export def independent: Int = 42;")
        };
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        assert!(
            mir.diagnostics
                .iter()
                .any(|d| d.message.contains("aliases cannot own properties")),
            "{}",
            mir.dump()
        );
        assert!(matches!(
            symbol_type(&mir, "independent"),
            TypeState::Known(_)
        ));
        mir.diagnostics.clear();
        assert!(
            mir.seal().is_err(),
            "seal must reject unrecorded decorators independently of diagnostics"
        );
    }
    let mut mir = graph(&[(
        "@src/main",
        "@property(PropertyTarget.Type) type Mark = struct {value: Int}; export {Mark};",
    )]);
    resolve(&mut mir);
    mir.seal()
        .unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    mir.properties.clear();
    assert!(
        mir.seal().is_err(),
        "removing all property records must not erase the obligations"
    );
}

#[test]
fn property_admission_links_capabilities_without_evaluating_targets() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        import "std/prelude" {property as marker};
        def attach: Fn(PropertyTarget) -> Fn(Type, Option(PropertyAttr)) -> PropertyAttr = marker;
        def choose: Fn() -> PropertyTarget = fn() { fail!("must not execute in type solving") };
        @attach(choose()) type Mark = struct { value: Int };
        def property: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { {value: 42} };
        @property type Item = struct { value: Int };
        export def answer: TypeOf(Item) = Item.type;
    "#,
    )]);
    resolve(&mut mir);
    mir.seal()
        .unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    let index = mir
        .properties
        .iter()
        .position(|record| matches!(record.admission, Some(PropertyAdmission::Require { .. })))
        .unwrap();
    let Some(PropertyAdmission::Require {
        capability,
        targets,
    }) = mir.properties[index].admission
    else {
        unreachable!()
    };
    assert_eq!(targets, 3);
    assert_eq!(
        mir.properties[capability.index()].owner,
        mir.properties[index].property
    );
    assert_eq!(
        mir.properties[capability.index()].admission,
        Some(PropertyAdmission::Capability)
    );
    mir.properties[index].admission = Some(PropertyAdmission::Capability);
    assert!(mir.seal().is_err());
}

#[test]
fn property_admission_rejects_missing_and_forged_capability_records() {
    for (source, expected) in [
        (
            "type Mark = struct {value: Int}; def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { {value: 42} }; @mark type Item = struct {value: Int}; export def answer: TypeOf(Item) = Item.type;",
            "no @property capability declaration",
        ),
        (
            "def forged: Fn(Type, Option(PropertyAttr)) -> PropertyAttr = fn(owner, previous) { {bits: 63} }; @forged type Mark = struct {value: Int}; export def answer: TypeOf(Mark) = Mark.type;",
            "reserved for @property capability records",
        ),
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(
            mir.diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(expected)),
            "{}",
            mir.dump()
        );
        assert!(mir.seal().is_err());
    }
}

#[test]
fn generic_properties_close_provider_instances_before_sealing() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        @property(PropertyTarget.Type) type Mark(T) = struct { witness: TypeOf(T) };
        def mark: for(T) Fn(TypeOf(T)) -> Fn(Type, Option(Mark(T))) -> Mark(T) = fn(witness) {
            fn(owner, previous) { {witness: witness} }
        };
        @mark(T.type) type Box(T) = struct { value: T };
        type Outer(T) = struct { value: Box(Array(T)) };
        export def answer: (TypeOf(Box(Int)), TypeOf(Box(String)), TypeOf(Outer(Int))) = (Box(Int).type, Box(String).type, Outer(Int).type);
    "#,
    )]);
    resolve(&mut mir);
    mir.seal()
        .unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    let box_symbol = mir
        .symbols
        .iter()
        .position(|symbol| {
            symbol.name == "Box" && symbol.kind == SymbolKind::Declaration(BindingKind::Type)
        })
        .unwrap();
    let records = mir
        .properties
        .iter()
        .enumerate()
        .filter(|(_, record)| {
            record.concrete
                && mir.types[record.owner.index()].constructor
                    == TypeConstructor::Nominal(SymbolId(box_symbol as u32))
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 3);
    for &index in &records {
        let record = &mir.properties[index];
        let instance = &mir.generic_instances[record.instance.unwrap().index()];
        assert!(instance.concrete);
        assert_eq!(instance.ty(record.providers[0]), Some(record.property));
        assert_eq!(
            mir.types[record.owner.index()].arguments,
            mir.types[record.property.index()].arguments
        );
    }
    let instance = mir.properties[records[0]].instance.take();
    assert!(mir.seal().is_err());
    mir.properties[records[0]].instance = instance;
    mir.properties.remove(records[0]);
    assert!(mir.seal().is_err());
}

#[test]
fn property_target_members_use_native_identity_and_ordinary_resolution() {
    let mut mir = graph(&[(
        "@src/main",
        "import \"std/prelude\" {PropertyTarget as Target}; import Target.{Member as Both}; def target: Fn(Bool) -> Target = fn(enabled) { if enabled { Target.StructType } else { Target.EnumType } }; @property(target(True)) type Mark = struct {value: Int}; export def answer: (Target, Target, Target, Target, Target, Target) = (Target.Type, Target.StructType, Target.EnumType, Both, Target.Field, Target.Variant);",
    )]);
    resolve(&mut mir);
    mir.seal()
        .unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump()));
    let TypeState::Known(tuple) = symbol_type(&mir, "answer") else {
        panic!("closed targets")
    };
    assert_eq!(mir.types[tuple.index()].arguments.len(), 6);
    for &target in &mir.types[tuple.index()].arguments {
        assert_eq!(
            mir.types[target.index()].constructor,
            TypeConstructor::PropertyTarget
        );
    }
    for source in [
        "type PropertyTarget = enum {Type}; @property(PropertyTarget.Type) type Mark = struct {value: Int}; export def answer: Int = 42;",
        "import \"./other\" as property; @property(PropertyTarget.Type) type Mark = struct {value: Int}; export def answer: Int = 42;",
    ] {
        let mut mir = graph(&[
            ("@src/main", source),
            ("@src/other", "export def value: Int = 42;"),
        ]);
        resolve(&mut mir);
        assert!(mir.seal().is_err(), "{source}");
        assert!(!mir.type_conflicts.is_empty(), "{source}");
    }
}

#[test]
fn checked_return_closes_the_callable_signature_before_instantiation() {
    let mut mir = graph(&[(
        "@src/main",
        "type Box(T) = struct {value: T}; def finish: for(T) Fn(Unchecked(Box(T))) -> Box(T) = fn(value) { value }; def candidate: Unchecked(Box(Int)) = {value: 42}; export def answer: Box(Int) = finish(candidate);",
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    mir.seal().unwrap();
    let closure = HirId(
        mir.hir
            .iter()
            .position(|node| matches!(node.kind, HirKind::Closure))
            .unwrap() as u32,
    );
    let boundary = mir.hir[closure.index()]
        .children
        .iter()
        .find(|edge| edge.role == Role::ReturnType)
        .unwrap()
        .node;
    assert!(mir.value_adjustments[closure.index()].is_some());
    let instance = mir
        .generic_instances
        .iter()
        .find(|instance| mir.symbols[instance.symbol.index()].name == "finish" && instance.concrete)
        .unwrap();
    let callable = instance.adjustment(closure).unwrap();
    assert_eq!(callable, instance.signature);
    let signature = &mir.types[callable.index()];
    assert_eq!(signature.constructor, TypeConstructor::Function);
    assert_eq!(
        signature.arguments.last().copied(),
        instance.adjustment(boundary)
    );
    mir.value_adjustments[boundary.index()] = None;
    assert!(
        mir.seal().is_err(),
        "a checked signature without its return check must not seal"
    );
}

#[test]
fn unchecked_identity_and_conversion_evidence_are_separate() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        type Point = struct {x: Int};
        def candidate: Unchecked(Unchecked(Point)) = {x: 42};
        export def checked: Point = candidate;
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    mir.seal().unwrap();
    let TypeState::Known(candidate) = symbol_type(&mir, "candidate") else {
        panic!("candidate");
    };
    let TypeState::Known(checked) = symbol_type(&mir, "checked") else {
        panic!("checked");
    };
    assert_ne!(candidate, checked);
    assert_eq!(
        mir.types[candidate.index()].constructor,
        TypeConstructor::Unchecked
    );
    assert_eq!(mir.types[candidate.index()].arguments, [checked]);
    assert_eq!(mir.value_adjustments.iter().flatten().count(), 1);
    mir.seal().unwrap();
    let image = crate::type_image::TypeImage::from_mir(&mir).unwrap();
    drop(mir);
    assert!(std::ptr::eq(
        image.layout(candidate).unwrap(),
        image.layout(checked).unwrap()
    ));
    for source in [
        "export type Bad = Unchecked(Int);",
        "type Item = struct(Int); export type Bad = Unchecked(Item);",
        "type Item = enum {One}; export type Bad = Unchecked(Item);",
        "type A = struct {x: Int}; type B = struct {x: Int}; def candidate: Unchecked(A) = {x: 1}; export def wrong: B = candidate;",
        "type Wrap(T) = Unchecked(T); export type Bad = Wrap(Int);",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(!mir.diagnostics.is_empty(), "{source}\n{}", mir.dump());
        assert!(mir.seal().is_err());
    }
}

#[test]
fn generic_construction_checks_close_bodies_and_member_discovered_owners() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        def identity: for(T) Fn(T) -> T = fn(value) { value };
        @check(fn(value) { let copied = identity(value.item); Ok(()) })
        type Item(T) = struct { item: T };
        type Envelope(T) = struct { child: Item(T) };
        export def first: TypeOf(Envelope(Int)) = Envelope(Int).type;
        export def second: TypeOf(Envelope(String)) = Envelope(String).type;
    "#,
    )]);
    let hir = mir.hir.as_ptr();
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    mir.seal().unwrap();
    assert_eq!(hir, mir.hir.as_ptr());
    let checks = mir
        .construction_checks
        .iter()
        .filter(|check| check.concrete)
        .collect::<Vec<_>>();
    assert_eq!(checks.len(), 2, "{}", mir.dump());
    for check in checks {
        let instance = &mir.generic_instances[check.instance.unwrap().index()];
        assert!(instance.concrete);
        assert_eq!(instance.ty(check.checker), Some(check.signature));
        assert!(instance.references.iter().any(|(_, reference)| {
            let target = &mir.generic_instances[reference.index()];
            mir.symbols[target.symbol.index()].name == "identity" && target.concrete
        }));
        let input = mir.types[check.signature.index()].arguments[0];
        assert_eq!(mir.types[input.index()].arguments, [check.owner]);
    }
}

#[test]
fn construction_checks_are_separate_closed_contracts_without_execution() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        @check(fn(value) { if value.port > 0 { Ok(()) } else { Err(blame!("positive port", value.port)) } })
        type Endpoint = struct { port: Int };
        @check(fn(value) { if value > 0 { Ok(()) } else { Err(blame!("positive count", value)) } })
        type Count = struct(Int);
        type Event = enum { @check(fn(value) { if value > 0 { Ok(()) } else { Err(blame!("positive payload", value)) } }) Item(Int), Empty };
        @check(fn(value) { fail!("must not execute during static solving") })
        type Deferred = struct { value: Int };
        export def answer: TypeOf(Endpoint) = Endpoint.type;
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    mir.seal().unwrap();
    assert_eq!(mir.construction_checks.len(), 4);
    assert!(mir.properties.iter().all(|property| {
        !mir.construction_checks
            .iter()
            .any(|check| check.owner == property.owner)
    }));
    for check in &mir.construction_checks {
        let signature = &mir.types[check.signature.index()];
        assert_eq!(signature.constructor, TypeConstructor::Function);
        assert_eq!(signature.arguments.len(), 2);
        let result = &mir.types[signature.arguments[1].index()];
        assert_eq!(result.constructor, TypeConstructor::Result);
        assert_eq!(
            mir.types[result.arguments[0].index()].constructor,
            TypeConstructor::Tuple
        );
        assert!(mir.types[result.arguments[0].index()].arguments.is_empty());
        assert_eq!(
            mir.types[result.arguments[1].index()].constructor,
            TypeConstructor::Native(NativeTypeId::BLAME_ERROR)
        );
        let input = &mir.types[signature.arguments[0].index()];
        let TypeConstructor::Nominal(symbol) = mir.types[check.owner.index()].constructor else {
            panic!("owner")
        };
        if ["Endpoint", "Deferred"].contains(&mir.symbols[symbol.index()].name.as_str()) {
            assert_eq!(input.constructor, TypeConstructor::Unchecked);
            assert_eq!(input.arguments, [check.owner]);
        } else {
            assert_eq!(input.constructor, TypeConstructor::Int);
        }
    }
}

#[test]
fn construction_checks_reject_wrong_boundaries_and_signatures() {
    for source in [
        "@check type Item = struct(Int);",
        "@check(fn(x) { Ok(()) }, fn(x) { Ok(()) }) type Item = struct(Int);",
        "@check(fn(x) { Ok(()) }) @check(fn(x) { Ok(()) }) type Item = struct(Int);",
        "@check(fn(x) { Ok(()) }) type Item = enum { One(Int) };",
        "type Item = enum { @check(fn(x) { Ok(()) }) Empty };",
        "type Item = struct { @check(fn(x) { Ok(()) }) value: Int };",
        "@check(fn(x) { 42 }) type Item = struct(Int);",
        "@check(fn(x) { Err(\"wrong error type\") }) type Item = struct(Int);",
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(!mir.diagnostics.is_empty(), "{source}");
        assert!(mir.seal().is_err(), "{source}");
    }
}

#[test]
fn never_returning_provider_preserves_its_declared_nominal_result() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        @property(PropertyTarget.Type) type Tag = struct { value: Int };
        def provider: Fn(Type, Option(Tag)) -> Tag = fn(owner, previous) { fail!("deferred") };
        @provider type Item = struct { value: Int };
        export def answer: TypeOf(Item) = Item.type;
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    mir.seal().unwrap();
    let TypeState::Known(signature) = symbol_type(&mir, "provider") else {
        panic!("provider signature");
    };
    let result = *mir.types[signature.index()].arguments.last().unwrap();
    let TypeConstructor::Nominal(symbol) = mir.types[result.index()].constructor else {
        panic!("declared result lost");
    };
    assert_eq!(mir.symbols[symbol.index()].name, "Tag");
    assert!(
        mir.properties
            .iter()
            .any(|property| property.property == result)
    );
}

#[test]
fn configured_decorators_use_factory_and_provider_signatures() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        @property(PropertyTarget.Type)
        type Label = struct { value: String };
        def make_label: Fn(String) -> Fn(Type, Option(Label)) -> Label = fn(text) {
            fn(owner, previous) { { value: text } }
        };
        @make_label("name")
        type Item = struct { value: Int };
        export def item: Item = { value: 1 };
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    assert!(mir.type_conflicts.is_empty(), "{:?}", mir.type_conflicts);
    assert!(mir.type_unknowns.is_empty(), "{}", mir.dump());
}

#[test]
fn property_presence_proves_signature_bounds_without_running_providers() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        @property(PropertyTarget.Type)
        type Label = struct { text: String };
        def label: Fn(Type, Option(Label)) -> Label = fn(owner, previous) { fail!("must not run") };
        @label @label
        type Item = struct { value: Int };
        native inspect: for(P, T: Property(P)) Fn(TypeOf(T), TypeOf(P)) -> P;
        def read: for(T: Property(Label)) Fn(TypeOf(T)) -> Label = fn(target) { inspect(target, Label.type) };
        export def answer: Label = read(Item.type);
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    assert!(mir.type_unknowns.is_empty(), "{}", mir.dump());
    assert!(
        mir.bound_requirements
            .iter()
            .any(|b| matches!(b.state, BoundState::Assumed(_)))
    );
    assert!(
        mir.bound_requirements
            .iter()
            .any(|b| matches!(b.state, BoundState::Property(_)))
    );
    assert_eq!(
        mir.properties
            .iter()
            .filter(|p| p.providers.len() == 2)
            .count(),
        1
    );
}

#[test]
fn missing_property_bound_is_rejected_with_all_type_slots_known() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        @property(PropertyTarget.Type)
        type Label = struct { text: String };
        native requires: for(T: Property(Label)) Fn(TypeOf(T)) -> Bool;
        export def answer: Bool = requires(Int.type);
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.type_unknowns.is_empty(), "{}", mir.dump());
    assert!(
        mir.bound_requirements
            .iter()
            .any(|b| b.state == BoundState::Rejected)
    );
    assert!(
        mir.diagnostics
            .iter()
            .any(|d| d.message.contains("no static evidence") && !d.labels.is_empty())
    );
}

#[test]
fn trait_implementations_consume_property_evidence_and_lexical_bounds() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        @property(PropertyTarget.Type)
        type Label = struct { text: String };
        def label: Fn(Type, Option(Label)) -> Label = fn(owner, previous) { fail!("not executed") };
        @label type Item = struct { value: Int };
        trait Named { name: Fn(Self) -> String };
        impl(T: Property(Label)) Named for T { name: fn(value) { "named" } };
        def name: for(T: Named) Fn(T) -> String = fn(value) { Named.name(value) };
        def item: Item = { value: 1 };
        export def answer: String = name(item);
    "#,
    )]);
    resolve(&mut mir);
    assert!(
        mir.diagnostics.is_empty(),
        "{:?}\n{}",
        mir.diagnostics,
        mir.dump()
    );
    assert!(mir.type_unknowns.is_empty(), "{}", mir.dump());
    assert!(
        mir.bound_requirements
            .iter()
            .any(|b| matches!(b.state, BoundState::Implementation(_)))
    );
    assert!(
        mir.bound_requirements
            .iter()
            .any(|b| matches!(b.state, BoundState::Assumed(_)))
    );
}

#[test]
fn trait_evidence_rejects_missing_cycles_overlap_and_wrong_member_signatures() {
    for (source, message) in [
        (
            "trait Show { show: Fn(Self) -> String }; export def answer: String = Show.show(1);",
            "no static evidence",
        ),
        (
            "trait Show { show: Fn(Self) -> String }; impl(T: Show) Show for T { show: fn(x) { \"cycle\" } }; export def answer: String = Show.show(1);",
            "no static evidence",
        ),
        (
            "trait Show { show: Fn(Self) -> String }; impl(T) Show for T { show: fn(x) { \"all\" } }; impl Show for Int { show: fn(x) { \"int\" } }; export { Show };",
            "overlapping trait implementations",
        ),
        (
            "trait Show { show: Fn(Self) -> String }; impl Show for Int { show: fn(x) { 42 } }; export { Show };",
            "type mismatch",
        ),
    ] {
        let mut mir = graph(&[("@src/main", source)]);
        resolve(&mut mir);
        assert!(
            mir.diagnostics.iter().any(|d| d.message.contains(message)),
            "{message}: {:?}",
            mir.diagnostics
        );
    }
}

#[test]
fn member_properties_keep_separate_presence_records_and_structural_contexts() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        @property(PropertyTarget.Field) type Mark = struct { value: Int };
        type Ctx = struct { owner: Type, index: Int, name: String, ty: Type };
        def mark: Fn(Ctx, Option(Mark)) -> Mark = fn(ctx, previous) { { value: ctx.index } };
        type Item = struct { @mark first: Int, @mark second: String };
        export def item: Item = { first: 1, second: "ok" };
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    assert!(
        mir.properties
            .iter()
            .any(|p| p.site == PropertySite::Field(0))
    );
    assert!(
        mir.properties
            .iter()
            .any(|p| p.site == PropertySite::Field(1))
    );
}

#[test]
fn exact_impl_wins_over_property_blanket_without_specializing_function_names() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        @property(PropertyTarget.Type) type Tag = struct { value: Int };
        def tag: Fn(Type, Option(Tag)) -> Tag = fn(owner, previous) { { value: 1 } };
        @tag type Item = struct { value: Int };
        trait Label { label: Fn(Self) -> String };
        impl(T: Property(Tag)) Label for T { label: fn(value) { "generic" } };
        impl Label for Item { label: fn(value) { "exact" } };
        impl Label for Int { label: fn(value) { "primitive" } };
        def item: Item = { value: 1 };
        export def answer: String = Label.label(item);
        export def number: String = Label.label(1);
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    for requirement in &mir.bound_requirements {
        let BoundState::Implementation(symbol) = requirement.state else {
            continue;
        };
        assert!(mir.symbol_generics[symbol.index()].is_empty());
    }
}

#[test]
fn structural_impl_wins_over_property_blanket() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        @property(PropertyTarget.Type) type Tag = struct {};
        def tag: Fn(Type, Option(Tag)) -> Tag = fn(owner, previous) { {} };
        trait Label { label: Fn(Self) -> String };
        type Box(T) = struct { value: T };
        impl(T: Label) Label for Box(T) { label: fn(value) { "box" } };
        impl(T: Property(Tag)) Label for T { label: fn(value) { "property" } };
        impl Label for Int { label: fn(value) { "int" } };
        @tag type Item = struct { value: Int };
        def boxed: Box(Int) = { value: 1 };
        def item: Item = { value: 1 };
        export def a: String = Label.label(boxed);
        export def b: String = Label.label(item);
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
}

#[test]
fn optional_property_bounds_close_as_present_or_absent() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        @property(PropertyTarget.Type) type Tag = struct {};
        def tag: Fn(Type, Option(Tag)) -> Tag = fn(owner, previous) { {} };
        @tag type Item = struct {};
        def accepts: for(T: ?Property(Tag)) Fn(TypeOf(T)) -> Bool = fn(target) { True };
        export def present: Bool = accepts(Item.type);
        export def absent: Bool = accepts(Int.type);
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    assert!(
        mir.bound_requirements
            .iter()
            .any(|bound| matches!(bound.state, BoundState::OptionalProperty(_)))
    );
    assert!(
        mir.evidence
            .iter()
            .any(|evidence| evidence.state == BoundState::OptionalAbsent)
    );
}

#[test]
fn codec_text_bridge_is_selected_by_property_and_requires_its_trait_capabilities() {
    let source = r#"
        import "std/codec" as codec;
        import "std/string" as string;
        import "std/fmt" as fmt;
        import "std/value" {Value};
        @string.decode_by_parse
        @string.encode_by_display
        @fmt.display_by("{value}")
        type Textual = struct { value: Int };
        impl string.FromStr for Textual {
            from_str: fn(input) { Ok({value: 1}.ty!(Textual)) },
        };
        @string.decode_by_parse
        @string.encode_by_display
        type Explicit = struct { value: Int };
        impl codec.Decode for Explicit {
            decode: fn(value) { Ok({value: 3}.ty!(Explicit)) },
        };
        impl codec.Encode for Explicit {
            encode: fn(value) { Value.Int(value.value) },
        };
        type Plain = struct { value: Int };
        export def textual: Value = codec.encode({value: 1}.ty!(Textual));
        export def plain: Value = codec.encode({value: 2}.ty!(Plain));
        export def explicit_encoded: Value = codec.encode({value: 3}.ty!(Explicit));
        export def explicit_decoded: Result(Explicit, codec.BlameError) =
            codec.decode@[Explicit](Value.Int(3));
    "#;
    let mut sources = vec![("@src/main", source)];
    sources.extend_from_slice(crate::static_sources::BUILTINS);
    let mut mir = graph(&sources);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());

    assert!(
        mir.evidence
            .iter()
            .any(|evidence| matches!(evidence.state, BoundState::Property(_)))
    );
    assert!(
        mir.evidence
            .iter()
            .any(|evidence| matches!(evidence.state, BoundState::Implementation(_)))
    );

    let missing = r#"
        import "std/codec" as codec;
        import "std/string" as string;
        import "std/value" {Value};
        @string.decode_by_parse
        type Missing = struct { value: Int };
        export def answer: Result(Missing, codec.BlameError) =
            codec.decode@[Missing](Value.String("1"));
    "#;
    let mut sources = vec![("@src/main", missing)];
    sources.extend_from_slice(crate::static_sources::BUILTINS);
    let mut mir = graph(&sources);
    resolve(&mut mir);
    assert!(
        mir.diagnostics.iter().any(|diagnostic| {
            diagnostic.message.contains("no static evidence")
                && diagnostic.message.contains("Decode")
        }),
        "{}",
        mir.dump()
    );
}

#[test]
fn optional_property_requirement_does_not_reject_a_blanket_impl() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        @property(PropertyTarget.Type) type Tag = struct {};
        trait Label { label: Fn(Self) -> String };
        impl(T: ?Property(Tag)) Label for T { label: fn(value) { "fallback" } };
        export def answer: String = Label.label(1);
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    assert!(
        mir.evidence
            .iter()
            .any(|evidence| evidence.state == BoundState::OptionalAbsent)
    );
}

#[test]
fn optional_property_requirement_does_not_specialize_an_impl() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        @property(PropertyTarget.Type) type Tag = struct {};
        trait Label { label: Fn(Self) -> String };
        impl(T: ?Property(Tag)) Label for T { label: fn(value) { "blanket" } };
        impl Label for Int { label: fn(value) { "exact" } };
        export def answer: String = Label.label(1);
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("overlapping trait implementations")
    }));
}

#[test]
fn different_optional_property_bounds_do_not_disambiguate_blanket_impls() {
    let mut mir = graph(&[(
        "@src/main",
        r#"
        @property(PropertyTarget.Type) type Left = struct {};
        @property(PropertyTarget.Type) type Right = struct {};
        trait Label { label: Fn(Self) -> String };
        impl(T: ?Property(Left)) Label for T { label: fn(value) { "left" } };
        impl(T: ?Property(Right)) Label for T { label: fn(value) { "right" } };
        export def answer: String = Label.label(1);
    "#,
    )]);
    resolve(&mut mir);
    assert!(mir.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("overlapping trait implementations")
    }));
}
