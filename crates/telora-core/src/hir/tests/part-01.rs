    #[test]
    fn duplicate_definitions_keep_distinct_ids_and_one_conflict() {
        let program = parse("duplicate.telora", "def a = 1; def a = 2; def a = 3; a").unwrap();
        let hir = HirProgram::resolve(&program, []);
        let HirResolveConflict::DuplicateDefinition { name, definitions } = &hir.conflicts()[0] else { panic!("duplicate conflict"); };
        assert_eq!(name, "a");
        assert_eq!(definitions.len(), 3);
        assert!(definitions.windows(2).all(|ids| ids[0] != ids[1]));
        assert_eq!(hir.definitions().len(), 3);
        for id in definitions { assert_eq!(hir.definition(*id).unwrap().name, "a"); }
        assert!(matches!(hir.references()[0].resolution, HirResolution::Conflicted(id) if id.index() == 0));
    }

    #[test]
    fn nested_definition_shadowing_is_not_a_duplicate() {
        let program = parse("shadow.telora", "def a = 1; def inner = do { def a = 2; a }; a").unwrap();
        let hir = HirProgram::resolve(&program, []);
        assert!(hir.conflicts().is_empty());
        let references = hir.references().iter().filter(|reference| reference.name == "a").collect::<Vec<_>>();
        assert_ne!(references[0].resolution, references[1].resolution);
    }

    #[test]
    fn generic_parameter_references_close_over_their_own_nested_binders() {
        let program = parse("generic-scopes.telora", r#"
            def outer: for(T) Fn(T) -> T = fn(value: T) {
                def inner: for(T) Fn(T) -> T = fn(item: T) { item };
                let copy: T = value;
                copy
            };
            outer
        "#).unwrap();
        let mut parameter_lookups = 0;
        let hir = HirProgram::resolve_with_lookup(&program, &mut |name| {
            if name == "T" { parameter_lookups += 1; }
            HirExternalName { declared: true, member: name == "T" }
        });
        assert_eq!(parameter_lookups, 0, "bound parameters must not query import scopes");
        let outer = hir.definitions().iter().find(|definition| definition.name == "outer").unwrap();
        let inner = hir.definitions().iter().find(|definition| definition.name == "inner").unwrap();
        assert_eq!(outer.type_parameters.len(), 1);
        assert_eq!(inner.type_parameters.len(), 1);
        let outer_id = outer.type_parameters[0].id;
        let inner_id = inner.type_parameters[0].id;
        assert_ne!(outer_id, inner_id);
        let inner_body = hir.expression(inner.value.unwrap()).unwrap().location;
        let mut referenced = HashSet::new();
        for reference in hir.references().iter().filter(|reference| reference.name == "T") {
            let HirResolution::Definition(id) = reference.resolution else { panic!("{reference:?}"); };
            assert_eq!(hir.definition(id).unwrap().kind, HirDefinitionKind::TypeParameter);
            assert_eq!(id, if inner.type_parameters[0].location.start <= reference.location.start && reference.location.end <= inner_body.end {
                inner_id
            } else { outer_id });
            referenced.insert(id);
        }
        assert_eq!(referenced, HashSet::from([outer_id, inner_id]));
    }

    #[test]
    fn construction_checks_are_not_deferred_property_roots() {
        let program = parse(
            "hir.telora",
            "@check(validate) @decorate(config) type Example = struct {x: Int};\
             type Choice = enum { @check(memberCheck) @decorate(memberConfig) Item(Int) };",
        ).unwrap();
        let hir = HirProgram::resolve(&program,
            ["Int", "\0telora_struct", "\0telora_enum", "validate", "decorate", "config", "memberCheck", "memberConfig"].map(String::from));
        assert!(hir.unresolved().next().is_none(), "{:?}", hir.unresolved().collect::<Vec<_>>());
        for name in ["validate", "memberCheck"] {
            let reference = hir.references().iter().find(|reference| reference.name == name).unwrap();
            assert!(!hir.is_property_root(reference.location));
        }
        for name in ["config", "memberConfig"] {
            let reference = hir.references().iter().find(|reference| reference.name == name).unwrap();
            assert!(hir.is_property_root(reference.location));
        }
    }

    #[test]
    fn resolves_slots_shadowing_parameters_patterns_and_externals() {
        let program = parse(
            "hir.telora",
            "decl loop: Fn(Int) -> Int;\
             def loop = fn(n) { if n < 1 { n } else { loop(n - 1) } };\
             let f = fn(x) { let x = x; match (Bool.True, x) { (Bool.True, y) => y, _ => ext } };\
             f(loop(2))",
        )
        .unwrap();
        let hir = HirProgram::resolve(&program, ["\0telora_function_type".into(), "Int".into(), "Bool".into(), "ext".into()]);
        let unresolved = hir
            .unresolved()
            .map(|reference| reference.name.as_str())
            .collect::<Vec<_>>();
        assert!(unresolved.is_empty(), "{unresolved:?}");
        for reference in hir.references() {
            let expected = hir.references().iter().find(|candidate|
                candidate.location == reference.location && candidate.name == reference.name).unwrap();
            assert_eq!(hir.reference_at(reference.location, &reference.name).unwrap().id, expected.id);
        }
        for definition in hir.definitions() {
            let mut expected = Vec::new();
            if let Some(root) = definition.value {
                for expression in hir.expressions() {
                    let Some(reference) = expression.reference.and_then(|id| hir.reference(id)) else { continue; };
                    let HirResolution::Definition(target) = reference.resolution else { continue; };
                    let mut current = Some(expression.id);
                    while let Some(id) = current {
                        if id == root { expected.push(target); break; }
                        current = hir.expression(id).and_then(|expression| expression.parent);
                    }
                }
            }
            expected.sort_unstable();
            expected.dedup();
            assert_eq!(hir.definition_dependencies(definition.id), expected);
        }
        let loop_definition = hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "loop")
            .unwrap();
        assert_eq!(loop_definition.additional_locations.len(), 1);
        assert!(hir.references().iter().any(|reference| {
            reference.name == "loop"
                && reference.resolution == HirResolution::Definition(loop_definition.id)
        }));
        assert!(hir.references().iter().any(|reference| {
            reference.name == "ext" && reference.resolution == HirResolution::External
        }));
        assert!(hir.expressions().len() > hir.references().len());
        assert!(
            hir.expressions()
                .iter()
                .any(|expression| expression.parent.is_some())
        );
    }

    #[test]
    fn resolves_blanket_impl_type_parameters_in_contracts() {
        let program = parse(
            "hir.telora",
            "impl(T: Property(DisplayBy)) Display for T { display: fn(value) { value } };",
        )
        .unwrap();
        let hir = HirProgram::resolve(
            &program,
            ["Property".into(), "DisplayBy".into(), "Display".into()],
        );
        let unresolved = hir
            .unresolved()
            .map(|reference| reference.name.as_str())
            .collect::<Vec<_>>();
        assert!(unresolved.is_empty(), "{unresolved:?}");
    }

    #[test]
    fn retains_type_argument_placeholders_without_references() {
        let source = "native pair: for(A, B) Fn(A, B) -> Tuple([A, B]); pair@[Int, _](1, \"x\")";
        let program = parse("hir.telora", source).unwrap();
        let hir = HirProgram::resolve(
            &program,
            ["for".into(), "Func".into(), "Int".into(), "pair".into()],
        );
        let placeholder = hir
            .expressions()
            .iter()
            .find(|expression| {
                expression.location.range()
                    == (source.find('_').unwrap()..source.find('_').unwrap() + 1)
            })
            .expect("placeholder expression");
        assert!(placeholder.reference.is_none());
        assert!(placeholder.parent.is_some());
    }

    #[test]
    fn interpreter_hir_indexes_only_authored_operand() {
        let program = parse(
            "hir.telora",
            "def lift: for(A) Fn(TypeOf(A)) -> Fn(A, A) -> Bool = interpreter!(eq_i); lift",
        )
        .unwrap();
        let hir = HirProgram::resolve(
            &program,
            ["Func", "TypeOf", "Bool", "eq_i"]
                .into_iter()
                .map(str::to_owned),
        );
        let names = hir
            .references()
            .iter()
            .map(|reference| reference.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"eq_i"));
        assert!(!names.iter().any(|name| name.contains("telora_interpreter")));
        assert!(!names.contains(&"\0telora_pack_dyn"));
    }

    #[test]
    fn fail_hir_indexes_arguments_but_not_internal_names() {
        let program = parse(
            "hir.telora",
            "let data = 1; let message = \"bad\"; fail!(message, data)",
        )
        .unwrap();
        let hir = HirProgram::resolve(&program, std::iter::empty());
        let names = hir
            .references()
            .iter()
            .map(|reference| reference.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"data"));
        assert!(names.contains(&"message"));
        assert!(!names.contains(&"fail"));
    }
