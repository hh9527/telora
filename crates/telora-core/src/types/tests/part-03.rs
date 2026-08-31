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
        {
            let (name, source) = ("alias", "type Left = Right; type Right = Left; 0");
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
