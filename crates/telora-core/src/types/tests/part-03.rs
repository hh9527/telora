    #[test]
    fn parameterized_type_family_publishes_a_precise_constructor_scheme() {
        let analysis = analyze_source(
            "family.telora",
            "type Box(A) = struct {value: A};\
             type IntBox = Box(Int);\
             def wrap: for(A) Fn(A) -> Box(A) = fn(value) { {value} };\
             wrap(1)",
        )
        .unwrap();
        let box_definition = analysis
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "Box")
            .expect("Box definition");
        assert_eq!(
            analysis.definition_schemes[&box_definition.id].display_name(),
            "for(A) Fn(TypeOf(A)) -> TypeOf(Box)"
        );
        assert_eq!(analysis.display(analysis.declared_types["IntBox"]), "Box");
        assert_eq!(analysis.display(analysis.result_type), "Box");
    }

    #[test]
    fn declared_family_applications_use_head_and_argument_identity() {
        let analysis = analyze_source(
            "family-identities.telora",
            "type Box(A) = struct {value: A};\
             type Other(A) = struct {value: A};\
             type Phantom(A) = struct {value: Int};\
             type Maybe(A) = enum {'None, 'Some(A)};\
             type IntBox = Box(Int);\
             type IntBoxAlias = Box(Int);\
             type Text = Box(String);\
             type EqualShape = Other(Int);\
             type PhantomInt = Phantom(Int);\
             type PhantomText = Phantom(String);\
             type Nested = Box(Maybe(Int));\
             type Optional = Maybe(Int);\
             0",
        )
        .unwrap();
        let declared_id = |name: &str| {
            let TypeNode::Declared { id, .. } = analysis.types.node(analysis.declared_types[name])
            else {
                panic!("{name} must be a declared family application")
            };
            id
        };

        assert_eq!(declared_id("IntBox"), declared_id("IntBoxAlias"));
        assert_ne!(declared_id("IntBox"), declared_id("Text"));
        assert_ne!(declared_id("IntBox"), declared_id("EqualShape"));
        assert_ne!(declared_id("PhantomInt"), declared_id("PhantomText"));
        assert_eq!(declared_id("Nested").arguments().len(), 1);
        assert_eq!(declared_id("Optional").arguments().len(), 1);
        assert_ne!(declared_id("Nested"), declared_id("Optional"));

        let error = analyze_source(
            "phantom-mismatch.telora",
            "type Phantom(A) = struct {value: Int};\
             let int_value: Phantom(Int) = {value: 1};\
             let text_value: Phantom(String) = int_value;\
             text_value",
        )
        .unwrap_err();
        assert!(
            error.message.contains("not assignable"),
            "{}",
            error.message
        );
    }

    #[test]
    fn generic_calls_infer_parameters_through_struct_family_arguments() {
        let analysis = analyze_source(
            "family-argument.telora",
            "type Box(Content) = struct {value: Content};\
             def unbox: for(Content) Fn(Box(Content)) -> Content = fn(boxed) { boxed.value };\
             let boxed: Box(Int) = {value: 7};\
             let value: Int = unbox(boxed);\
             let inferred = unbox(boxed);\
             (value, inferred)",
        )
        .unwrap();
        let unbox = analysis
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "unbox")
            .expect("unbox definition");
        assert_eq!(
            analysis.definition_schemes[&unbox.id].display_name(),
            "for(Content) Fn(Box) -> Content"
        );
        assert_eq!(analysis.display(analysis.result_type), "(Int, Int)");
    }

    #[test]
    fn generic_call_context_widens_singleton_fields_in_anonymous_records() {
        let prelude = "type Node = enum {'A, 'B};\
             type Requirement = struct {target: Node};\
             def target_of: Fn(Requirement) -> Node = fn(req) { req.target };\
             def use: for(Req) Fn(Array(Req), Fn(Req) -> Node) -> Node =\
                 fn(requirements, selector) { selector(requirements[0]) };";
        for records in ["[{target: 'B}]", "[{target: 'A}, {target: 'B}]"] {
            let analysis = analyze_source(
                "generic-record-widening.telora",
                &format!("{prelude} use({records}, target_of)"),
            )
            .unwrap();
            assert_eq!(analysis.display(analysis.result_type), "Node");
        }

        let conflict = analyze_source(
            "generic-record-conflict.telora",
            &format!("{prelude} use([{{target: 'Foreign}}], target_of)"),
        )
        .unwrap_err();
        assert!(
            conflict
                .message
                .contains("variant 'Foreign is not part of Node"),
            "{}",
            conflict.message
        );

        let conflicting_enum = analyze_source(
            "generic-record-enum-conflict.telora",
            &format!(
                "{prelude} type Foreign = enum {{'Foreign}};\
                 let foreign: Foreign = 'Foreign;\
                 use([{{target: foreign}}], target_of)"
            ),
        )
        .unwrap_err();
        assert!(
            conflicting_enum.message.contains("Foreign")
                && conflicting_enum.message.contains("Node"),
            "{}",
            conflicting_enum.message
        );
    }

    #[test]
    fn generic_struct_families_construct_nested_array_tuple_fields() {
        let analysis = analyze_source(
            "nested-family-field.telora",
            "type Box(A) = struct {value: Array(Tuple([A, Int]))};\
             def make: for(A) Fn(Array(Tuple([A, Int]))) -> Box(A) =\
                 fn(value) { {value} };\
             make([(1, 2)]).value",
        )
        .unwrap();
        assert_eq!(analysis.display(analysis.result_type), "Array<(Int, Int)>");

        for source in [
            "type Box(A) = struct {value: Array(Tuple([Int, A]))};\
             def make: for(A) Fn(Array(Tuple([Int, A]))) -> Box(A) =\
                 fn(value) { {value} };\
             make([(1, \"two\")]).value",
            "type Box(A) = struct {value: Array(Tuple([Int, A, String]))};\
             def make: for(A) Fn(Array(Tuple([Int, A, String]))) -> Box(A) =\
                 fn(value) { {value} };\
             make([(1, 2.0, \"three\")]).value",
        ] {
            analyze_source("nested-family-position.telora", source).unwrap();
        }

        let incompatible = analyze_source(
            "incompatible-nested-family-field.telora",
            "type Box(A) = struct {value: Array(Tuple([A, Int]))};\
             def make: for(A) Fn(Array(Tuple([A, String]))) -> Box(A) =\
                 fn(value) { {value} };\
             make([(1, \"wrong\")])",
        )
        .unwrap_err();
        assert!(
            incompatible.message.contains("String") && incompatible.message.contains("Int"),
            "{}",
            incompatible.message
        );

        let shadowed = analyze_source(
            "shadowed-tuple.telora",
            "let Tuple = fn(value) { value }; Tuple([1, 2])",
        )
        .unwrap();
        assert_eq!(shadowed.display(shadowed.result_type), "Array<Int>");
    }

    #[test]
    fn explicit_array_context_checks_anonymous_concrete_family_catalogs() {
        let definitions = "type Id = enum {'First, 'Second};\
             type Mode = enum {'Direct, 'Derived};\
             type Capability(IdType, Input, Output) = struct {\
                 id: IdType,\
                 mode: Mode,\
                 lower: Fn(Input) -> Option(Output),\
                 dependencies: Array(IdType),\
             };\
             type Concrete = Capability(Id, Int, String);";
        let first = "{\
            id: 'First,\
            mode: 'Direct,\
            lower: fn(value) { 'Some(`value=\\{value}`) },\
            dependencies: [],\
        }";
        let second = "{\
            id: 'Second,\
            mode: 'Derived,\
            lower: fn(value) { if value == 0 { 'None } else { 'Some(`value=\\{value}`) } },\
            dependencies: ['First],\
        }";

        for (name, elements) in [
            ("forward", format!("{first}, {second}")),
            ("reverse", format!("{second}, {first}")),
        ] {
            let source =
                format!("{definitions} let catalog: Array(Concrete) = [{elements}]; catalog");
            let analysis = analyze_source(&format!("catalog-{name}.telora"), &source).unwrap();
            assert_eq!(analysis.display(analysis.result_type), "Array<Capability>");
        }

        let incompatible = analyze_source(
            "catalog-incompatible.telora",
            &format!(
                "{definitions} let catalog: Array(Concrete) = [{first}, {{\
                    id: 'Second,\
                    mode: 'Derived,\
                    lower: fn(value) {{ 'Some(value) }},\
                    dependencies: ['First],\
                }}]; catalog"
            ),
        )
        .unwrap_err();
        assert!(
            incompatible.message.contains("String") && incompatible.message.contains("Int"),
            "{}",
            incompatible.message
        );
    }

    #[test]
    fn parameterized_type_families_compose_symbolic_templates() {
        let analysis = analyze_source(
            "families.telora",
            "type Box(A) = struct {value: A};\
             type Envelope(Payload, Error) = struct {\
                 payload: Option(Box(Payload)),\
                 error: Option(Error),\
             };\
             type Response = Envelope(String, Int);\
             Response",
        )
        .unwrap();
        let response = analysis.declared_types["Response"];
        assert_eq!(analysis.display(response), "Envelope");
        let TypeNode::Declared { body, .. } = analysis.types.node(response) else {
            panic!("Response must retain its Envelope owner")
        };
        let body = analysis.types.display(*body);
        assert!(body.contains("Box"), "{body}");
        assert!(body.contains("Int"), "{body}");
        assert!(!body.contains("Any"), "{body}");
    }

    #[test]
    fn parameterized_type_families_evaluate_in_dependency_order() {
        let analysis = analyze_source(
            "forward-family.telora",
            "type Outer(A) = Inner(A);\
             type Inner(A) = Array(A);\
             type Output = Outer(String);\
             Output",
        )
        .unwrap();
        assert_eq!(
            analysis.display(analysis.declared_types["Output"]),
            "Array<String>"
        );
    }

    #[test]
    fn parameterized_type_families_capture_acyclic_local_concrete_types() {
        for (name, source) in [
            (
                "earlier-concrete.telora",
                "type Id = Int;\
                 type Pair(A) = Tuple([Id, A]);\
                 type Output = Pair(String);\
                 Output",
            ),
            (
                "later-concrete.telora",
                "type Pair(A) = Tuple([Id, A]);\
                 type Id = Int;\
                 type Output = Pair(String);\
                 Output",
            ),
            (
                "concrete-family-chain.telora",
                "type Outer(A) = Tuple([Local, A]);\
                 type Local = Inner(String);\
                 type Inner(A) = Tuple([A, Int]);\
                 type Output = Outer(Float);\
                 Output",
            ),
        ] {
            let analysis = analyze_source(name, source).unwrap();
            let expected = if name == "concrete-family-chain.telora" {
                "((String, Int), Float)"
            } else {
                "(Int, String)"
            };
            assert_eq!(
                analysis.display(analysis.declared_types["Output"]),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn concrete_types_schedule_family_applications_with_later_concrete_arguments() {
        let analysis = analyze_source(
            "decorated-concrete-family-chain.telora",
            "type Requirement(E) = struct {target: E, reason: String};\
             type Output = struct {requirements: Array(Requirement(Entity))};\
             type Entity = enum {'Order};\
             Output",
        )
        .unwrap();
        assert_eq!(
            analysis.display(analysis.declared_types["Output"]),
            "Output"
        );
    }

    #[test]
    fn concrete_family_dependency_scheduling_is_source_order_independent_and_transitive() {
        let mut outputs = Vec::new();
        for (name, source) in [
            (
                "earlier-concrete-argument.telora",
                "type Entity = enum {'Order};\
                 type Requirement(E) = struct {target: E, reason: String};\
                 type Output = struct {requirements: Array(Requirement(Entity))}; Output",
            ),
            (
                "multilevel-concrete-family-chain.telora",
                "type Requirement(E) = struct {target: E, reason: String};\
                 type Requirements(E) = Array(Requirement(E));\
                 type Output = struct {requirements: Requirements(Entity)};\
                 type Entity = enum {'Order}; Output",
            ),
        ] {
            let analysis = analyze_source(name, source).unwrap();
            outputs.push((name, analysis.display(analysis.declared_types["Output"])));
        }
        assert!(outputs.iter().all(|(_, output)| output == &outputs[0].1));
    }

    #[test]
    fn type_aliases_preserve_recursive_concrete_family_arguments() {
        let analysis = analyze_source(
            "recursive-family-alias.telora",
            "type Box(A) = struct {value: A};\
             type Branch = struct {children: Array(Tree)};\
             type Tree = enum {'Leaf(Int), 'Branch(Branch)};\
             type TreeBox = Box(Tree);\
             def identity: Fn(TreeBox) -> TreeBox = fn(value) { value };\
             identity({value: 'Leaf(1)})",
        )
        .unwrap();
        let alias = analysis.declared_types["TreeBox"];
        assert_eq!(analysis.display(alias), "Box");
        let TypeNode::Declared { body, .. } = analysis.types.node(alias) else {
            panic!("a family application must retain its declared owner")
        };
        let body = analysis.types.display(*body);
        assert!(body.contains("Tree"), "{body}");
        assert!(!body.contains("Any"), "{body}");
        assert!(!analysis.display(analysis.result_type).contains("Any"));
    }

    #[test]
    fn parameterized_type_family_diagnostics_preserve_bounded_failures() {
        let duplicate = analyze_source(
            "duplicate-family.telora",
            "type Pair(A, A) = Tuple([A, A]); 0",
        )
        .unwrap_err();
        assert!(duplicate.message.contains("duplicate type parameter \"A\""));

        let arity = analyze_source(
            "arity-family.telora",
            "type Box(A) = Array(A); type Broken = Box(Int, String); 0",
        )
        .unwrap_err();
        assert!(
            arity.message.contains("expected 1 arguments, got 2"),
            "{}",
            arity.message
        );

        let invalid = analyze_source("invalid-family.telora", "type Broken(A) = 1; 0").unwrap_err();
        assert!(invalid.message.contains("produced invalid metadata"));

        let direct =
            analyze_source("recursive-family.telora", "type Loop(A) = Loop(A); 0").unwrap_err();
        assert!(direct.message.contains("recursive type alias component"));

        let mutual = analyze_source(
            "mutual-family.telora",
            "type Left(A) = Right(A); type Right(A) = Left(A); 0",
        )
        .unwrap_err();
        assert!(mutual.message.contains("recursive type alias component"));

        let mixed = analyze_source(
            "mixed-recursive-family.telora",
            "type Family(A) = Tuple([Concrete, A]);\
             type Concrete = Family(Int);\
             0",
        )
        .unwrap_err();
        assert!(
            mixed.message.contains("recursive type alias component")
                && mixed.message.contains("Family")
                && mixed.message.contains("Concrete"),
            "{}",
            mixed.message
        );
        let diagnostic = mixed.diagnostic.expect("mixed cycle diagnostic");
        assert_eq!(diagnostic.labels.len(), 2);
    }

    #[test]
    fn productive_recursive_type_family_preserves_its_bound_leaf() {
        let analysis = analyze_source(
            "recursive-expr-family.telora",
            "type Expr(A) = enum {'Leaf(A), 'Call(Array(Expr(A)))};\
             type IntExpr = Expr(Int);\
             type SameIntExpr = Expr(Int);\
             type StringExpr = Expr(String);\
             def identity: Fn(IntExpr) -> IntExpr = fn(value) { value };\
             identity('Call(['Leaf(1)]))",
        )
        .unwrap();
        let expression_family = analysis
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "Expr")
            .unwrap();
        let scheme = analysis.definition_schemes[&expression_family.id].display_name();
        assert_eq!(scheme, "for(A) Fn(TypeOf(A)) -> TypeOf(Expr)");
        assert!(!scheme.contains("Any"));
        assert_eq!(analysis.display(analysis.declared_types["IntExpr"]), "Expr");
        let nominal_id = |name| match analysis.types.node(analysis.declared_types[name]) {
            TypeNode::Declared { id, .. } => id.clone(),
            node => panic!("{name} is not nominal: {node:?}"),
        };
        assert_eq!(nominal_id("IntExpr"), nominal_id("SameIntExpr"));
        assert_ne!(nominal_id("IntExpr"), nominal_id("StringExpr"));
        assert!(!analysis.display(analysis.result_type).contains("Any"));
    }

    #[test]
    fn recursive_type_family_rejects_changed_arguments() {
        for (name, source) in [
            (
                "transformed",
                "type Grow(A) = struct {next: Grow(Array(A))}; 0",
            ),
            (
                "reordered",
                "type Swap(A, B) = struct {next: Swap(B, A)}; 0",
            ),
        ] {
            let error = analyze_source(&format!("recursive-{name}.telora"), source).unwrap_err();
            assert!(
                error
                    .message
                    .contains("must use its bound parameters unchanged"),
                "{}",
                error.message
            );
        }
    }

    #[test]
    fn type_validation_uses_the_authoritative_metadata_decoder() {
        let valid =
            crate::compile_source("valid-type.telora", "validate(Type, Array(Int))").unwrap();
        assert!(
            valid
                .execute_with_quota(&mut Vm::new(), Quota::with_fuel(100_000))
                .unwrap()
                .to_string()
                .starts_with("'Ok(")
        );

        let invalid = crate::compile_source(
            "invalid-type.telora",
            "validate(Type, {kind: 'Array, item: 1})",
        )
        .unwrap();
        let output = invalid
            .execute_with_quota(&mut Vm::new(), Quota::with_fuel(100_000))
            .unwrap()
            .to_string();
        assert!(output.starts_with("'Err("), "{output}");
        assert!(output.contains("value.item must be a Dict"), "{output}");
    }

    #[test]
    fn ordinary_closure_computes_type_metadata() {
        let analysis = analyze_source(
            "test",
            "def Optional = fn(item) { union('None, [Atom('None), Tagged('Some, item)]) };\
             type MaybeInt = Optional(Int);\
             let value: MaybeInt = 'Some(42);\
             value",
        )
        .unwrap();
        let maybe = analysis.declared_types.get("MaybeInt").unwrap();
        assert!(
            matches!(analysis.types.node(*maybe), TypeNode::Union(variants) if variants.len() == 2)
        );
    }

    #[test]
    fn reports_structural_annotation_mismatch() {
        let error = analyze_source(
            "test",
            "type User = struct {name: String, age: Int};\
             let user: User = {name: \"Ada\", age: \"old\"};\
             user",
        )
        .unwrap_err();
        assert!(
            error.message.contains("String") && error.message.contains("Int"),
            "{}",
            error.message
        );
    }

    #[test]
    fn checks_interpolation_inside_nested_binding_annotations() {
        let error = analyze_source("test", r#"let outer = { let x: `\{[1]}` = "x"; x }; outer"#)
            .unwrap_err();
        assert!(error.message.contains("interpolation"), "{}", error.message);
    }

    #[test]
    fn records_a_type_fact_for_every_resolved_hir_expression() {
        let analysis = analyze_source(
            "facts.telora",
            "let values = [1, 2]; let first = fn(x) { let y = x; y }; first(values)",
        )
        .unwrap();
        assert_eq!(
            analysis.expression_types.len(),
            analysis.hir.expressions().len()
        );
        assert!(
            analysis
                .expression_types
                .values()
                .any(|ty| matches!(analysis.types.node(*ty), TypeNode::Int))
        );
        assert!(
            analysis
                .expression_types
                .values()
                .any(|ty| matches!(analysis.types.node(*ty), TypeNode::Array(_)))
        );
        assert!(
            analysis
                .expression_types
                .values()
                .any(|ty| matches!(analysis.types.node(*ty), TypeNode::Function { .. }))
        );
    }

    #[test]
    fn struct_patterns_bind_field_types_and_reject_unknown_fields() {
        let analysis = analyze_source(
            "pattern.telora",
            "type User = struct {name: String, age: Int};\
             let user: User = {name: \"Ada\", age: 36};\
             match user { {age} => age + 1 }",
        )
        .unwrap();
        assert_eq!(analysis.display(analysis.result_type), "Int");
        let age = analysis
            .hir
            .definitions()
            .iter()
            .find(|definition| {
                definition.kind == HirDefinitionKind::Pattern && definition.name == "age"
            })
            .unwrap();
        assert_eq!(analysis.display(analysis.definition_types[&age.id]), "Int");

        let error = analyze_source(
            "pattern.telora",
            "type User = struct {name: String};\
             let user: User = {name: \"Ada\"};\
             match user { {age} => age }",
        )
        .unwrap_err();
        assert!(error.to_string().contains("Struct has no field \"age\""));

        let wrong_shape =
            analyze_source("pattern.telora", "match 1 { {} => 0, _ => 1 }").unwrap_err();
        assert!(
            wrong_shape
                .to_string()
                .contains("Struct pattern cannot match Int")
        );

        let duplicate = analyze_source(
            "pattern.telora",
            "let user = {name: \"Ada\"}; match user { {name, name} => name }",
        )
        .unwrap_err();
        assert!(
            duplicate
                .to_string()
                .contains("duplicate Struct pattern field \"name\""),
            "{duplicate}"
        );
    }

    #[test]
    fn closed_enum_matches_require_conservative_whole_variant_coverage() {
        let complete = analyze_source(
            "match.telora",
            "let option: Option(Int) = 'Some(1);\
             match option { 'None => 0, 'Some(value) => value }",
        )
        .unwrap();
        assert_eq!(complete.display(complete.result_type), "Int");

        let missing = analyze_source(
            "match.telora",
            "let option: Option(Int) = 'Some(1); match option { 'Some(value) => value }",
        )
        .unwrap_err();
        assert!(missing.to_string().contains("missing 'None"), "{missing}");

        let refutable = analyze_source(
            "match.telora",
            "let option: Option(Int) = 'Some(1);\
             match option { 'None => 0, 'Some(1) => 1 }",
        )
        .unwrap_err();
        assert!(
            refutable.to_string().contains("missing 'Some(_)"),
            "{refutable}"
        );

        let catch_all = analyze_source(
            "match.telora",
            "let option: Option(Int) = 'Some(1); match option { _ => 0 }",
        );
        assert!(catch_all.is_ok());

        let dynamic = analyze_source(
            "match.telora",
            "let inspect: Fn(Any) -> Int = fn(value) { match value { 'None => 0 } }; inspect",
        );
        assert!(dynamic.is_ok());
    }

    #[test]
    fn redundant_match_arms_require_certain_prior_coverage() {
        let incompatible =
            analyze_source("match.telora", "match (1, \"x\") { (left, 2) => left }").unwrap_err();
        assert!(
            incompatible
                .to_string()
                .contains("pattern cannot match (Int, String)"),
            "{incompatible}"
        );

        let after_catch_all = analyze_source(
            "match.telora",
            "let option: Option(Int) = 'None; match option { _ => 0, 'None => 1 }",
        )
        .unwrap_err();
        assert!(
            after_catch_all
                .to_string()
                .contains("prior arms cover every value"),
            "{after_catch_all}"
        );

        let repeated = analyze_source(
            "match.telora",
            "let option: Option(Int) = 'None;\
             match option { 'None => 0, 'None => 1, 'Some(_) => 2 }",
        )
        .unwrap_err();
        assert!(repeated.to_string().contains("cover 'None"), "{repeated}");

        let covered_payload = analyze_source(
            "match.telora",
            "let option: Option(Int) = 'Some(1);\
             match option { 'Some(_) => 0, 'Some(1) => 1, 'None => 2 }",
        )
        .unwrap_err();
        assert!(
            covered_payload.to_string().contains("cover 'Some"),
            "{covered_payload}"
        );

        let distinct_partial = analyze_source(
            "match.telora",
            "let option: Option(Int) = 'Some(1);\
             match option { 'None => 0, 'Some(1) => 1, 'Some(2) => 2, 'Some(_) => 3 }",
        );
        assert!(distinct_partial.is_ok(), "{distinct_partial:?}");

        let complete_then_catch_all = analyze_source(
            "match.telora",
            "let option: Option(Int) = 'None;\
             match option { 'None => 0, 'Some(_) => 1, _ => 2 }",
        )
        .unwrap_err();
        assert!(
            complete_then_catch_all
                .to_string()
                .contains("prior arms cover every value"),
            "{complete_then_catch_all}"
        );

        let struct_then_arm = analyze_source(
            "match.telora",
            "let user = {name: \"Ada\"}; match user { {name} => name, _ => \"none\" }",
        )
        .unwrap_err();
        assert!(
            struct_then_arm
                .to_string()
                .contains("prior arms cover every value"),
            "{struct_then_arm}"
        );
    }

    #[test]
    fn destructuring_let_requires_irrefutable_known_shapes() {
        let valid = analyze_source(
            "let.telora",
            "{ let (count, {name}) = (1, {name: \"Ada\"}); (count, name) }",
        )
        .unwrap();
        assert_eq!(valid.display(valid.result_type), "(Int, String)");
        let name = valid
            .hir
            .definitions()
            .iter()
            .find(|definition| {
                definition.kind == HirDefinitionKind::Pattern && definition.name == "name"
            })
            .unwrap();
        assert_eq!(valid.display(valid.definition_types[&name.id]), "String");

        let wrong_arity =
            analyze_source("let.telora", "{ let (left, right) = (1,); left }").unwrap_err();
        assert!(
            wrong_arity
                .to_string()
                .contains("refutable let pattern for (Int)"),
            "{wrong_arity}"
        );

        let dynamic = analyze_source(
            "let.telora",
            "let pair: Any = (1, 2); { let (left, right) = pair; left }",
        )
        .unwrap_err();
        assert!(
            dynamic
                .to_string()
                .contains("refutable let pattern for Any"),
            "{dynamic}"
        );

        let nested = analyze_source(
            "let.telora",
            "let option: Option(Int) = 'Some(1);\
             { let (first, 'Some(value)) = (0, option); value }",
        )
        .unwrap_err();
        assert!(
            nested
                .to_string()
                .contains("refutable let pattern for (Int, enum"),
            "{nested}"
        );
    }

    #[test]
    fn partial_type_evaluation_continues_independent_and_transitive_work() {
        let partial = analyze_partial_types(
            "partial.telora",
            "type A = broken(Int);\
             type B = String;\
             type C = Array(B);\
             type D = Array(A);\
             0",
            Quota::with_fuel(100),
        );
        let definition = |name: &str| {
            partial
                .hir
                .definitions()
                .iter()
                .find(|definition| definition.name == name)
                .unwrap()
                .id
        };
        let a = definition("A");
        let b = definition("B");
        let c = definition("C");
        let d = definition("D");
        assert!(matches!(
            partial.definition_facts[&a].state,
            FactState::Incomputable(IncomputableReason::UnsupportedOperation)
        ));
        assert_eq!(partial.definition_facts[&b].state, FactState::Known);
        assert_eq!(partial.definition_facts[&c].state, FactState::Known);
        assert_eq!(
            partial
                .types
                .display(partial.definition_facts[&c].value.unwrap()),
            "Array<String>"
        );
        assert_eq!(
            partial.definition_facts[&d].state,
            FactState::Unknown(UnknownReason::BlockedBy(FactIdentity::HirDefinition(a)))
        );
        assert!(partial.definition_facts[&d].diagnostics.is_empty());
        assert_eq!(partial.diagnostics.len(), 1);
        let c_node = partial
            .dependencies
            .nodes
            .iter()
            .find(|node| node.definition == c)
            .unwrap();
        assert_eq!(c_node.dependencies, vec![b]);
    }

    #[test]
    fn partial_type_evaluation_resolves_local_concrete_family_dependencies() {
        let partial = analyze_partial_types(
            "partial-family.telora",
            "type Result(A) = Tuple([Outcome, A]);\
             type Outcome = Int;\
             type Output = Result(String);\
             type Independent = Float;\
             0",
            Quota::with_fuel(100),
        );
        let definition = |name: &str| {
            partial
                .hir
                .definitions()
                .iter()
                .find(|definition| definition.name == name)
                .unwrap()
                .id
        };
        for name in ["Result", "Outcome", "Output", "Independent"] {
            assert_eq!(
                partial.definition_facts[&definition(name)].state,
                FactState::Known,
                "{name}"
            );
        }
        assert_eq!(
            partial.types.display(
                partial.definition_facts[&definition("Output")]
                    .value
                    .unwrap()
            ),
            "(Int, String)"
        );
        assert_eq!(partial.diagnostics, Vec::<Diagnostic>::new());
    }

    #[test]
    fn partial_type_evaluation_shares_one_fuel_account() {
        let partial = analyze_partial_types(
            "fuel.telora",
            "type A = Array(Int); type B = Array(Int); 0",
            Quota::with_fuel(1),
        );
        let facts = partial
            .hir
            .definitions()
            .iter()
            .filter(|definition| definition.kind == HirDefinitionKind::Type)
            .map(|definition| &partial.definition_facts[&definition.id])
            .collect::<Vec<_>>();
        assert_eq!(facts[0].state, FactState::Known);
        assert_eq!(
            facts[1].state,
            FactState::Incomputable(IncomputableReason::QuotaExceeded)
        );
    }

    #[test]
    fn partial_type_evaluation_seals_decorated_recursive_components() {
        let partial = analyze_partial_types(
            "recursive.telora",
            "type Node = struct {children: Array(Node)}; 0",
            Quota::with_fuel(100),
        );
        let node = partial
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "Node")
            .unwrap();
        assert_eq!(partial.definition_facts[&node.id].state, FactState::Known);
        assert_eq!(partial.dependencies.nodes[0].dependencies, vec![node.id]);
        assert!(partial.diagnostics.is_empty());
        assert_eq!(
            partial
                .types
                .display(partial.definition_facts[&node.id].value.unwrap()),
            "{children: Array<Node>}"
        );
    }

    #[test]
    fn partial_type_evaluation_seals_multi_node_expr_components_and_dependents() {
        let partial = analyze_partial_types(
            "recursive-expr.telora",
            "type CallNode = struct {args: Array(Expr)};\
             type BinNode = struct {left: Expr, right: Expr};\
             type Expr = enum {'Literal(Int), 'Call(CallNode), 'Bin(BinNode)};\
             type Plan(A) = struct {root: Expr, value: A};\
             def render: Fn(Expr) -> String = fn(expr) { \"ok\" };\
             def transform: for(A) Fn(Plan(A)) -> String = fn(plan) { render(plan.root) };\
             def duplicate: Fn(Array(Expr)) -> Array(Expr) = fn(items) { items };\
             0",
            Quota::with_fuel(1_000),
        );
        let definition = |name: &str| {
            partial
                .hir
                .definitions()
                .iter()
                .find(|definition| definition.name == name)
                .unwrap()
                .id
        };
        for name in ["CallNode", "BinNode", "Expr", "Plan"] {
            assert_eq!(
                partial.definition_facts[&definition(name)].state,
                FactState::Known,
                "{name}"
            );
        }
        assert!(partial.diagnostics.is_empty());
        let plan = partial
            .definition_schemes
            .get(&definition("Plan"))
            .expect("dependent family keeps its scheme");
        assert_eq!(plan.display_name(), "for(A) Fn(TypeOf(A)) -> TypeOf(Plan)");
        assert!(
            !plan.display_name().contains("Any"),
            "{}",
            plan.display_name()
        );
    }

    #[test]
    fn partial_type_evaluation_rejects_recursive_aliases() {
        for (name, source) in [("alias", "type Left = Right; type Right = Left; 0")] {
            let partial = analyze_partial_types(
                &format!("recursive-{name}.telora"),
                source,
                Quota::with_fuel(100),
            );
            assert!(partial.definition_facts.values().all(|fact| {
                fact.state == FactState::Incomputable(IncomputableReason::CyclicEvaluation)
            }));
            assert!(partial.diagnostics.iter().all(|diagnostic| {
                diagnostic.message.contains("cannot be partially evaluated")
            }));
        }
    }

    #[test]
    fn partial_type_evaluation_accepts_productive_recursive_families() {
        let partial = analyze_partial_types(
            "recursive-family.telora",
            "type Expr(A) = enum {'Leaf(A), 'Call(Array(Expr(A)))}; 0",
            Quota::with_fuel(100),
        );
        let expression_family = partial
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "Expr")
            .unwrap();
        assert_eq!(
            partial.definition_facts[&expression_family.id].state,
            FactState::Known
        );
        let scheme = partial.definition_schemes[&expression_family.id].display_name();
        assert_eq!(scheme, "for(A) Fn(TypeOf(A)) -> TypeOf(Expr)");
        assert!(partial.diagnostics.is_empty(), "{:?}", partial.diagnostics);
    }

    #[test]
    fn partial_type_evaluation_accepts_explicit_linked_capabilities() {
        let mut heap = Heap::work();
        let root = heap
            .type_descriptor_value(None, &TypeDescriptor::Int)
            .unwrap();
        let bindings =
            BTreeMap::from([("LinkedType".to_owned(), crate::DataWorld::new(heap, root))]);
        let partial = analyze_partial_types_with_bindings(
            "linked.telora",
            "type Linked = LinkedType; 0",
            Quota::with_fuel(10),
            &bindings,
        );
        let linked = partial
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "Linked")
            .unwrap();
        let fact = &partial.definition_facts[&linked.id];
        assert_eq!(fact.state, FactState::Known);
        assert_eq!(partial.types.node(fact.value.unwrap()), &TypeNode::Int);
        assert!(partial.hir.references().iter().any(|reference| {
            reference.name == "LinkedType"
                && reference.resolution == crate::hir::HirResolution::External
        }));
    }

    #[test]
    fn tool_stage_respects_evaluation_fuel() {
        let error = analyze_source_with_fuel("test", "type Number = Array(Int); 0", 0).unwrap_err();
        assert!(error.message.contains("fuel"));
    }

    #[test]
    fn tool_expressions_share_one_module_account() {
        let error = analyze_source_with_quota(
            "test",
            "type First = Array(Int); type Second = Array(Int); 0",
            Quota::new(1, 1_000, u64::MAX),
        )
        .unwrap_err();
        assert!(error.message.contains("fuel"));
    }


    #[test]
    fn rejects_invalid_metadata_protocol() {
        let error = analyze_source("test", "type Broken = {kind: 'Unknown}; 0").unwrap_err();
        assert!(error.message.contains("unknown value"));

        let malformed = analyze_source(
            "test",
            "type Broken = {kind: 'WithAttributes, inner: Int, attributes: []}; 0",
        )
        .unwrap_err();
        assert!(malformed.message.contains("attributes must be a Dict"));
    }

    #[test]
    fn runtime_validation_uses_computed_metadata() {
        let accepted = crate::run_source(
            "test",
            "type User = struct {name: String, age: Int};\
             validate(User, {age: 36, name: \"Ada\"})",
            100_000,
        )
        .unwrap();
        assert_eq!(
            accepted
                .value()
                .tagged_parts()
                .unwrap()
                .0
                .as_atom()
                .unwrap(),
            "Ok"
        );

        let rejected = crate::run_source(
            "test",
            "type User = struct {name: String, age: Int};\
             validate(User, {age: \"old\", name: \"Ada\"})",
            100_000,
        )
        .unwrap();
        assert_eq!(
            rejected
                .value()
                .tagged_parts()
                .unwrap()
                .0
                .as_atom()
                .unwrap(),
            "Err"
        );

        let family = crate::run_source(
            "test",
            "type Box(A) = struct {value: A};\
             validate(Box(Int), {value: 42})",
            100_000,
        )
        .unwrap();
        assert_eq!(
            family.value().tagged_parts().unwrap().0.as_atom().unwrap(),
            "Ok"
        );
    }

    #[test]
    fn fail_requires_a_string_message_and_has_never_type() {
        let analysis = analyze_source("fail.telora", "fail!(\"bad\", 1)").unwrap();
        assert_eq!(analysis.display(analysis.result_type), "Never");

        let error = analyze_source("fail.telora", "fail!(2, 1)").unwrap_err();
        assert!(
            error.message.contains("Int") && error.message.contains("String"),
            "{}",
            error.message
        );
    }

    #[test]
    fn program_bytecode_externalizes_type_metadata_and_retains_explicit_witnesses() {
        let erased = crate::compile_source(
            "test",
            "type User = struct {name: String}; let user: User = {name: \"Ada\"}; user.name",
        )
        .unwrap();
        assert!(!erased.constants().is_empty());

        let retained =
            crate::compile_source("test", "type User = struct {name: String}; User").unwrap();
        assert!(
            retained
                .constants()
                .iter()
                .all(|constant| matches!(constant, crate::bytecode::Constant::Placeholder))
        );
        let witness =
            crate::run_source("test", "type User = struct {name: String}; User", 100_000).unwrap();
        assert_eq!(witness.value().kind(), crate::ValueKind::Type);
    }
