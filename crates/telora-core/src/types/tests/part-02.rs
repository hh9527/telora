    #[test]
    fn callable_diagnostics_distinguish_static_values_from_explicit_any() {
        for (source, expected) in [
            ("let value = 1; value(2)", "Int"),
            ("let value = \"text\"; value(1)", "String"),
            ("let value = [1]; value(2)", "Array<Int>"),
            ("let value = {item: 1}; value(2)", "{item: Int}"),
            ("let value = Int; value(1)", "TypeOf(Int)"),
        ] {
            let error = analyze_with_natives(source, &[]).unwrap_err();
            assert_eq!(
                error.message,
                format!("cannot call value of type {expected}")
            );
        }

        let dynamic =
            analyze_with_natives("let callable: Any = fn(value) { value }; callable(1)", &[])
                .unwrap();
        assert_eq!(dynamic.display(dynamic.result_type), "Any");

        let arity = analyze_with_natives(
            "let invoke = fn(callback) { (callback(1), callback(1, 2)) }; invoke",
            &[],
        )
        .unwrap_err();
        assert!(arity.message.contains("call expects 1 arguments, found 2"));
    }

    #[test]
    fn inferred_callable_schemes_publish_separately_from_call_instances() {
        let source = "let apply = fn(callback, value) { callback(value) };\
                      let result = apply(fn(value) { value + 1 }, 41);\
                      {apply: apply, result: result}";
        let analysis = analyze_with_natives(source, &[]).unwrap();
        assert_eq!(
            analysis.module_interface.exports["apply"].display_name(),
            "for(A, B) Fn(Fn(A) -> B, A) -> B"
        );
        assert_eq!(analysis.display(analysis.binding_types["result"]), "Int");

        let call_start = source.find("apply(fn").unwrap();
        let call = analysis
            .hir
            .expressions()
            .iter()
            .filter(|expression| expression.location.range().start == call_start)
            .max_by_key(|expression| expression.location.range().end)
            .unwrap();
        assert_eq!(analysis.display(analysis.expression_types[&call.id]), "Int");
    }

    #[test]
    fn intrinsic_expression_constraints_infer_booleans_and_numeric_families() {
        let condition = analyze_with_natives(
            "let select = fn(condition, value) {\
                 if condition { value } else { value }\
             }; (select('True, 1), select('False, \"value\"))",
            &[],
        )
        .unwrap();
        assert_eq!(condition.display(condition.result_type), "(Int, String)");

        for (source, expected) in [
            ("let add = fn(value) { value + 1 }; add", "Fn(Int) -> Int"),
            ("let add = fn(value) { 1 + value }; add", "Fn(Int) -> Int"),
            (
                "let scale = fn(value) { value * 1.5 }; scale",
                "Fn(Float) -> Float",
            ),
            ("let negative: Float = -1.5; negative", "Float"),
            (
                "let before = fn(value) { value < 1 }; before",
                "Fn(Int) -> enum {False, True}",
            ),
            (
                "let before = fn(value) { value < \"z\" }; before",
                "Fn(String) -> enum {False, True}",
            ),
        ] {
            let analysis = analyze_with_natives(source, &[]).unwrap();
            assert_eq!(analysis.display(analysis.result_type), expected);
        }

        let equality = analyze_with_natives("1 == \"1\"", &[]).unwrap_err();
        assert!(equality.message.contains("cannot unify"));
        assert!(equality.message.contains("String"));
        assert!(equality.message.contains("Int"));

        let logical_not = analyze_with_natives(
            "let invert_bool: Fn(Bool) -> Bool = fn(value) { !value };\
             let invert_int: Fn(Int) -> Int = fn(value) { !value };\
             (invert_bool, invert_int, !'True, !0)",
            &[],
        )
        .unwrap();
        assert_eq!(
            logical_not.display(logical_not.result_type),
            "(Fn(enum {False, True}) -> enum {False, True}, Fn(Int) -> Int, enum {False, True}, Int)"
        );
    }

    #[test]
    fn intrinsic_expression_constraints_reject_invalid_or_ambiguous_operators() {
        for source in ["-\"text\"", "!1.0", "!\"text\"", "1 + 1.5"] {
            let error = analyze_with_natives(source, &[]).unwrap_err();
            assert!(
                error.message.contains("Int or Float")
                    || error.message.contains("Int or Bool")
                    || error.message.contains("cannot unify"),
                "{}",
                error.message
            );
        }

        let ambiguous =
            analyze_with_natives("let invert = fn(value) { !value }; invert", &[]).unwrap_err();
        assert!(
            ambiguous
                .message
                .contains("cannot infer monomorphic binding"),
            "{}",
            ambiguous.message
        );

        let ambiguous =
            analyze_with_natives("let negate = fn(value) { -value }; negate", &[]).unwrap_err();
        assert!(
            ambiguous
                .message
                .contains("cannot infer monomorphic binding")
        );

        let dynamic = analyze_with_natives(
            "let negate: Fn(Any) -> Any = fn(value) { -value }; negate",
            &[],
        )
        .unwrap();
        assert_eq!(dynamic.display(dynamic.result_type), "Fn(Any) -> Any");
    }

    #[test]
    fn equality_requires_one_static_semantic_type() {
        for source in [
            "1 == \"1\"",
            "[1] == [\"1\"]",
            "type Left = struct {value: Int}; type Right = struct {value: Int}; let left: Left = {value: 1}; let right: Right = {value: 1}; left == right",
        ] {
            let error = analyze_with_natives(source, &[]).unwrap_err();
            assert!(error.message.contains("cannot unify"), "{error}");
        }

        for source in [
            "('Ok, 1) == ('Err, 1)",
            "{state: 'Ready, value: 1} != {state: 'Pending, value: 1}",
        ] {
            let analysis = analyze_with_natives(source, &[]).unwrap();
            assert_eq!(analysis.display(analysis.result_type), "enum {False, True}");
        }
    }

    #[test]
    fn ordered_comparisons_reject_mixed_unsupported_and_ambiguous_operands() {
        for source in ["1 < 1.0", "\"a\" < 1", "[1] < [2]"] {
            let error = analyze_with_natives(source, &[]).unwrap_err();
            assert!(
                error.message.contains("cannot unify")
                    || error.message.contains("ordered comparison"),
                "{source}: {}",
                error.message
            );
        }

        let ambiguous =
            analyze_with_natives("let before = fn(left, right) { left < right }; before", &[])
                .unwrap_err();
        assert!(
            ambiguous
                .message
                .contains("cannot infer monomorphic binding"),
            "{}",
            ambiguous.message
        );
    }

    #[test]
    fn adversarial_numeric_domains_do_not_generalize_or_merge() {
        let conflict = analyze_with_natives(
            "let add = fn(left, right) { left + right };\
             let integer = add(1, 2); add(1.0, 2.0)",
            &[],
        )
        .unwrap_err();
        assert!(
            conflict.message.contains("cannot unify"),
            "{}",
            conflict.message
        );

        let callback = analyze_with_natives(
            "native use: for(A) Fn(Fn(Float) -> A) -> A; use(fn(value) { value + 2.0 })",
            &[("use", 1)],
        )
        .unwrap();
        assert_eq!(callback.display(callback.result_type), "Float");

        let explicit = analyze_with_natives(
            "let negate = fn(value) { -value }; negate@[String](\"x\")",
            &[],
        )
        .unwrap_err();
        assert!(
            explicit
                .message
                .contains("cannot infer monomorphic binding")
                || explicit.message.contains("monomorphic binding")
                || explicit
                    .message
                    .contains("statically known generic binding"),
            "{}",
            explicit.message
        );
    }

    #[test]
    fn branch_joins_are_canonical_pure_and_order_independent() {
        let left = analyze_with_natives("if 'True { 1 } else { \"x\" }", &[]).unwrap();
        let right = analyze_with_natives("if 'True { \"x\" } else { 1 }", &[]).unwrap();
        assert_eq!(
            left.display(left.result_type),
            right.display(right.result_type)
        );
        assert_eq!(left.display(left.result_type), "Int | String");

        let metadata = analyze_with_natives("if 'True { Int } else { String }", &[]).unwrap();
        let reversed = analyze_with_natives("if 'True { String } else { Int }", &[]).unwrap();
        assert_eq!(metadata.display(metadata.result_type), "Type");
        assert_eq!(reversed.display(reversed.result_type), "Type");

        let nested = analyze_with_natives(
            "if 'True { if 'False { 1 } else { \"x\" } } else { 1 }",
            &[],
        )
        .unwrap();
        assert_eq!(nested.display(nested.result_type), "Int | String");

        let delayed = analyze_with_natives(
            "def choose = fn(flag, value) {\
                 if flag { value } else { 1 }\
             }; let selected = choose('True, 2); choose",
            &[],
        )
        .unwrap();
        assert_eq!(
            delayed.display(delayed.result_type),
            "Fn(enum {False, True}, Any) -> Any | Int"
        );

        let dynamic =
            analyze_with_natives("let value: Any = 1; if 'True { value } else { 1 }", &[]).unwrap();
        assert_eq!(dynamic.display(dynamic.result_type), "Any");
    }

    #[test]
    fn adversarial_branch_joins_are_pure_symmetric_and_canonical() {
        for (left, right, expected) in [
            (
                "if 'True { if 'False { 1 } else { \"x\" } } else { 1.0 }",
                "if 'True { 1.0 } else { if 'False { \"x\" } else { 1 } }",
                "Float | Int | String",
            ),
            (
                "let dynamic: Any = 1; if 'True { dynamic } else { \"x\" }",
                "let dynamic: Any = 1; if 'True { \"x\" } else { dynamic }",
                "Any",
            ),
            (
                "if 'True { Int } else { Array(String) }",
                "if 'True { Array(String) } else { Int }",
                "Type",
            ),
        ] {
            let left = analyze_with_natives(left, &[]).unwrap();
            let right = analyze_with_natives(right, &[]).unwrap();
            assert_eq!(left.display(left.result_type), expected);
            assert_eq!(right.display(right.result_type), expected);
        }

        let no_leak = analyze_with_natives(
            "let select = fn(flag, value) { if flag { value } else { 1 } };\
             (select('True, \"x\"), select('False, 2.0))",
            &[],
        )
        .unwrap();
        assert_eq!(
            no_leak.display(no_leak.result_type),
            "(Int | String, Float | Int)"
        );
    }

    #[test]
    fn structural_joins_infer_empty_collection_elements_from_sibling_branches() {
        for source in [
            "let choose = fn(flag) { if flag { {kind: 'Full, path: [1]} } else { {kind: 'Empty, path: []} } }; choose",
            "let choose = fn(flag) { if flag { {kind: 'Empty, path: []} } else { {kind: 'Full, path: [1]} } }; choose",
        ] {
            let analysis = analyze_with_natives(source, &[]).unwrap();
            assert_eq!(
                analysis.display(analysis.result_type),
                "Fn(enum {False, True}) -> {kind: 'Empty, path: Array<Int>} | {kind: 'Full, path: Array<Int>}"
            );
        }

        let matched = analyze_with_natives(
            "let choose = fn(value) { match value {\
                 0 => {kind: 'First, path: []},\
                 1 => {kind: 'Second, path: [1]},\
                 2 => {kind: 'Third, path: []}\
             } }; choose",
            &[],
        )
        .unwrap();
        assert_eq!(
            matched.display(matched.result_type),
            "Fn(Any) -> {kind: 'First, path: Array<Int>} | {kind: 'Second, path: Array<Int>} | {kind: 'Third, path: Array<Int>}"
        );

        let nested = analyze_with_natives(
            "let choose = fn(flag) { if flag { 'Some(({path: []},)) } else { 'Some(({path: [1]},)) } }; choose",
            &[],
        )
        .unwrap();
        assert_eq!(
            nested.display(nested.result_type),
            "Fn(enum {False, True}) -> 'Some(({path: Array<Int>}))"
        );

        let conflict = analyze_with_natives(
            "let choose = fn(value) { match value {\
                 0 => {kind: 'Empty, path: []},\
                 1 => {kind: 'Ints, path: [1]},\
                 2 => {kind: 'Strings, path: [\"x\"]}\
             } }; choose",
            &[],
        )
        .unwrap();
        assert_eq!(
            conflict.display(conflict.result_type),
            "Fn(Any) -> {kind: 'Empty, path: Array<Int | String>} | {kind: 'Ints, path: Array<Int>} | {kind: 'Strings, path: Array<String>}"
        );
    }

    #[test]
    fn match_joins_are_stable_across_arm_order_and_absorb_never() {
        let first = analyze_with_natives("match 0 { 0 => 1, 1 => \"x\" }", &[]).unwrap();
        let reversed = analyze_with_natives("match 0 { 1 => \"x\", 0 => 1 }", &[]).unwrap();
        assert_eq!(
            first.display(first.result_type),
            reversed.display(reversed.result_type)
        );
        assert_eq!(first.display(first.result_type), "Int | String");

        let never = analyze_with_natives(
            "native stop: Fn() -> Never;\
             if 'True { stop() } else { 1 }",
            &[("stop", 0)],
        )
        .unwrap();
        assert_eq!(never.display(never.result_type), "Int");
    }

    #[test]
    fn partial_closure_contracts_constrain_only_annotated_positions() {
        for (source, expected) in [
            (
                "let add = fn(value: Int, other) { value + other }; add",
                "Fn(Int, Int) -> Int",
            ),
            (
                "let add = fn(value) -> Int { value + 1 }; add",
                "Fn(Int) -> Int",
            ),
            (
                "let decorate = fn(ctx: Any, value) -> Int { value + 1 }; decorate",
                "Fn(Any, Int) -> Int",
            ),
            (
                "let outer = fn(value: Int) {\
                     fn(other: Int) -> Int { value + other }\
                 }; outer",
                "Fn(Int) -> Fn(Int) -> Int",
            ),
        ] {
            let analysis = analyze_with_natives(source, &[]).unwrap();
            assert_eq!(analysis.display(analysis.result_type), expected);
        }

        let compatible = analyze_with_natives(
            "let increment: Fn(Int) -> Int = fn(value: Int) -> Int { value + 1 }; increment",
            &[],
        )
        .unwrap();
        assert_eq!(compatible.display(compatible.result_type), "Fn(Int) -> Int");
    }

    #[test]
    fn partial_closure_contracts_reject_conflicts_and_invalid_metadata() {
        let conflict = analyze_with_natives(
            "let value: Fn(String) -> String = fn(value: Int) -> Int { value }; value",
            &[],
        )
        .unwrap_err();
        assert!(conflict.message.contains("cannot unify"));

        let invalid =
            analyze_with_natives("let value = fn(item: 1) { item }; value", &[]).unwrap_err();
        assert!(invalid.message.contains("closure annotation is invalid"));
    }

    #[test]
    fn explicit_type_application_instantiates_complete_generic_schemes() {
        let empty = analyze_with_natives(
            "native empty: for(A) Fn() -> Array(A); empty@[Int]()",
            &[("empty", 0)],
        )
        .unwrap();
        assert_eq!(empty.display(empty.result_type), "Array<Int>");

        let pair = analyze_with_natives(
            "native pair: for(A, B) Fn(A, B) -> B;\
             pair@[Int, String](1, \"x\")",
            &[("pair", 2)],
        )
        .unwrap();
        assert_eq!(pair.display(pair.result_type), "String");

        let computed = analyze_with_natives(
            "native identity: for(A) Fn(A) -> A;\
             identity@[Array(Int)]([1, 2])",
            &[("identity", 1)],
        )
        .unwrap();
        assert_eq!(computed.display(computed.result_type), "Array<Int>");
    }

    #[test]
    fn partial_type_application_combines_rigid_and_inferred_arguments() {
        for (source, expected) in [
            (
                "native pair: for(A, B) Fn(A, B) -> Tuple([A, B]); pair@[Int, _](1, \"x\")",
                "(Int, String)",
            ),
            (
                "native pair: for(A, B) Fn(A, B) -> Tuple([A, B]); pair@[_, String](1, \"x\")",
                "(Int, String)",
            ),
            (
                "native pair: for(A, B) Fn(A, B) -> Tuple([A, B]); pair@[_, _](1, \"x\")",
                "(Int, String)",
            ),
            (
                "native empty: for(A) Fn() -> Array(A); let values: Array(Int) = empty@[_](); values",
                "Array<Int>",
            ),
            (
                "let pair = fn(left, right) { (left, right) }; pair@[Int, _](1, \"x\")",
                "(Int, String)",
            ),
        ] {
            let analysis = analyze_with_natives(source, &[("pair", 2), ("empty", 0)]).unwrap();
            assert_eq!(analysis.display(analysis.result_type), expected);
        }

        let source = "native pair: for(A, B) Fn(A, B) -> Tuple([A, B]); pair@[Int, _](1, \"x\")";
        let analysis = analyze_with_natives(source, &[("pair", 2)]).unwrap();
        let placeholder = analysis
            .hir
            .expressions()
            .iter()
            .find(|expression| {
                expression.location.range()
                    == (source.find('_').unwrap()..source.find('_').unwrap() + 1)
            })
            .expect("placeholder expression");
        assert_eq!(
            analysis.display(analysis.expression_types[&placeholder.id]),
            "String"
        );
    }

    #[test]
    fn partial_type_application_rejects_unresolved_and_conflicting_arguments() {
        let unresolved_source = "native empty: for(A) Fn() -> Array(A); empty@[_]()";
        let unresolved = analyze_with_natives(unresolved_source, &[("empty", 0)]).unwrap_err();
        assert!(
            unresolved
                .message
                .contains("cannot infer type argument `_` for parameter \"A\""),
            "{}",
            unresolved.message
        );
        assert_eq!(
            unresolved.location.offset,
            unresolved_source.find('_').unwrap()
        );

        let never = analyze_with_natives(
            "native stop: Fn() -> Never; native identity: for(A) Fn(A) -> A; identity@[_](stop())",
            &[("stop", 0), ("identity", 1)],
        )
        .unwrap_err();
        assert!(
            never.message.contains("cannot infer type argument `_`"),
            "{}",
            never.message
        );

        let explicit_any = analyze_with_natives(
            "native empty: for(A) Fn() -> Array(A); empty@[Any]()",
            &[("empty", 0)],
        )
        .unwrap();
        assert_eq!(explicit_any.display(explicit_any.result_type), "Array<Any>");

        let conflict = analyze_with_natives(
            "native identity: for(A) Fn(A) -> A; identity@[Int](\"x\")",
            &[("identity", 1)],
        )
        .unwrap_err();
        assert!(
            conflict.message.contains("cannot unify"),
            "{}",
            conflict.message
        );
    }

    #[test]
    fn explicit_type_application_rejects_bad_targets_counts_and_values() {
        for (source, expected) in [
            (
                "native pair: for(A, B) Fn(A, B) -> A; pair@[Int](1, 2)",
                "expects 2 arguments, found 1",
            ),
            (
                "native identity: for(A) Fn(A) -> A; identity@[Int, String](1)",
                "expects 1 arguments, found 2",
            ),
            (
                "let identity = fn(value: Int) { value }; identity@[Int](1)",
                "statically known generic binding",
            ),
        ] {
            let error = analyze_with_natives(source, &[("pair", 2), ("identity", 1)]).unwrap_err();
            assert!(error.message.contains(expected), "{}", error.message);
        }

        let invalid = analyze_with_natives(
            "native identity: for(A) Fn(A) -> A; identity@[1](1)",
            &[("identity", 1)],
        )
        .unwrap_err();
        assert!(
            invalid.message.contains("type argument is invalid"),
            "{}",
            invalid.message
        );
    }

    #[test]
    fn monomorphic_recursive_closures_infer_direct_mutual_and_nested_types() {
        let direct = analyze_with_natives(
            "def countdown = fn(value) {\
                 if value < 1 { 0 } else { countdown(value - 1) }\
             }; countdown",
            &[],
        )
        .unwrap();
        assert_eq!(direct.display(direct.result_type), "Fn(Int) -> Int");

        let mutual = analyze_with_natives(
            "def even = fn(value) {\
                 if value < 1 { 'True } else { odd(value - 1) }\
             };\
             def odd = fn(value) {\
                 if value < 1 { 'False } else { even(value - 1) }\
             }; (even, odd)",
            &[],
        )
        .unwrap();
        assert_eq!(
            mutual.display(mutual.result_type),
            "(Fn(Int) -> 'False | 'True, Fn(Int) -> 'False | 'True)"
        );

        let nested = analyze_with_natives(
            "{ def sum = fn(value) {\
                 if value < 1 { 0 } else { value + sum(value - 1) }\
             }; sum }",
            &[],
        )
        .unwrap();
        assert_eq!(nested.display(nested.result_type), "Fn(Int) -> Int");
    }

    #[test]
    fn acyclic_definitions_generalize_in_dependency_order() {
        for source in [
            "def identity = fn(value) { value }; (identity(1), identity(\"x\"))",
            "def identity = fn(value) { value }; def apply = fn(value) { identity(value) };\
             (apply(1), apply(\"x\"))",
            "def apply = fn(value) { identity(value) }; def identity = fn(value) { value };\
             (apply(1), apply(\"x\"))",
            "def outer = fn(value) {\
                 { def identity = fn(item) { item }; (identity(value), identity(\"x\")) }\
             }; outer(1)",
        ] {
            let analysis = analyze_with_natives(source, &[]).unwrap();
            assert_eq!(analysis.display(analysis.result_type), "(Int, String)");
        }

        let shadowed = analyze_with_natives(
            "def identity = fn(identity) { identity }; (identity(1), identity(\"x\"))",
            &[],
        )
        .unwrap();
        assert_eq!(shadowed.display(shadowed.result_type), "(Int, String)");
    }

    #[test]
    fn acyclic_definition_aliases_instantiate_once() {
        let error = analyze_with_natives(
            "def identity = fn(value) { value }; let alias = identity;\
             (alias(1), alias(\"x\"))",
            &[],
        )
        .unwrap_err();
        assert!(error.message.contains("cannot unify"), "{}", error.message);
    }

    #[test]
    fn adversarial_alias_chains_share_one_monomorphic_instance() {
        let error = analyze_with_natives(
            "let identity = fn(value) { value }; let first = identity; let second = first;\
             let number = second(1); first(\"x\")",
            &[],
        )
        .unwrap_err();
        assert!(error.message.contains("cannot unify"), "{}", error.message);
    }

    #[test]
    fn indirect_recursive_definitions_never_publish_acyclic_schemes() {
        for source in [
            "def a = fn(value) { b(value) }; let tmp = a;\
             def b = fn(value) { tmp(value) }; let number = a(1); a(\"x\")",
            "def a = fn(value) { b(value) }; let holder = {call: a};\
             def b = fn(value) { holder.call(value) }; let number = a(1); a(\"x\")",
            "def a = fn(value) { b(value) }; let make = fn() { a };\
             def b = fn(value) { make()(value) }; let number = a(1); a(\"x\")",
        ] {
            let error = analyze_with_natives(source, &[]).unwrap_err();
            assert!(
                error
                    .message
                    .contains("indirect recursive definition requires an explicit contract"),
                "{}",
                error.message
            );
        }
    }

    #[test]
    fn recursive_inference_uses_partial_and_later_evidence_but_stays_monomorphic() {
        let partial = analyze_with_natives(
            "def countdown = fn(value: Int) -> Int {\
                 if value < 1 { 0 } else { countdown(value - 1) }\
             }; countdown",
            &[],
        )
        .unwrap();
        assert_eq!(partial.display(partial.result_type), "Fn(Int) -> Int");

        let later = analyze_with_natives(
            "def bounce = fn(value) {\
                 if 'True { value } else { bounce(value) }\
             }; let number = bounce(1); bounce",
            &[],
        )
        .unwrap();
        assert_eq!(later.display(later.result_type), "Fn(Int) -> Int");

        let conflict = analyze_with_natives(
            "def bounce = fn(value) {\
                 if 'True { value } else { bounce(value) }\
             }; let number = bounce(1); bounce(\"x\")",
            &[],
        )
        .unwrap_err();
        assert!(conflict.message.contains("cannot unify String with Int"));

        let non_closure = analyze_with_natives("def value = value; value", &[]).unwrap_err();
        assert!(non_closure.message.contains("requires a closure value"));
    }

    #[test]
    fn delayed_bindings_are_solved_monomorphically_by_later_uses() {
        let direct = analyze_with_natives(
            "def identity = fn(value) { value }; let number = identity(1); identity",
            &[],
        )
        .unwrap();
        assert_eq!(direct.display(direct.result_type), "Fn(Any) -> Any");
        assert_eq!(direct.display(direct.binding_types["number"]), "Int");

        let alias = analyze_with_natives(
            "def identity = fn(value) { value }; let alias = identity;\
             let number = alias(1); identity",
            &[],
        )
        .unwrap();
        assert_eq!(alias.display(alias.result_type), "Fn(Any) -> Any");

        let callback = analyze_with_natives(
            "native map: for(A, B) Fn(Array(A), Fn(A) -> B) -> Array(B);\
             def identity = fn(value) { value };\
             let mapped = map([1, 2], identity); identity",
            &[("map", 2)],
        )
        .unwrap();
        assert_eq!(callback.display(callback.result_type), "Fn(Any) -> Any");

        let field = analyze_with_natives(
            "let holder = {apply: fn(value) { value }};\
             let number = holder.apply(1); holder.apply",
            &[],
        )
        .unwrap();
        assert_eq!(field.display(field.result_type), "Fn(Int) -> Int");

        let empty = analyze_with_natives(
            "native append: for(A) Fn(Array(A), A) -> Array(A);\
             let values = []; let appended = append(values, 1); values",
            &[("append", 2)],
        )
        .unwrap();
        assert_eq!(empty.display(empty.result_type), "Array<Int>");
    }

    #[test]
    fn delayed_bindings_reject_conflicts_and_underconstrained_results() {
        let independent = analyze_with_natives(
            "def identity = fn(value) { value };\
             let number = identity(1); identity(\"text\")",
            &[],
        )
        .unwrap();
        assert_eq!(independent.display(independent.result_type), "String");

        let error = analyze_with_natives("let values = []; values", &[]).unwrap_err();
        assert!(
            error.message.contains("cannot infer monomorphic binding"),
            "{}",
            error.message
        );

        let explicit = analyze_with_natives(
            "let identity: Fn(Any) -> Any = fn(value) { value }; identity",
            &[],
        )
        .unwrap();
        assert_eq!(explicit.display(explicit.result_type), "Fn(Any) -> Any");

        let recursive =
            analyze_with_natives("def recurse = fn(value) { recurse(value) }; recurse", &[]);
        assert!(recursive.is_err());
    }

    #[test]
    fn eligible_let_closures_generalize_and_instantiate_independently() {
        let analysis = analyze_with_natives(
            "let identity = fn(value) { value };\
             let wrap = fn(value) { [value] };\
             (identity(1), identity(\"text\"), wrap(2), wrap(\"value\"))",
            &[],
        )
        .unwrap();
        assert_eq!(
            analysis.display(analysis.result_type),
            "(Int, String, Array<Int>, Array<String>)"
        );

        let explicit =
            analyze_with_natives("let identity = fn(value) { value }; identity@[Int](1)", &[])
                .unwrap();
        assert_eq!(explicit.display(explicit.result_type), "Int");

        let exported = analyze_with_natives(
            "let identity = fn(value) { value }; {identity: identity}",
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
    }

    #[test]
    fn local_generalization_respects_annotations_aliases_constraints_and_scopes() {
        let partial = analyze_with_natives(
            "let keep = fn(left: Int, right) { (left, right) };\
             (keep(1, \"x\"), keep(2, 3))",
            &[],
        )
        .unwrap();
        assert_eq!(
            partial.display(partial.result_type),
            "((Int, String), (Int, Int))"
        );

        let captures = analyze_with_natives(
            "native append: for(A) Fn(Array(A), A) -> Array(A);\
             let values = [];\
             let pair = fn(value) { (values, value) };\
             let first = pair(1); let second = pair(\"x\");\
             let appended = append(values, 2); (first, second, appended)",
            &[("append", 2)],
        )
        .unwrap();
        assert_eq!(
            captures.display(captures.result_type),
            "((Array<Int>, Int), (Array<Int>, String), Array<Int>)"
        );

        let alias = analyze_with_natives(
            "let identity = fn(value) { value }; let alias = identity;\
             (alias(1), alias(\"text\"))",
            &[],
        )
        .unwrap_err();
        assert!(alias.message.contains("cannot unify String with Int"));

        let numeric =
            analyze_with_natives("let negate = fn(value) { -value }; negate", &[]).unwrap_err();
        assert!(numeric.message.contains("cannot infer monomorphic binding"));

        let nested = analyze_with_natives(
            "let identity = fn(value) { value };\
             ({ let identity = fn(value) { [value] }; identity(1) }, identity(\"x\"))",
            &[],
        )
        .unwrap();
        assert_eq!(nested.display(nested.result_type), "(Array<Int>, String)");

        let rigid_capture = analyze_with_natives(
            "def outer: for(Outer) Fn(Outer) -> Any = fn(value) {\
                 let pair = fn(other) { (value, other) };\
                 (pair(1), pair(\"x\"))\
             }; outer(0)",
            &[],
        )
        .unwrap();
        let pair = rigid_capture
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "pair")
            .unwrap();
        assert_eq!(
            rigid_capture.definition_schemes[&pair.id].display_name(),
            "for(A) Fn(A) -> (T0, A)"
        );
    }

    #[test]
    fn generic_contract_parameters_are_available_in_implementation_annotations() {
        let analysis = analyze_with_natives(
            "type Pair(Left, Right) = struct {left: Left, right: Right};\
             type Box(Content) = struct {value: Content};\
             def collect: for(N, M) Fn(Array(Box(Pair(N, M)))) -> Array(Box(Pair(N, M))) = fn(items) {\
                 let result: Array(Box(Pair(N, M))) = items;\
                 let retain = fn(values: Array(Box(Pair(N, M)))) -> Array(Box(Pair(N, M))) { values };\
                 retain(result)\
             };\
             collect",
            &[],
        )
        .unwrap();
        let collect = analysis
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "collect")
            .unwrap();
        assert_eq!(
            analysis.definition_schemes[&collect.id].display_name(),
            "for(N, M) Fn(Array<Box>) -> Array<Box>"
        );

        let leaked = analyze_with_natives(
            "def identity: for(N) Fn(N) -> N = fn(value) {\
                 let result: N = value; result\
             };\
             let unrelated: N = 1; unrelated",
            &[],
        )
        .unwrap_err();
        assert!(
            leaked.message.contains("unknown binding \"N\""),
            "{}",
            leaked.message
        );
    }

    #[test]
    fn nested_closures_share_only_body_constraints() {
        let analysis = analyze_with_natives(
            "let nested = fn(left) { fn(right) { left + right + 1 } }; nested",
            &[],
        )
        .unwrap();
        assert_eq!(
            analysis.display(analysis.result_type),
            "Fn(Int) -> Fn(Int) -> Int"
        );
    }

    #[test]
    fn ordinary_expressions_use_bidirectional_checking_without_schemes() {
        let analysis = analyze_with_natives(
            "let values: Array(Int) = if 'True { [] } else { [1] };\
             let selected: Int = match (1, \"x\") { (number, _) => number };\
             (values, selected)",
            &[],
        )
        .unwrap();
        assert_eq!(analysis.display(analysis.result_type), "(Array<Int>, Int)");

        let error = analyze_with_natives(
            "def broken: Fn(Int) -> Int = fn(value) { value + \"x\" }; broken",
            &[],
        )
        .unwrap_err();
        assert!(error.message.contains("cannot unify String with Int"));

        let nested =
            analyze_with_natives("let outer = { let value: Int = \"x\"; value }; outer", &[])
                .unwrap_err();
        assert!(nested.message.contains("cannot unify String with Int"));
    }

    #[test]
    fn generic_native_parameters_must_be_unique() {
        let error = analyze_with_natives(
            "native identity: for(A, A) Fn(A) -> A; identity(1)",
            &[("identity", 1)],
        )
        .unwrap_err();
        assert!(error.message.contains("duplicate type parameter"));

        let leaked =
            analyze_with_natives("native identity: for(A) Fn(A) -> A; A", &[("identity", 1)])
                .unwrap_err();
        assert!(leaked.message.contains("unknown binding \"A\""));
    }

    #[test]
    fn generic_native_schemes_are_data_and_occurs_checks_reject_infinite_types() {
        let analysis = analyze_with_natives(
            "native identity: for(A) Fn(A) -> A; {identity: identity}",
            &[("identity", 1)],
        )
        .unwrap();
        let scheme = &analysis.module_interface.exports["identity"];
        assert_eq!(scheme.parameters[0].name, "A");
        assert!(matches!(
            &scheme.body,
            TypeDescriptor::Function { parameters, result }
                if parameters == &[TypeDescriptor::Bound(TypeParameterId(0))]
                    && **result == TypeDescriptor::Bound(TypeParameterId(0))
        ));

        let schemes = HashMap::new();
        let interfaces = BTreeMap::new();
        let annotations = HashMap::new();
        let dyn_namespaces = HashSet::new();
        let named_types = BTreeMap::new();
        let trait_ids = BTreeMap::new();
        let hir = HirProgram::default();
        let mut inference = GenericInference::new(
            &schemes,
            &hir,
            &interfaces,
            &named_types,
            &annotations,
            &[],
            &[],
            &trait_ids,
            None,
            &dyn_namespaces,
            true,
            None,
        );
        let variable = TypeDescriptor::Inference(InferenceVariableId(0));
        assert!(
            inference
                .unify(
                    &variable,
                    &TypeDescriptor::Array(Box::new(variable.clone()))
                )
                .unwrap_err()
                .contains("infinite type")
        );
    }

    #[test]
    fn published_schemes_reject_solver_and_unbound_parameter_identities() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("scheme.telora", "");
        let location = crate::Location::from_usize(source, 0..0).unwrap();
        let valid = TypeScheme {
            parameters: vec![TypeParameter {
                id: TypeParameterId(0),
                name: "A".into(),
                location,
            }],
            constraints: Vec::new(),
            body: TypeDescriptor::Function {
                parameters: vec![TypeDescriptor::Bound(TypeParameterId(0))],
                result: Box::new(TypeDescriptor::Bound(TypeParameterId(0))),
            },
        };
        assert!(validate_publishable_scheme(&valid).is_ok());

        let unresolved = TypeScheme {
            parameters: Vec::new(),
            constraints: Vec::new(),
            body: TypeDescriptor::Inference(InferenceVariableId(0)),
        };
        assert!(
            validate_publishable_scheme(&unresolved)
                .unwrap_err()
                .contains("unresolved")
        );

        let unbound = TypeScheme {
            parameters: Vec::new(),
            constraints: Vec::new(),
            body: TypeDescriptor::Bound(TypeParameterId(7)),
        };
        assert!(
            validate_publishable_scheme(&unbound)
                .unwrap_err()
                .contains("unbound parameter T7")
        );

        let unbound_constraint = TypeScheme {
            parameters: Vec::new(),
            constraints: vec![TypeConstraint {
                parameter: TypeParameterId(7),
                capability: TypeCapability::Property(TypeDescriptor::Int),
                location,
            }],
            body: TypeDescriptor::Int,
        };
        assert!(
            validate_publishable_scheme(&unbound_constraint)
                .unwrap_err()
                .contains("constraint references unbound parameter T7")
        );
    }

    #[test]
    #[should_panic(expected = "solver descriptors must be explicitly erased before interning")]
    fn strict_type_graph_interning_rejects_solver_descriptors() {
        TypeGraph::default().intern_descriptor(&TypeDescriptor::Inference(InferenceVariableId(0)));
    }

    #[test]
    fn explicit_runtime_erasure_is_the_only_solver_to_any_path() {
        let mut types = TypeGraph::default();
        let erased = types.intern_erased_descriptor(&TypeDescriptor::Function {
            parameters: vec![TypeDescriptor::Bound(TypeParameterId(0))],
            result: Box::new(TypeDescriptor::Inference(InferenceVariableId(0))),
        });
        assert_eq!(types.display(erased), "Fn(Any) -> Any");
    }

    #[test]
    fn metadata_round_trips() {
        fn round_trip(descriptor: &TypeDescriptor) {
            let mut heap = Heap::work();
            let value = heap.type_descriptor_value(None, descriptor).unwrap();
            let world = crate::DataWorld::new(heap, value);
            assert_eq!(decode_type_ref(world.value(), "Type").unwrap(), *descriptor);
        }

        let descriptor = TypeDescriptor::Function {
            parameters: vec![TypeDescriptor::Struct(BTreeMap::from([
                ("age".into(), TypeDescriptor::Int),
                ("name".into(), TypeDescriptor::String),
            ]))],
            result: Box::new(TypeDescriptor::Enum(BTreeMap::from([
                ("None".into(), None),
                ("Some".into(), Some(Box::new(TypeDescriptor::String))),
            ]))),
        };
        round_trip(&descriptor);

        let bound = TypeDescriptor::Array(Box::new(TypeDescriptor::Bound(TypeParameterId(7))));
        round_trip(&bound);

        let metatype = TypeDescriptor::Type;
        round_trip(&metatype);

        let never = TypeDescriptor::Never;
        round_trip(&never);

        round_trip(&TypeDescriptor::AtomValue);

        let witness = TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Array(Box::new(
            TypeDescriptor::Int,
        ))));
        round_trip(&witness);
    }

    #[test]
    fn metadata_values_and_typed_constructors_have_the_type_metatype() {
        let analysis = analyze_source(
            "metatype.telora",
            "def Maybe: for(A) Fn(TypeOf(A)) -> TypeOf(Option(A)) = fn(Item) { Option(Item) };\
             type MaybeInt = Maybe(Int);\
             (Type, Int, Array(Int), Maybe)",
        )
        .unwrap();
        assert_eq!(
            analysis.display(analysis.result_type),
            "(TypeOf(Type), TypeOf(Int), TypeOf(Array<Int>), Fn(TypeOf(Any)) -> TypeOf(enum {None, Some(Any)}))"
        );
        let maybe_int = analysis
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "MaybeInt")
            .expect("MaybeInt definition");
        assert_eq!(
            analysis.display(analysis.definition_types[&maybe_int.id]),
            "TypeOf(enum {None, Some(Int)})"
        );
        assert!(matches!(
            analysis.types.node(analysis.declared_types["MaybeInt"]),
            TypeNode::Enum(_)
        ));

        let bad_argument = analyze_source(
            "bad-argument.telora",
            "def Broken: Fn(Type) -> Type = fn(Item) { Array(1) }; Broken",
        )
        .unwrap_err();
        assert!(bad_argument.message.contains("cannot unify Int with Type"));

        let bad_result = analyze_source(
            "bad-result.telora",
            "def Broken: Fn(Type) -> Type = fn(Item) { 1 }; Broken",
        )
        .unwrap_err();
        assert!(bad_result.message.contains("cannot unify Int with Type"));
    }

    #[test]
    fn fn_notation_and_func_constructor_share_canonical_metadata() {
        let definitions = analyze_source(
            "definitions.telora",
            "def make: Fn(Int) -> Tuple([Int, String]) = fn(value) { (value, \"ok\") };\
             decl copy: Fn(Int) -> Tuple([Int, String]);\
             def copy = make;\
             (make(1), copy(2))",
        )
        .unwrap();
        assert_eq!(
            definitions.display(definitions.result_type),
            "((Int, String), (Int, String))"
        );

        let native = analyze_with_natives(
            "native convert: Fn(Int) -> Array(Tuple([String, Int])); convert(1)",
            &[("convert", 1)],
        )
        .unwrap();
        assert_eq!(native.display(native.result_type), "Array<(String, Int)>");

        let explicit = analyze_source(
            "explicit.telora",
            "type ViaSyntax = Func([Int], String);\
             def value: ViaSyntax = fn(number) { if number == 0 { \"zero\" } else { \"other\" } };\
             value",
        )
        .unwrap();
        assert_eq!(
            explicit.display(explicit.declared_types["ViaSyntax"]),
            "Fn(Int) -> String"
        );
    }

    #[test]
    fn tuple_contracts_do_not_rewrite_constructor_arity() {
        let error = analyze_with_natives(
            "native invalid: Fn(Int) -> Tuple(Int, String); invalid(1)",
            &[("invalid", 1)],
        )
        .unwrap_err();
        assert!(
            error.message.contains("expected 1 arguments, got 2"),
            "{error}"
        );
    }

    #[test]
    fn function_remains_available_as_a_domain_type_name() {
        let analysis = analyze_source(
            "domain-name.telora",
            "type Function = Int; let value: Function = 1; value",
        )
        .unwrap();
        assert_eq!(analysis.display(analysis.result_type), "Int");
    }
