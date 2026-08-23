    #[test]
    fn resolves_slots_shadowing_parameters_patterns_and_externals() {
        let program = parse(
            "hir.telora",
            "decl loop: Fn(Int) -> Int;\
             def loop = fn(n) { if n < 1 { n } else { loop(n - 1) } };\
             let f = fn(x) { let x = x; match ('Ok, x) { ('Ok, y) => y, _ => ext } };\
             f(loop(2))",
        )
        .unwrap();
        let hir = HirProgram::resolve(&program, ["Func".into(), "Int".into(), "ext".into()]);
        let unresolved = hir
            .unresolved()
            .map(|reference| reference.name.as_str())
            .collect::<Vec<_>>();
        assert!(unresolved.is_empty(), "{unresolved:?}");
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
