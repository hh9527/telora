    #[test]
    fn selects_nested_tuple_and_tagged_binding_types() {
        let input = pattern(
            PatternKind::Tagged {
                tag: "Some".into(),
                payload: Box::new(pattern(
                    PatternKind::Tuple(vec![binding("name", 2), binding("age", 3)]),
                    1,
                )),
            },
            0,
        );
        let matched = TypeDescriptor::Tagged {
            tag: Atom::named("Some"),
            payload: Box::new(TypeDescriptor::Tuple(vec![
                TypeDescriptor::String,
                TypeDescriptor::Int,
            ])),
        };
        let analysis = analyze_pattern(&input, &matched);
        assert_eq!(analysis.compatibility, PatternCompatibility::Compatible);
        assert!(analysis.irrefutable);
        assert_eq!(analysis.bindings[0].ty, TypeDescriptor::String);
        assert_eq!(analysis.bindings[1].ty, TypeDescriptor::Int);
    }

    #[test]
    fn unknown_tuple_shape_keeps_bindings_conservative() {
        let input = pattern(
            PatternKind::Tuple(vec![binding("left", 1), binding("right", 2)]),
            0,
        );
        let analysis = analyze_pattern(&input, &TypeDescriptor::Any);
        assert_eq!(analysis.compatibility, PatternCompatibility::Unknown);
        assert!(!analysis.irrefutable);
        assert!(
            analysis
                .bindings
                .iter()
                .all(|binding| binding.ty == TypeDescriptor::Any)
        );
    }

    #[test]
    fn enum_payload_coverage_requires_irrefutable_payload() {
        let matched = TypeDescriptor::Enum(BTreeMap::from([
            ("None".into(), None),
            ("Some".into(), Some(Box::new(TypeDescriptor::Int))),
        ]));
        let binding_payload = pattern(
            PatternKind::Tagged {
                tag: "Some".into(),
                payload: Box::new(binding("value", 1)),
            },
            0,
        );
        let literal_payload = pattern(
            PatternKind::Tagged {
                tag: "Some".into(),
                payload: Box::new(pattern(PatternKind::Int(1), 3)),
            },
            2,
        );
        assert!(
            analyze_pattern(&binding_payload, &matched)
                .covered_variants
                .contains("Some")
        );
        assert!(
            !analyze_pattern(&literal_payload, &matched)
                .covered_variants
                .contains("Some")
        );
    }

    #[test]
    fn duplicate_bindings_keep_the_first_fact() {
        let input = pattern(
            PatternKind::Tuple(vec![binding("item", 1), binding("item", 2)]),
            0,
        );
        let matched = TypeDescriptor::Tuple(vec![TypeDescriptor::Int, TypeDescriptor::String]);
        let analysis = analyze_pattern(&input, &matched);
        assert_eq!(analysis.bindings.len(), 1);
        assert_eq!(analysis.bindings[0].ty, TypeDescriptor::Int);
        assert_eq!(analysis.bindings[0].location, location(1));
        assert_eq!(analysis.duplicates.len(), 1);
        assert_eq!(analysis.duplicates[0].name, "item");
        assert_eq!(analysis.duplicates[0].location, location(2));
    }
