    #[test]
    fn bootstrap_prelude_keeps_public_projections_consistent() {
        let prelude = BootstrapPrelude::new();
        for name in prelude.schemes.keys() {
            assert!(prelude.types.contains_key(name), "missing type for {name}");
        }
    }

    #[test]
    fn exported_traits_keep_stable_constructor_identity() {
        let analysis = analyze_source(
            "traits.telora",
            r#"trait Display { display: Fn(Self) -> String };
               export { Display };"#,
        )
        .unwrap();
        let trait_id = analysis.trait_ids["Display"];
        assert_eq!(trait_id.module, crate::ModuleId::ANONYMOUS);
        assert_eq!(trait_id.local, crate::FIRST_DYNAMIC_MODULE_LOCAL);
        assert_eq!(analysis.module_interface.traits["Display"], trait_id);
        assert_eq!(
            analysis.module_interface.type_family_templates["Display"]
                .constructor()
                .unwrap()
                .id,
            crate::TypeConstructorId::from(trait_id)
        );
    }

    fn analyze_with_natives(
        source: &str,
        natives: &[(&'static str, usize)],
    ) -> Result<Analysis, FrontendError> {
        let mut sources = SourceDatabase::default();
        let source_id = sources.add("generic-native.telora", source);
        let parsed = parse_registered(&sources, source_id);
        let program = parsed.program.unwrap_or_else(|| {
            panic!(
                "generic native source parses: {source:?}: {:?}",
                parsed.diagnostics
            )
        });
        let mut tool_heap = Heap::main();
        let mut work = Heap::work_for(&tool_heap);
        let external_roots = natives
            .iter()
            .map(|(name, arity)| {
                let value = work.native_closure(
                    NativeFunction::new(name, *arity, native_validate),
                    Vec::<Val>::new().into_boxed_slice(),
                );
                publish_root(&mut tool_heap, &work, value)
                    .map(|value| ((*name).to_owned(), value))
                    .unwrap()
            })
            .collect();
        let debug_sink: Arc<dyn DebugSink> = Arc::new(DiscardDebugSink);
        let mut type_store = TypeStore::default();
        analyze_program_with_bindings_observed(
            "generic-native.telora",
            crate::ModuleId::ANONYMOUS,
            &program,
            &mut QuotaAccount::new(Quota::with_fuel(100_000)),
            &external_roots,
            &HashSet::new(),
            &sources,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &debug_sink,
            &mut tool_heap,
            &mut type_store,
        )
    }

    fn analyze_with_host_binding(
        source: &str,
        native_arity: Option<usize>,
        dynamic: bool,
        interface: Option<TypeScheme>,
    ) -> Result<Analysis, FrontendError> {
        let mut sources = SourceDatabase::default();
        let source_id = sources.add("host-binding.telora", source);
        let parsed = parse_registered(&sources, source_id);
        let program = parsed.program.unwrap_or_else(|| {
            panic!(
                "host binding source parses: {source:?}: {:?}",
                parsed.diagnostics
            )
        });
        let mut tool_heap = Heap::main();
        let mut work = Heap::work_for(&tool_heap);
        let value = native_arity.map_or_else(
            || Val::unknown(crate::heap::DecodedValue::Int(1)),
            |arity| {
                work.native_closure(
                    NativeFunction::new("host", arity, native_validate),
                    Vec::<Val>::new().into_boxed_slice(),
                )
            },
        );
        let external_roots = BTreeMap::from([(
            "host".to_owned(),
            publish_root(&mut tool_heap, &work, value).unwrap(),
        )]);
        let dynamic_bindings = if dynamic {
            HashSet::from(["host".to_owned()])
        } else {
            HashSet::new()
        };
        let external_interfaces = interface
            .map(|scheme| {
                BTreeMap::from([(
                    "host".to_owned(),
                    ModuleInterface {
                        exports: BTreeMap::from([("host".to_owned(), scheme)]),
                        concrete_types: BTreeMap::new(),
                        traits: BTreeMap::new(),
                        type_family_templates: BTreeMap::new(),
                    },
                )])
            })
            .unwrap_or_default();
        let debug_sink: Arc<dyn DebugSink> = Arc::new(DiscardDebugSink);
        let mut type_store = TypeStore::default();
        analyze_program_with_bindings_observed(
            "host-binding.telora",
            crate::ModuleId::ANONYMOUS,
            &program,
            &mut QuotaAccount::new(Quota::with_fuel(100_000)),
            &external_roots,
            &dynamic_bindings,
            &sources,
            &BTreeMap::new(),
            &external_interfaces,
            &debug_sink,
            &mut tool_heap,
            &mut type_store,
        )
    }

    #[test]
    fn host_bindings_distinguish_erased_dynamic_and_declared_interfaces() {
        let erased = analyze_with_host_binding("host(1)", Some(1), false, None).unwrap();
        assert_eq!(
            erased.display(erased.binding_types["host"]),
            "Fn(Any) -> Any"
        );
        assert_eq!(erased.display(erased.result_type), "Any");

        let mut interface_sources = SourceDatabase::default();
        let interface_source = interface_sources.add("host-interface", "");
        let interface_location = crate::Location::from_usize(interface_source, 0..0).unwrap();
        let parameter = TypeParameterId(37);
        let declared = analyze_with_host_binding(
            "host(1)",
            Some(1),
            false,
            Some(TypeScheme {
                parameters: vec![TypeParameter {
                    id: parameter,
                    name: "Value".into(),
                    location: interface_location,
                }],
                body: TypeDescriptor::Function {
                    parameters: vec![TypeDescriptor::Bound(parameter)],
                    result: Box::new(TypeDescriptor::Bound(parameter)),
                },
            }),
        )
        .unwrap();
        assert_eq!(declared.display(declared.result_type), "Int");
        assert_eq!(
            declared.module_interface.exports.get("host"),
            None,
            "a consumed Host interface is not implicitly re-exported"
        );

        let dynamic = analyze_with_host_binding("host", None, true, None).unwrap();
        assert_eq!(dynamic.display(dynamic.binding_types["host"]), "Any");
        assert_eq!(dynamic.display(dynamic.result_type), "Any");

        let chained =
            analyze_with_natives("if 'False { 1 } else if 'True { \"x\" } else { 2.0 }", &[])
                .unwrap();
        let explicit_nested = analyze_with_natives(
            "if 'False { 1 } else { if 'True { \"x\" } else { 2.0 } }",
            &[],
        )
        .unwrap();
        assert_eq!(
            chained.display(chained.result_type),
            explicit_nested.display(explicit_nested.result_type)
        );
    }

    #[test]
    fn unresolved_source_names_fail_before_generic_inference_fallbacks() {
        let error = analyze_with_natives("missing(1)", &[]).unwrap_err();
        assert_eq!(error.message, "unknown binding \"missing\"");
    }

    #[test]
    fn strict_collection_joins_preserve_unions_without_synthesizing_any() {
        let arrays = analyze_with_natives(
            "native stop: Fn() -> Never;\
             let values = [1, \"x\"];\
             let reachable = [stop(), 1];\
             let dynamic: Any = 1;\
             let erased = [dynamic, \"x\"];\
             (values, reachable, erased)",
            &[("stop", 0)],
        )
        .unwrap();
        assert_eq!(
            arrays.display(arrays.binding_types["values"]),
            "Array<Int | String>"
        );
        assert_eq!(
            arrays.display(arrays.binding_types["reachable"]),
            "Array<Int>"
        );
        assert_eq!(arrays.display(arrays.binding_types["erased"]), "Array<Any>");

        let dict = analyze_with_natives(
            "let ints: Dict(Int) = {a: 1};\
             let strings: Dict(String) = {b: \"x\"};\
             let values = {...ints, ...strings};\
             values",
            &[],
        )
        .unwrap();
        assert_eq!(
            dict.display(dict.binding_types["values"]),
            "Dict<Int | String>"
        );

        for source in [
            "let values = [1, \"x\"]; let output: Array(Int) = values; output",
            "let ints: Dict(Int) = {a: 1};\
             let strings: Dict(String) = {b: \"x\"};\
             let values = {...ints, ...strings};\
             let output: Dict(Int) = values; output",
        ] {
            let error = analyze_with_natives(source, &[]).unwrap_err();
            assert!(
                error.message.contains("String") && error.message.contains("Int"),
                "{}",
                error.message
            );
        }
    }

    #[test]
    fn strict_field_projection_is_precise_or_diagnostic() {
        let source = "let record = {value: 1};\
                      let dictionary: Dict(String) = {value: \"x\"};\
                      let alternative = if 'True { {value: 1} } else { {value: \"x\"} };\
                      let dynamic: Any = record;\
                      export def output = (record.value, dictionary.value, alternative.value, dynamic.value);";
        let analysis = analyze_with_natives(source, &[]).unwrap();
        assert_eq!(
            analysis.display(analysis.binding_types["output"]),
            "(Int, String, Int | String, Any)"
        );
        assert_eq!(
            analysis.module_interface.exports["output"].display_name(),
            "(Int, String, Int | String, Any)"
        );
        let dictionary_field_start = source.find("dictionary.value").unwrap();
        let dictionary_field = analysis
            .hir
            .expressions()
            .iter()
            .filter(|expression| expression.location.range().start == dictionary_field_start)
            .max_by_key(|expression| expression.location.range().end)
            .expect("dictionary field expression");
        assert_eq!(
            analysis.display(analysis.expression_types[&dictionary_field.id]),
            "String"
        );

        let inferred_accessor = analyze_with_natives(
            "let get = fn(value) { value.name }; get({name: \"x\", extra: 1})",
            &[],
        )
        .unwrap();
        assert_eq!(
            inferred_accessor.display(inferred_accessor.result_type),
            "String"
        );

        let unconstrained =
            analyze_with_natives("let get = fn(value) { value.name }; get", &[]).unwrap_err();
        assert!(
            unconstrained
                .message
                .contains("cannot infer monomorphic binding \"get\""),
            "{}",
            unconstrained.message
        );

        let deferred = analyze_with_natives(
            "native combine: for(Record, Left, Right)\
                 Fn(Fn(Record) -> Left, Fn(Record) -> Right, Record) -> Tuple([Left, Right]);\
             combine(fn(value) { value.left }, fn(value) { value.right }, {left: 1, right: \"x\"})",
            &[("combine", 3)],
        )
        .unwrap();
        assert_eq!(deferred.display(deferred.result_type), "(Int, String)");

        let shadowed = analyze_with_natives(
            "type Holder = struct {value: Int};\
             let value = fn(input) { input };\
             let read: Fn(Holder) -> Int = fn(value) { value.value };\
             read({value: 1})",
            &[],
        )
        .unwrap();
        assert_eq!(shadowed.display(shadowed.result_type), "Int");

        for (source, expected) in [
            (
                "let value = {present: 1}; value.missing",
                "Struct has no field \"missing\"",
            ),
            ("1.missing", "cannot access field \"missing\" on Int"),
            (
                "let value: Dict(String) = {present: \"x\"};\
                 let output: Int = value.present; output",
                "cannot unify String with Int",
            ),
            (
                "let get: Fn(Dyn) -> Any = fn(value) { value.missing }; get",
                "cannot access field \"missing\" on Dyn",
            ),
            (
                "let value = if 'True { {present: 1} } else { {other: 2} };\
                 value.present",
                "Struct has no field \"present\"",
            ),
        ] {
            let error = analyze_with_natives(source, &[]).unwrap_err();
            assert!(error.message.contains(expected), "{}", error.message);
        }
    }

    #[test]
    fn generic_native_calls_instantiate_fresh_types_and_check_callbacks() {
        let analysis = analyze_with_natives(
            "native identity: for(A) Fn(A) -> A;\
             native map: for(A, B) Fn(Array(A), Fn(A) -> B) -> Array(B);\
             (identity(1), identity(\"x\"), map([1, 2], fn(x) { x + 1 }))",
            &[("identity", 1), ("map", 2)],
        )
        .unwrap();
        assert_eq!(
            analysis.display(analysis.result_type),
            "(Int, String, Array<Int>)"
        );
        let identity = analysis
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "identity")
            .expect("identity definition");
        assert_eq!(identity.type_parameters[0].name, "A");
        let callback_parameter = analysis
            .hir
            .expressions()
            .iter()
            .find(|expression| {
                expression
                    .reference
                    .and_then(|reference| analysis.hir.reference(reference))
                    .is_some_and(|reference| reference.name == "x")
            })
            .expect("callback parameter expression");
        assert_eq!(
            analysis.display(analysis.expression_types[&callback_parameter.id]),
            "Int"
        );
    }

    #[test]
    fn generic_definition_contracts_check_rigidly_and_instantiate_at_each_use() {
        let analysis = analyze_with_natives(
            "decl identity: for(A) Fn(A) -> A;\
             def identity = fn(value) { value };\
             decl apply: for(A, B) Fn(Fn(A) -> B, A) -> B;\
             def apply = fn(function, value) { function(value) };\
             (identity(1), identity(\"x\"), apply(fn(value) { value + 1 }, 2))",
            &[],
        )
        .unwrap();
        assert_eq!(analysis.display(analysis.result_type), "(Int, String, Int)");
        assert!(analysis.module_interface.exports.is_empty());

        let invalid = analyze_with_natives(
            "decl identity: for(A) Fn(A) -> A;\
             def identity = fn(value) { 1 };\
             identity",
            &[],
        )
        .unwrap_err();
        assert!(
            invalid.message.contains("cannot unify Int with T0"),
            "{}",
            invalid.message
        );
    }

    #[test]
    fn definition_contracts_evaluate_referenced_concrete_types_first() {
        for (name, source) in [
            (
                "earlier-contract-type.telora",
                "type Plan = struct {name: String};\
                 def builder: Fn(Int) -> Plan = fn(value) { {name: `plan \\{value}`} };\
                 builder(1)",
            ),
            (
                "later-contract-type.telora",
                "def builder: Fn(Int) -> Plan = fn(value) { {name: `plan \\{value}`} };\
                 type Plan = struct {name: String};\
                 builder(1)",
            ),
            (
                "parameter-and-result-contract-types.telora",
                "type Input = struct {value: Int};\
                 type Output = struct {text: String};\
                 def convert: Fn(Input) -> Output = fn(input) { {text: `value \\{input.value}`} };\
                 convert({value: 1})",
            ),
            (
                "transitive-contract-types.telora",
                "type Plan = NamedPlan;\
                 type NamedPlan = struct {name: String};\
                 def builder: Fn(Int) -> Plan = fn(value) { {name: `plan \\{value}`} };\
                 builder(1)",
            ),
        ] {
            let analysis = analyze_source(name, source).unwrap();
            let expected = if name == "parameter-and-result-contract-types.telora" {
                "Output"
            } else if name == "transitive-contract-types.telora" {
                "NamedPlan"
            } else {
                "Plan"
            };
            assert_eq!(analysis.display(analysis.result_type), expected, "{name}");
        }
    }

    #[test]
    fn contracted_definitions_preserve_generic_callback_result_precision() {
        let source = "type Box(T) = struct {value: T};\
                      type Plan = struct {name: String};\
                      def invoke: for(P) Fn(Fn(Int) -> P) -> Box(P) = fn(build) {\
                          {value: build(1)}\
                      };\
                      def builder: Fn(Int) -> Plan = fn(value) {\
                          {name: `plan:\\{value}`}\
                      };\
                      let result = invoke(builder);\
                      let output = result;\
                      {output}";
        let analysis = analyze_source("callback-contract.telora", source).unwrap();
        assert_eq!(analysis.display(analysis.result_type), "{output: Box}");
        assert_eq!(analysis.display(analysis.binding_types["output"]), "Box");

        let with_unused_family = analyze_source(
            "callback-contract-unused-family.telora",
            "type Box(T) = struct {value: T};\
             type Plan = struct {name: String};\
             type Unused(T) = Tuple([Plan, T]);\
             def invoke: for(P) Fn(Fn(Int) -> P) -> Box(P) = fn(build) {\
                 {value: build(1)}\
             };\
             def builder: Fn(Int) -> Plan = fn(value) {\
                 {name: `plan:\\{value}`}\
             };\
             let result = invoke(builder);\
             let output = result;\
             {output}",
        )
        .unwrap();
        assert_eq!(
            with_unused_family.display(with_unused_family.result_type),
            analysis.display(analysis.result_type)
        );
    }

    #[test]
    fn contract_reachable_concrete_type_cycles_are_diagnosed_deterministically() {
        for (name, source, participants) in [
            (
                "direct",
                "type Loop = Loop;\
                 def use: Fn(Loop) -> Int = fn(value) { 0 };\
                 use",
                &["Loop"][..],
            ),
            (
                "mutual",
                "type Left = Right;\
                 type Right = Left;\
                 def use: Fn(Left) -> Int = fn(value) { 0 };\
                 use",
                &["Left", "Right"][..],
            ),
        ] {
            let error =
                analyze_source(&format!("contract-type-{name}-cycle.telora"), source).unwrap_err();
            assert!(
                error
                    .message
                    .contains("does not reach a struct or enum constructor"),
                "{error}"
            );
            for participant in participants {
                assert!(error.message.contains(participant), "{error}");
            }
        }

        let recursive = analyze_source(
            "recursive-contract-type.telora",
            "type Node = struct {value: Int, children: Array(Node)};\
             def leaf: Fn(Int) -> Node = fn(value) { {value, children: []} };\
             leaf(1)",
        )
        .expect("decorated recursive type contracts retain the sealing path");
        assert!(recursive.declared_types.contains_key("Node"));
    }

    #[test]
    fn recursive_concrete_types_remain_strict_in_definition_contracts_and_families() {
        let recursive = analyze_source(
            "recursive-node.telora",
            "type Node = struct {value: Int, children: Array(Node)};\
             def value_of: Fn(Node) -> Int = fn(node) { node.value };\
             let node: Node = {value: 1, children: []};\
             value_of(node)",
        )
        .unwrap();
        assert_eq!(recursive.display(recursive.result_type), "Int");
        assert_eq!(
            recursive.display(recursive.binding_types["value_of"]),
            "Fn(Node) -> Int"
        );

        let invalid = analyze_source(
            "recursive-node-invalid.telora",
            "type Node = struct {value: Int, children: Array(Node)};\
             def value_of: Fn(Node) -> Int = fn(node) { node.value };\
             value_of(\"bad\")",
        )
        .unwrap_err();
        assert!(invalid.message.contains("String") && invalid.message.contains("Node"));

        let mutual = analyze_source(
            "recursive-expr.telora",
            "type Expr = enum {'Value(Int), 'Call(CallExpr)};\
             type CallExpr = struct {name: String, args: Array(Expr)};\
             type Renderer(Context) = struct {render: Fn(Context, Expr) -> String};\
             type Context = struct {prefix: String};\
             def inspect: Fn(Expr) -> Int = fn(expr) {\
                 match expr {'Value(value) => value, 'Call(call) => 0}\
             };\
             let expr: Expr = 'Call({name: \"sum\", args: ['Value(1)]});\
             inspect(expr)",
        )
        .unwrap();
        assert_eq!(mutual.display(mutual.result_type), "Int");
        assert!(
            !mutual
                .display(mutual.binding_types["inspect"])
                .contains("Any")
        );
        let renderer = mutual
            .definition_schemes
            .iter()
            .find_map(|(definition, scheme)| {
                (mutual.hir.definition(*definition)?.name == "Renderer").then_some(scheme)
            })
            .expect("Renderer family scheme");
        assert!(!renderer.display_name().contains("Any"));
    }

    #[test]
    fn nested_inference_errors_retain_the_offending_expression_location() {
        let source = "def apply: for(A) Fn(Fn(A, Int, A) -> A, A) -> A = fn(step, acc) {\
                      step(acc, 1)\
                      };\
                      apply(fn(acc, value, extra) { acc + value + extra }, 0)";
        let error = analyze_with_natives(source, &[]).unwrap_err();
        assert!(
            error.message.contains("call expects 3 arguments, found 2"),
            "{}",
            error.message
        );
        let diagnostic = error.diagnostic.expect("located inference diagnostic");
        let call = "step(acc, 1)";
        let start = source.find(call).expect("call expression exists");
        assert_eq!(
            diagnostic.labels[0].location.range(),
            start..start + call.len()
        );
    }

    #[test]
    fn strict_contracts_authorize_related_generic_results_after_shallow_inference() {
        let natives = &[("map", 2), ("flat_map", 2)];
        let helpers = "native map: for(A, B) Fn(Array(A), Fn(A) -> B) -> Array(B);\
                       native flat_map: for(A, B) Fn(Array(A), Fn(A) -> Array(B)) -> Array(B);\
                       def option_to_list: for(A) Fn(Option(A)) -> Array(A) = fn(value) {\
                           match value { 'Some(item) => [item], 'None => [] }\
                       };\
                       def completed_values: for(A) Fn(Array(Option(A))) -> Array(A) = fn(results) {\
                           flat_map(results, option_to_list)\
                       };";

        let tuple_source = [
            helpers,
            "type Batch(A) = Tuple([Array(Option(A)), Array(A)]);\
             export def collect: for(Input, Output)\
                 Fn(Array(Input), Fn(Input) -> Option(Output)) -> Batch(Output) =\
                 fn(inputs, lower) {\
                     let results = map(inputs, lower);\
                     let values = completed_values(results);\
                     (results, values)\
                 };",
        ]
        .concat();
        let tuple = analyze_with_natives(&tuple_source, natives).unwrap();
        assert_eq!(
            tuple.module_interface.exports["collect"].display_name(),
            "for(Input, Output) Fn(Array<Input>, Fn(Input) -> enum {None, Some(Output)}) -> (Array<enum {None, Some(Output)}>, Array<Output>)"
        );

        let struct_source = [
            helpers,
            "type Batch(A) = struct {\
                 complete: Option(Array(A)),\
                 results: Array(Option(A)),\
                 values: Array(A),\
             };\
             export def collect: for(Input, Output)\
                 Fn(Array(Input), Fn(Input) -> Option(Output)) -> Batch(Output) =\
                 fn(inputs, lower) {\
                     let results = map(inputs, lower);\
                     let values = completed_values(results);\
                     let complete = if 'True { 'Some(values) } else { 'None };\
                     {complete: complete, results: results, values: values}\
                 };",
        ]
        .concat();
        let structure = analyze_with_natives(&struct_source, natives).unwrap();
        assert!(
            structure.module_interface.exports["collect"]
                .display_name()
                .contains("-> Batch")
        );

        let invalid_source = [
            helpers,
            "type Batch(A) = struct {results: Array(Option(A)), values: Array(A)};\
             export def collect: for(Input, Output)\
                 Fn(Array(Input), Fn(Input) -> Option(Output)) -> Batch(Output) =\
                 fn(inputs, lower) {\
                     let results = map(inputs, lower);\
                     {results: results, values: [1]}\
                 };",
        ]
        .concat();
        let invalid = analyze_with_natives(&invalid_source, natives).unwrap_err();
        assert!(
            invalid.message.contains("cannot unify Int with T1"),
            "{}",
            invalid.message
        );
    }

    #[test]
    fn generic_definition_aliases_instantiate_once_and_exports_retain_schemes() {
        let alias_error = analyze_with_natives(
            "decl identity: for(A) Fn(A) -> A;\
             def identity = fn(value) { value };\
             let local = identity;\
             (local(1), local(\"x\"))",
            &[],
        )
        .unwrap_err();
        assert!(alias_error.message.contains("cannot unify String with Int"));

        let exported = analyze_with_natives(
            "decl identity: for(A) Fn(A) -> A;\
             def identity = fn(value) { value };\
             {identity: identity}",
            &[],
        )
        .unwrap();
        let scheme = &exported.module_interface.exports["identity"];
        assert_eq!(scheme.parameters[0].name, "A");
        assert!(matches!(
            &scheme.body,
            TypeDescriptor::Function { parameters, result }
                if parameters == &[TypeDescriptor::Bound(TypeParameterId(0))]
                    && **result == TypeDescriptor::Bound(TypeParameterId(0))
        ));
        let identity = exported
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "identity")
            .unwrap();
        assert_eq!(
            exported.definition_schemes[&identity.id].display_name(),
            "for(A) Fn(A) -> A"
        );
    }

    #[test]
    fn annotated_definitions_are_atomic_generic_contracts() {
        let analysis = analyze_with_natives(
            "def identity: for(A) Fn(A) -> A = fn(value) { value };\
             (identity(1), identity(\"x\"))",
            &[],
        )
        .unwrap();
        assert_eq!(analysis.display(analysis.result_type), "(Int, String)");

        let duplicate = analyze_with_natives(
            "decl identity: for(A) Fn(A) -> A;\
             def identity: for(A) Fn(A) -> A = fn(value) { value };\
             identity",
            &[],
        )
        .unwrap_err();
        assert!(
            duplicate
                .message
                .contains("duplicate declaration \"identity\"")
        );

        let specialized = analyze_with_natives(
            "def identity: for(A) Fn(A) -> A = fn(value) { 1 }; identity",
            &[],
        )
        .unwrap_err();
        assert!(
            specialized.message.contains("cannot unify Int with T0"),
            "{}",
            specialized.message
        );
    }

    #[test]
    fn generic_native_result_uses_expected_type_and_rejects_missing_or_conflicting_evidence() {
        let inferred = analyze_with_natives(
            "native empty: for(A) Fn() -> Array(A);\
             let values: Array(Int) = empty();\
             values",
            &[("empty", 0)],
        )
        .unwrap();
        assert_eq!(inferred.display(inferred.result_type), "Array<Int>");

        let missing = analyze_with_natives(
            "native empty: for(A) Fn() -> Array(A); empty()",
            &[("empty", 0)],
        )
        .unwrap_err();
        assert!(missing.message.contains("cannot infer generic result type"));

        let conflicting = analyze_with_natives(
            "native choose: for(A) Fn(A, A) -> A; choose(1, \"x\")",
            &[("choose", 2)],
        )
        .unwrap_err();
        assert!(
            conflicting.message.contains("cannot unify String with Int"),
            "{}",
            conflicting.message
        );
    }

    #[test]
    fn generic_calls_complete_from_the_whole_call_context() {
        for source in [
            "native empty: for(A) Fn() -> A; native choose: for(A) Fn(A, A) -> A; choose(empty(), 1)",
            "native empty: for(A) Fn() -> A; native choose: for(A) Fn(A, A) -> A; choose(1, empty())",
            "native stop: Fn() -> Never; native choose: for(A) Fn(A, A) -> A; choose(stop(), 1)",
            "native stop: Fn() -> Never; native choose: for(A) Fn(A, A) -> A; choose(1, stop())",
        ] {
            let analysis =
                analyze_with_natives(source, &[("empty", 0), ("choose", 2), ("stop", 0)]).unwrap();
            assert_eq!(analysis.display(analysis.result_type), "Int");
        }

        let callback = analyze_with_natives(
            "native make: for(A, B) Fn(Fn(A) -> B) -> B;\
             let values: Array(Int) = make(fn(value) { [value] }); values",
            &[("make", 1)],
        )
        .unwrap();
        assert_eq!(callback.display(callback.result_type), "Array<Int>");

        let partial = analyze_with_natives(
            "native empty: for(A) Fn() -> A; native choose: for(A) Fn(A, A) -> A;\
             choose@[_](empty(), \"value\")",
            &[("empty", 0), ("choose", 2)],
        )
        .unwrap();
        assert_eq!(partial.display(partial.result_type), "String");
    }

    #[test]
    fn generic_use_refines_option_result_of_a_let_bound_callback() {
        let analysis = analyze_with_natives(
            "def apply: for(A, B) Fn(A, Fn(A) -> Option(B)) -> Option(B) =\
                 fn(value, f) { f(value) };\
             let build = fn(value) {\
                 if value > 0 { 'Some(\"ok\") } else { 'None }\
             };\
             let unrelated = 1;\
             apply(1, build)",
            &[],
        )
        .unwrap();
        assert_eq!(
            analysis.display(analysis.result_type),
            "enum {None, Some(String)}"
        );

        for callback in [
            "fn(value) { if value > 0 { 'Some(1) } else { 'None } }",
            "fn(value) { if value > 0 { 'Some(\"ok\") } else { 'Foreign } }",
        ] {
            let error = analyze_with_natives(
                &format!(
                    "def apply: for(A) Fn(A, Fn(A) -> Option(String)) -> Option(String) =\
                         fn(value, f) {{ f(value) }};\
                     let build = {callback}; apply(1, build)"
                ),
                &[],
            )
            .unwrap_err();
            assert!(
                error.message.contains("Int")
                    || error.message.contains("Foreign")
                    || error.message.contains("Some(String)"),
                "{}",
                error.message
            );
        }
    }

    #[test]
    fn generic_calls_combine_singleton_atoms_with_closed_enum_evidence() {
        let prelude = "type NodeId = enum {'Base, 'Other};\
             let nodes: Array(NodeId) = ['Other];";
        for call in [
            "def choose: for(Node) Fn(Node, Array(Node)) -> Node = fn(base, nodes) { base };\
             choose('Base, nodes)",
            "def choose: for(Node) Fn(Array(Node), Node) -> Node = fn(nodes, base) { base };\
             choose(nodes, 'Base)",
            "def choose: for(Node) Fn(Node, Node, Array(Node)) -> Node = fn(base, other, nodes) { base };\
             choose('Base, 'Other, nodes)",
        ] {
            let analysis = analyze_with_natives(&format!("{prelude}{call}"), &[]).unwrap();
            assert_eq!(analysis.display(analysis.result_type), "NodeId");
        }

        let conflict = analyze_with_natives(
            "type NodeId = enum {'Base, 'Other};\
             type ForeignId = enum {'Foreign};\
             def choose: for(Node) Fn(Node, Array(Node)) -> Node = fn(base, nodes) { base };\
             let foreign: Array(ForeignId) = ['Foreign];\
             choose('Base, foreign)",
            &[],
        )
        .unwrap_err();
        assert!(
            conflict
                .message
                .contains("variant 'Base is not part of ForeignId"),
            "{}",
            conflict.message
        );
    }

    #[test]
    fn generic_call_context_rejects_conflicts_and_remains_underconstrained() {
        let conflict = analyze_with_natives(
            "native empty: for(A) Fn() -> A; native choose: for(A) Fn(A, A) -> A;\
             let value: String = choose(empty(), 1); value",
            &[("empty", 0), ("choose", 2)],
        )
        .unwrap_err();
        assert!(
            conflict.message.contains("cannot unify"),
            "{}",
            conflict.message
        );

        let unresolved = analyze_with_natives(
            "native empty: for(A) Fn() -> A; native choose: for(A) Fn(A, A) -> A;\
             choose(empty(), empty())",
            &[("empty", 0), ("choose", 2)],
        )
        .unwrap_err();
        assert!(
            unresolved
                .message
                .contains("cannot infer generic result type"),
            "{}",
            unresolved.message
        );
    }

    #[test]
    fn never_checks_directionally_without_constraining_generic_evidence() {
        let inferred = analyze_with_natives(
            "native stop: Fn() -> Never;\
             native choose: for(A) Fn(A, A) -> A;\
             (choose(stop(), 1), choose(1, stop()))",
            &[("stop", 0), ("choose", 2)],
        )
        .unwrap();
        assert_eq!(inferred.display(inferred.result_type), "(Int, Int)");

        let missing = analyze_with_natives(
            "native stop: Fn() -> Never;\
             native choose: for(A) Fn(A, A) -> A;\
             choose(stop(), stop())",
            &[("stop", 0), ("choose", 2)],
        )
        .unwrap_err();
        assert!(missing.message.contains("cannot infer generic result type"));

        let expected = analyze_with_natives(
            "native stop: Fn() -> Never;\
             native choose: for(A) Fn(A, A) -> A;\
             let value: String = choose(stop(), stop()); value",
            &[("stop", 0), ("choose", 2)],
        )
        .unwrap();
        assert_eq!(expected.display(expected.result_type), "String");
    }

    #[test]
    fn adversarial_never_evidence_is_directional_through_structures_and_callbacks() {
        for (name, source) in [
            (
                "never-first",
                "native stop: Fn() -> Never; native choose: for(A) Fn(A, A) -> A;\
                 choose([stop()], [1])",
            ),
            (
                "never-last",
                "native stop: Fn() -> Never; native choose: for(A) Fn(A, A) -> A;\
                 choose([1], [stop()])",
            ),
        ] {
            let analysis = analyze_with_natives(source, &[("stop", 0), ("choose", 2)])
                .unwrap_or_else(|error| panic!("{name}: {}", error.message));
            assert_eq!(analysis.display(analysis.result_type), "Array<Int>");
        }

        let callback = analyze_with_natives(
            "native stop: Fn() -> Never;\
             native apply: for(A, B) Fn(A, Fn(A) -> B, B) -> B;\
             apply(1, fn(value) { stop() }, \"fallback\")",
            &[("stop", 0), ("apply", 3)],
        )
        .unwrap();
        assert_eq!(callback.display(callback.result_type), "String");

        let reverse = analyze_with_natives(
            "native produce: for(A) Fn() -> A; let impossible: Never = produce(); impossible",
            &[("produce", 0)],
        )
        .unwrap();
        assert_eq!(reverse.display(reverse.result_type), "Never");
    }

    #[test]
    fn never_is_bottom_for_expected_types_and_branch_results() {
        let analysis = analyze_with_natives(
            "native stop: Fn() -> Never;\
             let value: Int = stop();\
             let branch = if 'True { 1 } else { stop() };\
             let all_never = if 'True { stop() } else { stop() };\
             (value, branch, all_never, Never)",
            &[("stop", 0)],
        )
        .unwrap();
        assert_eq!(
            analysis.display(analysis.result_type),
            "(Int, Int, Never, TypeOf(Never))"
        );

        let reverse = analyze_with_natives(
            "native produce: Fn() -> Int;\
             let impossible: Never = produce(); impossible",
            &[("produce", 0)],
        )
        .unwrap_err();
        assert!(reverse.message.contains("Int") && reverse.message.contains("Never"));
    }

    #[test]
    fn nested_structural_expectations_preserve_generic_constraints() {
        let inferred = analyze_with_natives(
            "native concat: for(A) Fn(Array(Array(A))) -> Array(A);\
             (concat([[1], [], [2]]), concat([[], [1]]), concat([[1], []]))",
            &[("concat", 1)],
        )
        .unwrap();
        assert_eq!(
            inferred.display(inferred.result_type),
            "(Array<Int>, Array<Int>, Array<Int>)"
        );

        let expected = analyze_with_natives(
            "native concat: for(A) Fn(Array(Array(A))) -> Array(A);\
             let values: Array(String) = concat([[], []]); values",
            &[("concat", 1)],
        )
        .unwrap();
        assert_eq!(expected.display(expected.result_type), "Array<String>");

        let missing = analyze_with_natives(
            "native concat: for(A) Fn(Array(Array(A))) -> Array(A);\
             concat([[], []])",
            &[("concat", 1)],
        )
        .unwrap_err();
        assert!(missing.message.contains("cannot infer generic result type"));
    }

    #[test]
    fn structural_constraints_ignore_never_and_preserve_metadata_widening() {
        let analysis = analyze_with_natives(
            "native stop: Fn() -> Never;\
             native concat: for(A) Fn(Array(Array(A))) -> Array(A);\
             native identity: for(A) Fn(Array(A)) -> Array(A);\
             (concat([[stop()], [1]]), identity([Int, String]))",
            &[("stop", 0), ("concat", 1), ("identity", 1)],
        )
        .unwrap();
        assert_eq!(
            analysis.display(analysis.result_type),
            "(Array<Int>, Array<Type>)"
        );

        let conflict = analyze_with_natives(
            "native concat: for(A) Fn(Array(Array(A))) -> Array(A);\
             concat([[1], [\"x\"]])",
            &[("concat", 1)],
        )
        .unwrap_err();
        assert!(conflict.message.contains("String") && conflict.message.contains("Int"));
    }

    #[test]
    fn unannotated_closures_infer_parameters_from_their_bodies() {
        let arithmetic =
            analyze_with_natives("let increment = fn(value) { value + 1 }; increment", &[])
                .unwrap();
        assert_eq!(arithmetic.display(arithmetic.result_type), "Fn(Int) -> Int");

        let known_call = analyze_with_natives(
            "native length: Fn(String) -> Int;\
             let measure = fn(value) { length(value) }; measure",
            &[("length", 1)],
        )
        .unwrap();
        assert_eq!(
            known_call.display(known_call.result_type),
            "Fn(String) -> Int"
        );

        let related = analyze_with_natives(
            "let combine = fn(left, right) { left + right + 1 }; combine",
            &[],
        )
        .unwrap();
        assert_eq!(related.display(related.result_type), "Fn(Int, Int) -> Int");
    }

    #[test]
    fn unknown_callee_calls_infer_closed_function_shapes() {
        let apply = analyze_with_natives(
            "let apply = fn(callback, value) { callback(value) }; apply",
            &[],
        )
        .unwrap();
        let apply_definition = apply
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "apply")
            .unwrap();
        assert_eq!(
            apply.definition_schemes[&apply_definition.id].display_name(),
            "for(A, B) Fn(Fn(A) -> B, A) -> B"
        );

        let called = analyze_with_natives(
            "let apply = fn(callback, value) { callback(value) };\
             apply(fn(value) { value + 1 }, 41)",
            &[],
        )
        .unwrap();
        assert_eq!(called.display(called.result_type), "Int");

        let intrinsic = analyze_with_natives(
            "let use = fn(callback) { callback(1.0) + 2.0 };\
             use(fn(value) { value })",
            &[],
        )
        .unwrap();
        assert_eq!(intrinsic.display(intrinsic.result_type), "Float");
    }

    #[test]
    fn unknown_callee_calls_use_expected_results_and_existing_completion() {
        let expected = analyze_with_natives(
            "let recover = fn(callback) { callback() };\
             let value: String = recover(fn() { \"ok\" }); value",
            &[],
        )
        .unwrap();
        assert_eq!(expected.display(expected.result_type), "String");

        let conflict = analyze_with_natives(
            "let apply = fn(callback) { callback(1) };\
             apply(fn(value: String) { value })",
            &[],
        )
        .unwrap_err();
        assert!(
            conflict.message.contains("cannot unify"),
            "{}",
            conflict.message
        );

        let incomplete =
            analyze_with_natives("let invoke = fn(callback) { callback() }; invoke", &[]).unwrap();
        let invoke = incomplete
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "invoke")
            .unwrap();
        assert_eq!(
            incomplete.definition_schemes[&invoke.id].display_name(),
            "for(A) Fn(Fn() -> A) -> A"
        );
    }

    #[test]
    fn inferred_callable_obligations_converge_across_repeated_calls() {
        for source in [
            "let use = fn(callback) { (callback(1), callback(2)) };\
             use(fn(value) { value + 1 })",
            "let use = fn(callback) { (callback(2), callback(1)) };\
             use(fn(value) { value + 1 })",
        ] {
            let analysis = analyze_with_natives(source, &[]).unwrap();
            assert_eq!(analysis.display(analysis.result_type), "(Int, Int)");
        }

        for source in [
            "let use = fn(callback) { (callback(1), callback(\"x\")) }; use",
            "let use = fn(callback) { (callback(\"x\"), callback(1)) }; use",
            "let use = fn(callback) { let alias = callback;\
                 (alias(1), callback(\"x\")) }; use",
        ] {
            let error = analyze_with_natives(source, &[]).unwrap_err();
            assert!(error.message.contains("cannot unify"), "{}", error.message);
        }

        let arity = analyze_with_natives(
            "let use = fn(callback) { (callback(1), callback(1, 2)) }; use",
            &[],
        )
        .unwrap_err();
        assert!(arity.message.contains("call expects 1 arguments, found 2"));
    }

    #[test]
    fn inferred_callable_obligations_converge_through_nested_calls() {
        let compose = analyze_with_natives(
            "let compose = fn(outer, inner, value) { outer(inner(value)) }; compose",
            &[],
        )
        .unwrap();
        let definition = compose
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "compose")
            .unwrap();
        assert_eq!(
            compose.definition_schemes[&definition.id].display_name(),
            "for(A, B, C) Fn(Fn(A) -> B, Fn(C) -> A, C) -> B"
        );

        let nested = analyze_with_natives(
            "let invoke_factory = fn(factory) { factory()() }; invoke_factory",
            &[],
        )
        .unwrap();
        let definition = nested
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "invoke_factory")
            .unwrap();
        assert_eq!(
            nested.definition_schemes[&definition.id].display_name(),
            "for(A) Fn(Fn() -> Fn() -> A) -> A"
        );

        let executed = analyze_with_natives(
            "let compose = fn(outer, inner, value) { outer(inner(value)) };\
             compose(fn(value) { `value=\\{value}` }, fn(value) { value + 1 }, 41)",
            &[],
        )
        .unwrap();
        assert_eq!(executed.display(executed.result_type), "String");
    }
