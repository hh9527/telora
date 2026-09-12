use super::*;
#[test]
fn interpreter_adapters_use_solved_plans_and_preserve_factory_identity() {
    for source in [
        r#"import "std/dyn" as dyn;
            type Witness(T) = TypeOf(T);
            def erased: Fn(Int, Dyn, Bool, Dyn, Dyn) -> Int = fn(offset, text, enabled, number, again) {
                if enabled && dyn.check_string(text) == Some("a") && dyn.check_string(again) == Some("b") {
                    match dyn.check_int(number) { Some(value) => offset + value, None => 0 }
                } else { 0 }
            };
            def factory: for(A, B) Fn(Witness(B), Witness(A)) -> Fn(Int, A, Bool, B, A) -> Int = interpreter!(erased);
            export def answer = factory(Int.type, String.type)(2, "a", True, 40, "b");"#,
        r#"def erased: Fn(Dyn) -> Int = fn(value) { 42 };
            def first: for(T) Fn(TypeOf(T)) -> Fn(T) -> Int = interpreter!(erased);
            def second: for(T) Fn(TypeOf(T)) -> Fn(T) -> Int = interpreter!(erased);
            export def answer = do { let a = first(Int.type); let b = first(Int.type); let c = second(Int.type);
                if a == b && a != c { a(0) } else { 0 }
            };"#,
        r#"def operand: Fn(Dyn) -> Int = fn(value) { fail!("operand body must remain lazy") };
            def factory: for(T) Fn(TypeOf(T)) -> Fn(T) -> Int = interpreter!(operand);
            export def answer = do { let adapter = factory(Int.type); 42 };"#,
        r#"def run: Fn(Int) -> Int = fn(offset) {
                def factory: for(T) Fn(TypeOf(T)) -> Fn(T) -> Int = interpreter!(fn(value) { offset });
                factory(String.type)("ignored")
            }; export def answer = run(42);"#,
        r#"def factory: for(T) Fn(TypeOf(T)) -> Fn(Int) -> Int = interpreter!(fn(value) { value });
            export def answer = factory(String.type)(42);"#,
    ] {
        let mir = graph(source, "");
        let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
        let result = execute(artifact).unwrap_or_else(|error| panic!("{source}\n{error}"));
        assert_eq!(result.value().as_int(), Some(42), "{source}");
    }
}

#[test]
fn decoded_container_fields_preserve_their_solved_types() {
    let mir = graph(r#"import "std/codec" as codec;
        type Model = struct {labels: Dict(String), values: Array(Int), pair: (Int, String), selected: Option(Int), missing: Option(Int)};
        export def answer = do {
            let original: Model = {labels: {role: "admin"}, values: [1, 2], pair: (3, "four"), selected: Some(5), missing: None};
            let decoded = codec.decode(Model.type, codec.encode(codec.Value.type, original)).unwrap!();
            (if decoded == original { 42 } else { 0 }, decoded, original)
        };"#, "");
    let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
    let result = execute(artifact).unwrap();
    let value = result.value();
    let decoded = value.sequence_get(1).unwrap();
    let original = value.sequence_get(2).unwrap();
    for field in ["labels", "values", "pair", "selected", "missing"] {
        let expected = original.dict_get(field).unwrap().solved_type_id();
        assert_eq!(expected.is_some(), field == "labels", "{field}");
        assert_eq!(decoded.dict_get(field).unwrap().solved_type_id(), expected, "{field}");
    }
    assert_eq!(value.sequence_get(0).unwrap().as_int(), Some(42));
}

#[test]
fn empty_option_variants_close_without_context() {
    for body in [
        "export def answer = if option.is_some(None) { 0 } else { 42 };",
        "import \"std/prelude\" {None as absent}; def renamed = absent; export def answer = if option.is_some(renamed) { 0 } else { 42 };",
        "export def answer = if option.is_some(Option.None) { 0 } else { 42 };",
        "export def answer = do { let empty: Option(Int) = None; option.unwrap_or(empty, 42) };",
    ] {
        let mir = graph(&format!("import \"std/option\" as option; {body}"), "");
        let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{body}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
        assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
    }
}

#[test]
fn temporal_decoding_obeys_payload_types_and_construction_checks() {
    let mir = graph(r#"import "std/codec" as codec; import "std/_rt" as rt;
        type WrongPayload = enum {LocalDate(Int)};
        type Missing = enum {Other(String)};
        @check(fn(value) { Err(blame!("rejected date", value)) }) type DateText = struct(String);
        type Checked = enum {LocalDate(DateText)};
        export def answer = do {
            let raw = codec.Value.LocalDate("2026-08-04");
            let wrong = match codec.decode(WrongPayload.type, raw) { Err(_) => True, _ => False };
            let missing = match codec.decode(Missing.type, raw) { Err(_) => True, _ => False };
            let checked = match rt.with_diagnostics(fn(n: Int) { codec.decode(Checked.type, raw).unwrap!() })(0) {
                Err(errors) => errors[0].message == "rejected date", _ => False
            };
            if wrong && missing && checked { 42 } else { 0 }
        };"#, "");
    let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
    assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
}

#[test]
fn solved_temporal_data_decodes_into_declared_enum_payloads() {
    let mir = graph(r#"import "std/toml" as toml; import "std/codec" as codec;
        type Date = enum {LocalDate(String), LocalTime(String), LocalDateTime(String), OffsetDateTime(String)};
        type Config = struct {date: Date, time: Date, local: Date, offset: Date};
        export def answer = do {
            let raw = toml.parse("date = 2026-08-04\ntime = 07:32:00\nlocal = 2026-08-04T07:32:00\noffset = 2026-08-04T07:32:00Z").unwrap!();
            let config = codec.decode(Config.type, raw).unwrap!();
            if config.date == Date.LocalDate("2026-08-04")
                && config.time == Date.LocalTime("07:32:00")
                && config.local == Date.LocalDateTime("2026-08-04T07:32:00")
                && config.offset == Date.OffsetDateTime("2026-08-04T07:32:00Z") { 42 } else { 0 }
        };"#, "");
    let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
    assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
}

#[test]
fn imported_generic_parameters_complete_unchecked_arguments() {
    for body in [
        r#"export def answer = do { let candidate: Unchecked(Box(Int)) = {value: 42, count: 2}; read(candidate) };"#,
        r#"export def answer = match rt.with_diagnostics(fn(n: Int) {
            let candidate: Unchecked(Box(Int)) = {value: n, count: 0}; read(candidate)
        })(42) { Err(errors) => if errors[0].message == "count below minimum" { 42 } else { 0 }, _ => 0 };"#,
        r#"export def answer = do { let candidate: Unchecked(Box(Int)) = {value: 42, count: 0};
            let copy = identity(candidate);
            match dyn.project_with(Box(Int).type, dyn.pack(Unchecked(Box(Int)).type, copy)) { None => copy.value, _ => 0 }
        };"#,
        r#"export def answer = match rt.with_diagnostics(fn(n: Int) {
            let candidate: Unchecked(Box(Int)) = {value: n, count: 0}; dyn.pack(Box(Int).type, candidate)
        })(42) { Err(errors) => if errors[0].message == "count below minimum" { 42 } else { 0 }, _ => 0 };"#,
    ] {
        let mir = graph(&format!(r#"import "./math" {{Box}}; import "std/_rt" as rt; import "std/dyn" as dyn;
            def read: for(T) Fn(Box(T)) -> T = fn(value) {{ value.value }};
            def identity: for(T) Fn(T) -> T = fn(value) {{ value }}; {body}"#),
            r#"@check(fn(value) { if value.count >= 1 { Ok(()) } else { Err(blame!("count below minimum", value.count)) } })
            type Box(T) = struct {value: T, count: Int}; export {Box};"#);
        let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{body}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
        assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42), "{body}");
    }
}

#[test]
fn local_struct_update_chains_keep_their_nominal_type() {
    let mir = graph(r#"import "./math" {Point};
        export def answer = do { let point: Point = {x: 1}; let updated = point <~ {x: 2} <~ {x: 42}; updated.x };"#,
        r#"@check(fn(value) { let warning: Option(()) = warn!(blame!("checked", value.x)); Ok(()) }) type Point = struct {x: Int}; export {Point};"#);
    let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
    let result = execute(artifact).unwrap();
    assert_eq!(result.value().as_int(), Some(42));
}

#[test]
fn json_schema_reports_recoverable_mapping_and_property_errors() {
    for (declaration, target, expected) in [
        ("", "Type", "JSON Schema cannot describe Type metadata"),
        ("", "Bytes", "Type Bytes has no JSON Schema mapping"),
        ("", "Fn(Int) -> Int", "Type Func has no JSON Schema mapping"),
        ("@json.rename_all(json.RenameCase.CamelCase) type Bad = struct {foo_bar: Int, fooBar: Int};", "Bad", "$.fooBar: duplicate external field name"),
        ("@json.untagged type Bad = enum {One, Two};", "Bad", "$: untagged Enum may contain at most one unit variant"),
        ("import \"std/string\" as string; @string.encode_by_display type Bad = struct(Int);", "Bad", "std/string.decode_by_parse and std/string.encode_by_display must be used together"),
    ] {
        let source = format!("import \"std/json\" as json; import \"std/_rt\" as rt; {declaration} export def answer = match rt.with_diagnostics(fn(n: Int) {{ json.schema(({target}).type) }})(0) {{ Err(errors) => errors[0].message, _ => \"unexpected success\" }};");
        let mir = graph(&source, "");
        let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
        let result = execute(artifact).unwrap_or_else(|e| panic!("{source}\n{e}"));
        assert_eq!(result.value().as_str().unwrap().as_str(), expected, "{source}");
    }
}

#[test]
fn json_schema_consumes_solved_layouts_and_lazy_properties() {
    for (declarations, target, expected) in [
        ("", "Int", r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"integer"}"#),
        ("type Id = struct(Int);", "Id", r##"{"$defs":{"Type0":{"type":"integer"}},"$ref":"#/$defs/Type0","$schema":"https://json-schema.org/draft/2020-12/schema"}"##),
        ("@check(fn(value) { fail!(\"schema must not run construction checks\") }) type Id = struct(Int);", "Id", r##"{"$defs":{"Type0":{"type":"integer"}},"$ref":"#/$defs/Type0","$schema":"https://json-schema.org/draft/2020-12/schema"}"##),
        ("type Node = struct {value: Int, next: Option(Node)};", "Node", r##"{"$defs":{"Type0":{"additionalProperties":false,"properties":{"next":{"anyOf":[{"type":"null"},{"$ref":"#/$defs/Type0"}]},"value":{"type":"integer"}},"required":["value"],"type":"object"}},"$ref":"#/$defs/Type0","$schema":"https://json-schema.org/draft/2020-12/schema"}"##),
        ("", "(Int, String)", r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","maxItems":2,"minItems":2,"prefixItems":[{"type":"integer"},{"type":"string"}],"type":"array"}"#),
        ("@json.rename_all(json.RenameCase.CamelCase) type User = struct {user_name: String};", "User", r##"{"$defs":{"Type0":{"additionalProperties":false,"properties":{"userName":{"type":"string"}},"required":["userName"],"type":"object"}},"$ref":"#/$defs/Type0","$schema":"https://json-schema.org/draft/2020-12/schema"}"##),
        ("@json.untagged type Scalar = enum {Text(String), Empty};", "Scalar", r##"{"$defs":{"Type0":{"oneOf":[{"type":"null"},{"type":"string"}]}},"$ref":"#/$defs/Type0","$schema":"https://json-schema.org/draft/2020-12/schema"}"##),
        ("", "Result(Int, String)", r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","oneOf":[{"additionalProperties":false,"properties":{"Err":{"type":"string"}},"required":["Err"],"type":"object"},{"additionalProperties":false,"properties":{"Ok":{"type":"integer"}},"required":["Ok"],"type":"object"}]}"#),
        ("import \"std/string\" as string; @string.decode_by_parse @string.encode_by_display type Text = struct(Int);", "Text", r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"string"}"#),
    ] {
        let source = format!("import \"std/json\" as json; {declarations} def schema = json.schema(({target}).type); def text = json.stringify(schema); def parsed = match json.parse(text) {{ Ok(value) => value, Err(error) => raise!(error) }}; export def answer = (text, schema == parsed);");
        let mir = graph(&source, "");
        let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
        drop(mir);
        let result = execute(artifact).unwrap_or_else(|e| panic!("{source}\n{e}"));
        assert_eq!(result.value().sequence_get(0).unwrap().as_str().unwrap().as_str(), expected, "{source}");
        assert_eq!(result.value().sequence_get(1).unwrap().as_atom().unwrap().as_str(), "True", "{source}");
    }
}

#[test]
fn newtype_facets_and_patterns_consume_static_constructor_selections() {
    for body in [
        "let make = Box; (make(42).0, make(\"text\").0); 42",
        "let make: Fn(Int) -> Id = Id; make(42).0",
        "def make: for(T) Fn(T) -> Box(T) = Box; make@[Int](42).0",
        "let Wrapped(payload) = Wrapped(42); payload",
        "let wrapper: Box(Id) = Box(Id(42)); wrapper.0.0",
    ] {
        let mir = graph(&format!("type Id = struct(Int); type Box(T) = struct(T); type Wrapped = Id; export def answer = do {{ {body} }};"), "");
        let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{body}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
        drop(mir);
        let result = execute(artifact).unwrap_or_else(|e| panic!("{body}\n{e}"));
        assert_eq!(result.value().as_int(), Some(42), "{body}");
    }
    let mut mir = graph("type Id = struct(Int); export def answer = Id(42);", "");
    mir.seal().unwrap();
    let selection = mir.member_selections.iter().position(|selection| matches!(selection, Some(MemberSelection::NewtypeConstructor))).unwrap();
    let TypeState::Known(signature) = mir.ty_slots[selection] else { unreachable!() };
    let wrong_payload = mir.types[signature.index()].arguments[1];
    mir.types[signature.index()].arguments[0] = wrong_payload;
    assert!(mir.seal().is_err(), "a constructor signature must agree with its sealed payload layout");
}

#[test]
fn dyn_projection_uses_ordinary_generic_bindings_and_solved_witnesses() {
    for source in [
        r#"import "std/dyn" as dyn;
            export def answer = match dyn.project@[Int](dyn.pack(Int.type, 42)) { Some(value) => value, None => 0 };"#,
        r#"import "std/dyn" {project as unpack, pack};
            export def answer = match unpack@[Int](pack(Int.type, 42)) { Some(value) => value, None => 0 };"#,
        r#"import "std/dyn" as dyn;
            def unpack: for(T) Fn(Dyn) -> Option(T) = fn(value) { dyn.project@[T](value) };
            export def answer = match unpack@[Int](dyn.pack(Int.type, 42)) { Some(value) => value, None => 0 };"#,
        r#"import "std/dyn" as dyn;
            def unpack: Fn(Dyn) -> Option(Int) = dyn.project;
            export def answer = match unpack(dyn.pack(Int.type, 42)) { Some(value) => value, None => 0 };"#,
        r#"import "std/dyn" as dyn; type A = struct {value: Int}; type B = struct {value: Int};
            def value: A = {value: 1};
            export def answer = if dyn.project@[B](dyn.pack(A.type, value)) == None { 42 } else { 0 };"#,
        r#"import "./math" as user;
            export def answer = user.project@[Int](42);"#,
    ] {
        let mir = graph(source, "export def project: for(T) Fn(T) -> T = fn(value) { value };");
        let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
        drop(mir);
        let result = execute(artifact).unwrap_or_else(|e| panic!("{source}\n{e}"));
        assert_eq!(result.value().as_int(), Some(42), "{source}");
    }
}

#[test]
fn property_target_enum_computes_matches_reflects_and_reduces_all_categories() {
    let mir = graph(r#"
        import "std/type-property" as props;
        import "std/type-desc" as td;
        import "std/dyn" as dyn;
        import PropertyTarget.{Member as Both};
        def choose: Fn(Bool) -> PropertyTarget = fn(flag) { if flag { PropertyTarget.StructType } else { PropertyTarget.EnumType } };
        @property(PropertyTarget.Type) @property(choose(True)) @property(choose(False))
        @property(Both) @property(PropertyTarget.Field) @property(PropertyTarget.Variant)
        type Mark = struct {value: Int};
        def name: Fn(PropertyTarget) -> String = fn(value) { match value {
            PropertyTarget.Type => "type", PropertyTarget.StructType => "struct",
            PropertyTarget.EnumType => "enum", Both => "member",
            PropertyTarget.Field => "field", PropertyTarget.Variant => "variant",
        } };
        export def answer = match props.get_type_prop(Mark.type, PropertyAttr.type) {
            Some(attr) => if attr.bits == 63 && name(choose(False)) == "enum" && name(Both) == "member"
                && td.kind(PropertyTarget.type) == td.TypeDescKind.Enum
                && td.variants(PropertyTarget.type)[2].name == "Member"
                && dyn.get_variant_index(dyn.pack(PropertyTarget.type, PropertyTarget.Variant)) == 5 { 42 } else { 0 },
            None => 0,
        };
    "#, "");
    let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
    let result = execute(artifact).unwrap();
    assert_eq!(result.value().as_int(), Some(42));
}

#[test]
fn record_construction_accepts_context_across_local_bindings() {
    for body in [
        "let raw = {value: 42}; [raw] == [item] && [item] == [raw] && (raw, 1) == (item, 1)",
        "let raw = [{value: 42}]; [...raw] == [item] && [item] == [...raw] && [...[{value: 42}]] == [item]",
        "let raw = [{value: 42}]; choose(raw, [item]) == [item] && choose_array(raw, item) == [item]",
        "choose_independent([{value: 42}], [item]) == [item] && choose([{value: 42}], [item]) == [item]",
        "let raw = if True { {value: 42} } else { {value: 42} }; [raw] == [item] && [{value: 42}] == [item]",
    ] {
        let mir = graph(&format!("type Item = struct {{value: Int}}; def item: Item = {{value: 42}}; def choose: for(T) Fn(T, T) -> T = fn(left, right) {{left}}; def choose_array: for(T) Fn(Array(T), T) -> Array(T) = fn(left, right) {{left}}; def choose_independent: for(A, B) Fn(A, B) -> A = fn(left, right) {{left}}; export def answer = {{ {body} }};"), "");
        let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{body}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
        let result = execute(artifact).unwrap();
        assert_eq!(result.value().as_atom().unwrap().as_str(), "True", "{body}");
    }
}

#[test]
fn encoded_enum_values_equal_explicit_semantic_value_constructors() {
    let mir = graph("import \"std/codec\" as codec; import \"std/value\" { Value }; type Event = enum { Progress(Int), Finished }; def encoded = codec.encode(Value.type, Event.Progress(47)); def expected = Value.Object({Progress: Value.Int(47)}); export def answer = (encoded == expected, encoded, expected);", "");
    let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    let result = execute(artifact).unwrap();
    let encoded = result.value().sequence_get(1).unwrap();
    let expected = result.value().sequence_get(2).unwrap();
    assert_eq!(encoded.solved_type_id(), expected.solved_type_id());
    let (encoded_tag, encoded_fields) = encoded.tagged_parts().unwrap();
    let (expected_tag, expected_fields) = expected.tagged_parts().unwrap();
    assert_eq!(encoded_tag.as_atom(), expected_tag.as_atom());
    assert_eq!(encoded_fields.solved_type_id(), expected_fields.solved_type_id());
    let encoded_number = encoded_fields.dict_get("Progress").unwrap();
    let expected_number = expected_fields.dict_get("Progress").unwrap();
    assert_eq!(encoded_number.solved_type_id(), expected_number.solved_type_id());
    assert_eq!(encoded_number.tagged_parts().unwrap().1.runtime(), expected_number.tagged_parts().unwrap().1.runtime());
    assert_eq!(result.value().sequence_get(0).unwrap().as_atom().unwrap().as_str(), "True");
}

#[test]
fn encoded_object_payload_witnesses_cover_nested_and_dictionary_outputs() {
    for (source, expected) in [
        ("{a: 47}.ty!(Dict(Int))", "Value.Object({a: Value.Int(47)})"),
        ("{let value: Dict(Int) = {a: 47}; value}", "Value.Object({a: Value.Int(47)})"),
        ("[{a: 47}].ty!(Array(Dict(Int)))", "Value.Array([Value.Object({a: Value.Int(47)})])"),
        ("{a: {b: 47}}.ty!(Dict(Dict(Int)))", "Value.Object({a: Value.Object({b: Value.Int(47)})})"),
        ("Event.Finished", "Value.String(\"Finished\")"),
    ] {
        let mir = graph(&format!("import \"std/codec\" as codec; import \"std/value\" {{ Value }}; type Event = enum {{ Finished }}; export def answer = codec.encode(Value.type, {source}) == {expected};"), "");
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        let result = execute(artifact).unwrap();
        assert_eq!(result.value().as_atom().unwrap().as_str(), "True", "{source}");
    }
}

#[test]
fn metadata_comparisons_execute_without_equating_type_witnesses() {
    for source in [
        "export def answer = if Int.type != String.type && Int.type == Int.type { 42 } else { 0 };",
        "type Choice = enum { Selected(Type), Empty }; def chosen = match Choice.Selected(Int.type) { Choice.Selected(value) => value, Choice.Empty => String.type }; export def answer = if chosen == Int.type { 42 } else { 0 };",
        "def matches = fn(value) { value == Int.type }; export def answer = if matches(Int.type) && !matches(String.type) { 42 } else { 0 };",
    ] {
        let mir = graph(source, "");
        let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
        let result = execute(artifact).unwrap();
        assert_eq!(result.value().as_int(), Some(42), "{source}");
    }
}

#[test]
fn nullary_constructor_aliases_have_independent_closed_owners() {
    for source in [
        "import \"./math\" { Empty }; export def answer = (Empty@[Int], Empty@[String]);",
        "import \"./math\" { Message }; export def answer = { import Message.{Empty}; (Empty@[Int], Empty@[String]) };",
        "export def answer = { import Option.{None as Empty}; (Empty@[Int], Empty@[String]) };",
    ] {
        let mir = graph(source, "export type Message(T) = enum { Data(T), Empty }; import Message.{Empty}; export { Empty };");
        let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
        let types = artifact.types.types[artifact.result_type.index()].arguments.clone();
        assert_ne!(types[0], types[1]);
        let nominal = matches!(artifact.types.types[types[0].index()].constructor, TypeConstructor::Nominal(_));
        let result = execute(artifact).unwrap();
        if nominal {
            for (index, ty) in types.into_iter().enumerate() {
                assert_eq!(result.value().sequence_get(index).unwrap().solved_type_id(), Some(ty));
            }
        } else {
            for index in 0..2 {
                assert_eq!(result.value().sequence_get(index).unwrap().as_atom().unwrap().as_str(), "None");
            }
        }
    }
}

#[test]
fn generalized_function_aliases_preserve_local_captures() {
    for source in [
        "def identity = fn(value) { value }; def alias = identity; export def answer = (alias(21), alias(True));",
        "export def answer = { let identity = fn(value) { value }; let alias = identity; (alias(21), alias(True)) };",
        "export def answer = { let offset = 1; let identity = fn(value) { if offset == 1 { value } else { value } }; let alias = identity; (alias(21), alias(True)) };",
    ] {
        let mir = graph(source, "");
        let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
        let result = execute(artifact).unwrap();
        assert_eq!(result.value().sequence_get(0).unwrap().as_int(), Some(21));
        assert_eq!(result.value().sequence_get(1).unwrap().runtime().value(), crate::heap::DecodedValue::BuiltinAtom(crate::BuiltinAtom::True));
    }
}

#[test]
fn imported_generic_constructor_aliases_execute_closed_instances() {
    for source in [
        "import \"./math\" { Message }; import Message.{Data}; export def answer = (Data(1), Data@[String](\"text\"));",
        "import \"./math\" { Make }; export def answer = (Make(1), Make@[String](\"text\"));",
        "export def answer = { import Option.{Some as Make}; (Make(1), Make@[String](\"text\")) };",
        "export def answer = (Option.Some@[Int](1), Option.Some@[String](\"text\"));",
        "import \"./math\" { Message }; export def answer = (Message.Data@[Int](1), Message.Data@[String](\"text\"));",
    ] {
        let mir = graph(source, "export type Message(T) = enum { Data(T), Empty }; import Message.{Data as Make}; export { Make };");
        let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
        let items = artifact.types.types[artifact.result_type.index()].arguments.clone();
        assert_ne!(items[0], items[1], "{source}");
        let nominal = matches!(artifact.types.types[items[0].index()].constructor, TypeConstructor::Nominal(_));
        let result = execute(artifact).unwrap();
        let first = result.value().sequence_get(0).unwrap();
        let second = result.value().sequence_get(1).unwrap();
        assert_eq!(first.tagged_parts().unwrap().1.as_int(), Some(1), "{source}");
        assert_eq!(second.tagged_parts().unwrap().1.as_str().unwrap().as_ref(), "text", "{source}");
        if nominal {
            assert_eq!(first.solved_type_id(), Some(items[0]), "{source}");
            assert_eq!(second.solved_type_id(), Some(items[1]), "{source}");
        }
    }
}

#[test]
fn first_and_cached_demands_preserve_initializer_origin_through_function_returns() {
    let mir = graph("import \"./math\" { original }; def echo: Fn(Int) -> Int = fn(value) { value }; export def answer = (echo(original), echo(original), original);", "export def original = -7;");
    let expected = mir.hir.iter().find(|node| matches!(node.kind, HirKind::Unary(crate::ast::UnaryOperator::Negate))).unwrap().location;
    let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    let result = execute(artifact).unwrap();
    for index in 0..3 {
        let value = result.value().sequence_get(index).unwrap();
        assert_eq!(value.as_int(), Some(-7));
        assert_eq!(value.runtime().loc(), Some(expected));
    }
    let mir = graph("import \"./math\" { Item, original }; def echo: Fn(Item) -> Item = fn(value) { value }; export def answer = (echo(original), original);", "export type Item = struct {value: Int}; export def original: Item = {value: 42};");
    let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    let result = execute(artifact).unwrap();
    let first = result.value().sequence_get(0).unwrap();
    let cached = result.value().sequence_get(1).unwrap();
    assert!(first.solved_type_id().is_some());
    assert_eq!(first.solved_type_id(), cached.solved_type_id());
    assert_eq!(first.runtime().value(), cached.runtime().value());
    assert_eq!(first.runtime().loc(), cached.runtime().loc());
}

#[test]
fn tail_positions_use_existing_frame_replacement_without_skipping_followup_work() {
    for source in [
        "def count: Fn(Int) -> Int = fn(n) { if n == 0 { 42 } else { count(n - 1) } }; export def answer = count(2000);",
        "export def answer = { decl even: Fn(Int) -> Int; decl odd: Fn(Int) -> Int; def even = fn(n) { if n == 0 { 42 } else { odd(n - 1) } }; def odd = fn(n) { if n == 0 { 0 } else { even(n - 1) } }; even(2000) };",
        "def count: Fn(Int) -> Int = fn(n) { match n { 0 => 42, value => count(value - 1) } }; export def answer = count(2000);",
        "def count: Fn(Int) -> Int = fn(n) { if n == 0 { 42 } else { return count(n - 1); } }; export def answer = count(2000);",
        "def count: for(T) Fn(T, Int) -> T = fn(value, n) { if n == 0 { value } else { count(value, n - 1) } }; export def answer = count(42, 2000);",
        "def count: Fn(Int) -> Int = fn(n) { if n == 0 { 0 } else { count(n - 1) + 1 } }; export def answer = count(42);",
        "import \"std/_rt\" as rt; def count: Fn(Int) -> Int = fn(n) { if n == 0 { 42 } else { count(n - 1) } }; export def answer = match rt.with_diagnostics(count)(2000) { Ok((value, _)) => value, Err(_) => 0 };",
        "import \"std/_rt\" as rt; def count: Fn(Int) -> Int = fn(n) { if n == 0 { fail!(\"caught tail failure\") } else { count(n - 1) } }; export def answer = match rt.with_diagnostics(count)(2000) { Err(errors) => if errors[0].message == \"caught tail failure\" { 42 } else { 0 }, Ok(_) => 0 };",
    ] {
        let mir = graph(source, "");
        let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
        assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42), "{source}");
    }
    for body in ["raw()", "return raw();"] {
        let source = format!("@check(fn(value) {{ Err(blame!(\"must run return check\", value)) }}) type Item = struct {{x: Int}}; def raw: Fn() -> Unchecked(Item) = fn() {{ {{x: 0}} }}; def checked: Fn() -> Item = fn() {{ {body} }}; export def answer = checked();");
        let mir = graph(&source, "");
        let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
        assert!(execute(artifact).err().expect("return check rejects").to_string().contains("must run return check"));
    }
}


#[test]
fn metadata_joins_execute_the_selected_original_witness() {
    let mir = graph("def choose = fn(flag: Bool) { if flag { Int.type } else { String.type } }; export def answer = (choose(True), choose(False));", "");
    let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
    let int = TypeId(artifact.types.types.iter().position(|ty| ty.constructor == TypeConstructor::Int).unwrap() as u32);
    let string = TypeId(artifact.types.types.iter().position(|ty| ty.constructor == TypeConstructor::String).unwrap() as u32);
    let result = execute(artifact).unwrap();
    assert_eq!(result.value().sequence_get(0).unwrap().represented_type_id(), Some(int));
    assert_eq!(result.value().sequence_get(1).unwrap().represented_type_id(), Some(string));
}

#[test]
fn nested_callable_results_use_closed_instances() {
    for source in [
        "def invoke = fn(factory) { factory()() }; export def answer = invoke(fn() { fn() { 42 } });",
        "def invoke = fn(factory) { factory()() }; export def answer = if invoke(fn() { fn() { \"text\" } }) == \"text\" { invoke(fn() { fn() { 42 } }) } else { 0 };",
        "def invoke = fn(callback, value) { let saved = callback; saved(value) }; export def answer = invoke(fn(value: Int) { value + 1 }, 41);",
    ] {
        let mir = graph(source, "");
        let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
        let artifact = compile(sealed, entry(&mir)).unwrap();
        assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
    }
}

#[test]
fn implicit_schemes_execute_closed_global_and_local_instances() {
    let mir = graph("import \"./math\" { identity }; export def answer = if identity(\"text\") == \"text\" { identity(42) } else { 0 };", "export def identity = fn(value) { value };");
    let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
    for source in [
        "def identity = fn(value) { value }; export def answer = if identity(\"text\") == \"text\" { identity(42) } else { 0 };",
        "export def answer = { let identity = fn(value) { value }; if identity(\"text\") == \"text\" && identity@[Int](3) == 3 { identity(42) } else { 0 } };",
        "export def answer = { def first = fn(value) { second(value) }; def second = fn(value) { value }; if first(\"text\") == \"text\" { first(42) } else { 0 } };",
        "export def answer = { let captured = 42; let keep = fn(value) { captured }; if keep(\"text\") == 42 { keep(True) } else { 0 } };",
        "def outer = fn(value) { let keep = fn(other) { value }; (keep(True), keep(\"text\")) }; export def answer = if outer(\"text\").0 == \"text\" { outer(42).1 } else { 0 };",
        "export def answer = { let identity = fn(value) { value }; let use_it = fn(value: Int) { identity(value) }; if identity(\"text\") == \"text\" { use_it(42) } else { 0 } };",
        "export def answer = { let identity = fn(value) { value }; if identity@[Int] == identity@[Int] { identity(42) } else { 0 } };",
    ] {
        let mir = graph(source, "");
        let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
        let artifact = compile(sealed, entry(&mir)).unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
        assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42), "{source}");
    }
}

#[test]
fn propagation_uses_solved_families_and_nearest_function_boundary() {
    for source in [
        "def step: Fn(Option(Int)) -> Option(String) = fn(value) { value?; Some(\"ok\") }; export def answer = if step(None) == None && step(Some(1)) == Some(\"ok\") { 42 } else { 0 };",
        "def step: Fn(Result(Int, String)) -> Result(Bool, String) = fn(value) { value?; Ok(True) }; export def answer = if step(Err(\"bad\")) == Err(\"bad\") && step(Ok(1)) == Ok(True) { 42 } else { 0 };",
        "def step = fn(value: Option(Int)) { let item = { value? }; Some(item + 1) }; export def answer = if step(None) == None && step(Some(41)) == Some(42) { 42 } else { 0 };",
        "def outer: Fn(Option(Int)) -> Option(Option(Int)) = fn(value) { let inner: Fn(Option(Int)) -> Option(Int) = fn(item) { Some(item?) }; Some(inner(value)) }; export def answer = if outer(None) == Some(None) { 42 } else { 0 };",
        "def step: for(T) Fn(Result(T, String)) -> Result(T, String) = fn(value) { Ok(value?) }; export def answer = if step@[Int](Err(\"bad\")) == Err(\"bad\") { step(Ok(42)).unwrap!() } else { 0 };",
        "def step: Fn(Result(Int, String)) -> Result((), String) = fn(value) { value?; fail!(\"unreachable\") }; export def answer = if step(Err(\"bad\")) == Err(\"bad\") { 42 } else { 0 };",
        "def step = fn(value: Result(Int, String)) { value?; fail!(\"unreachable\") }; def checked: Fn(Result(Int, String)) -> Result((), String) = step; export def answer = if checked(Err(\"bad\")) == Err(\"bad\") { 42 } else { 0 };",
    ] {
        let mir = graph(source, "");
        let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
        let artifact = compile(sealed, entry(&mir)).unwrap();
        assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42), "{source}");
    }
}

#[test]
fn solved_parsers_and_diagnostics_use_closed_types() {
    for source in [
        "import \"std/json\" as json; import \"std/value\" {Value}; export def answer = match json.parse(\"42\") { Ok(Value.Int(n)) => n, _ => 0 };",
        "import \"std/yaml\" as yaml; import \"std/value\" {Value}; export def answer = match yaml.parse(\"42\") { Ok(Value.Int(n)) => n, _ => 0 };",
        "import \"std/toml\" as toml; import \"std/dict\" as dict; import \"std/value\" {Value}; export def answer = match toml.parse(\"n = 42\") { Ok(Value.Object(fields)) => match dict.get(fields, \"n\") { Some(Value.Int(n)) => n, _ => 0 }, _ => 0 };",
        "import \"std/json\" as json; export def answer = match json.parse(\"{\") { Err(_) => 42, Ok(_) => 0 };",
        "import \"std/json\" as json; import \"std/_rt\" as rt; import \"std/array\" as array; export def answer = match rt.with_diagnostics(fn(text: String) { json.parse(text).unwrap!() })(\"{\") { Err(errors) => if array.length(errors) == 1 { 42 } else { 0 }, Ok(_) => 0 };",
        "import \"std/_rt\" as rt; export def answer = match rt.with_diagnostics(fn(n: Int) { fail!(\"boom\") })(1) { Err(errors) => if errors[0].message == \"boom\" { 42 } else { 0 }, _ => 0 };",
    ] {
        let mir = graph(source, "");
        let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}")), entry(&mir)).unwrap_or_else(|d| panic!("{source}\n{d:?}"));
        assert_eq!(execute(artifact).unwrap_or_else(|d| panic!("{source}\n{d}")).value().as_int(), Some(42), "{source}");
    }
}
#[test]
fn sealed_run_policy_configures_and_initializes_in_one_graph() {
    let mir = graph(
        r#"
        import "./math" as policy;
        import "std/entry" as entry;
        import "std/ees" as ees;
        import "std/_rt" as rt;
        def app = entry.run(Int.type, {sources: [], envs: [], args: False}, ees.none,
            fn(ctx) { (42, fn(state, event) { (state, []) }) });
        def main: policy.MainType = {config: app.config, ees: app.ees, start: app.start};
        def configured = policy.config({args: [], ees: {}, mode: rt.EntryMode.Run,
            platform: {os: "linux", arch: "x86_64"}, sources: {}}, main);
        def initialized = configured.1({data: {}, texts: {}, vars: {}, stdin: None}, main);
        def transition = initialized.1(initialized.0, rt.SystemEvent.Initialize);
        export def answer = if transition.0.completed == False { 42 } else { 0 };
        "#,
        include_str!("../../../modules/std/_entry/run.telora"),
    );
    let sealed = mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{:?}", mir.diagnostics));
    let artifact = compile(sealed, entry(&mir)).unwrap();
    assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
}

#[test]
fn sealed_run_policy_emits_json_reply_and_exit() {
    let mir = graph(
        r#"
        import "./math" as policy;
        import "std/entry" as entry;
        import "std/ees" as ees;
        import "std/actor" as actor;
        import "std/value" {Value};
        import "std/_rt" as rt;
        def app = entry.run(Int.type, {sources: [], envs: [], args: False}, ees.none,
            fn(ctx) { (42, fn(state, event) {
                (state, [actor.Effect.Reply({request_id: "run", value: Value.Int(state)})])
            }) });
        def main: policy.MainType = {config: app.config, ees: app.ees, start: app.start};
        def configured = policy.config({args: [], ees: {}, mode: rt.EntryMode.Run,
            platform: {os: "linux", arch: "x86_64"}, sources: {}}, main);
        def initialized = configured.1({data: {}, texts: {}, vars: {}, stdin: None}, main);
        def transition = initialized.1(initialized.0, rt.SystemEvent.Initialize);
        export def answer = if transition.0.completed {
            match (transition.1[0], transition.1[1]) {
                (rt.SystemEffect.Output("42"), rt.SystemEffect.Exit(0)) => 42,
                _ => 0,
            }
        } else { 0 };
        "#,
        include_str!("../../../modules/std/_entry/run.telora"),
    );
    let sealed = mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{:?}", mir.diagnostics));
    let artifact = compile(sealed, entry(&mir)).unwrap();
    assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
}

#[test]
fn solved_json_formatters_read_the_original_value_graph() {
    let source = r#"
        import "std/json" as json;
        import "std/value" {Value};
        def input = Value.Object({a: Value.Array([Value.Int(42), Value.True]), b: Value.Object({})});
        export def answer = (json.stringify(input), json.stringify_pretty(2)(input), json.stringify_pretty(0)(input));
    "#;
    let mir = graph(source, "");
    let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    let result = execute(artifact).unwrap();
    for (index, expected) in [
        r#"{"a":[42,true],"b":{}}"#,
        "{\n  \"a\": [\n    42,\n    true\n  ],\n  \"b\": {}\n}",
        "{\n\"a\": [\n42,\ntrue\n],\n\"b\": {}\n}",
    ].iter().enumerate() {
        assert_eq!(result.value().sequence_get(index).unwrap().as_str().unwrap().as_str(), *expected);
    }
}

#[test]
fn solved_type_desc_observes_static_bodies_and_applied_members() {
    for source in [
        r#"decl message: Fn(Int) -> String; def message = fn(n) { `value=\{n}` }; export def answer = if message(42) == "value=42" { 42 } else { 0 };"#,
        r#"import "std/type-desc" as td;
            type Box(T) = struct { value: T }; type Tree = enum { Empty, Branch(Array(Tree)) };
            def body = match td.resolve(Box(Int).type) { Ok(value) => value, Err(_) => fail!("resolve") };
            export def answer = if td.kind(Box(Int).type) == td.TypeDescKind.Ref && td.kind(body) == td.TypeDescKind.Struct && td.fields(body)[0].ty == Int.type { 42 } else { 0 };"#,
        r#"import "std/type-desc" as td;
            type Box(T) = struct(T);
            def body = match td.resolve(Box(Int).type) { Ok(value) => value, Err(_) => fail!("resolve") };
            export def answer = if td.kind(body) == td.TypeDescKind.Newtype && td.children(body)[0] == Int.type { 42 } else { 0 };"#,
        r#"import "std/type-desc" as td;
            type Tree(T) = enum { Empty, Branch(Array(Tree(T))), Leaf(T) };
            def body = match td.resolve(Tree(Int).type) { Ok(value) => value, Err(_) => fail!("resolve") };
            def variants = td.variants(body);
            export def answer = if variants[0].name == "Branch" && variants[0].payload == Some(Array(Tree(Int)).type) && variants[1].name == "Empty" && variants[1].payload == None && variants[2].payload == Some(Int.type) && td.kind(body) == td.TypeDescKind.Enum { 42 } else { 0 };"#,
        r#"import "std/type-desc" as td;
            def variants = td.variants(Result(Int, String).type);
            export def answer = if variants[0].name == "Err" && variants[0].payload == Some(String.type) && variants[1].name == "Ok" && variants[1].payload == Some(Int.type) { 42 } else { 0 };"#,
        r#"import "std/type-desc" as td; import "std/array" as array;
            export def answer = if td.kind(Int.type) == td.TypeDescKind.Int && td.variants(Option(Int).type)[1].payload == Some(Int.type) && array.length(td.children((Fn(Int) -> String).type)) == 0 { 42 } else { 0 };"#,
        r#"import "std/type-desc" as td; export def answer = match td.resolve(Int.type) { Err(_) => 42, _ => 0 };"#,
    ] {
        let mir = graph(source, "");
        assert!(mir.diagnostics.is_empty(), "{source}\n{:?}", mir.diagnostics);
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        drop(mir);
        let result = execute(artifact).unwrap_or_else(|e| panic!("{source}\n{e}"));
        assert_eq!(result.value().as_int(), Some(42), "{source}");
    }
}

#[test]
fn solved_prepared_display_uses_type_desc_and_dyn_member_ids() {
    let mir = graph(r#"
        import "std/fmt" as fmt; import "std/type-property" as properties; import "std/dyn" as dyn;
        @fmt.display_by("{host}:{port}") type Endpoint = struct { host: String, port: Int };
        def value: Endpoint = {host: "localhost", port: 8080};
        def property = match properties.get_type_prop(Endpoint.type, fmt.DisplayBy.type) { Some(p) => p, None => fail!("missing display") };
        export def answer = fmt.render(property.display(dyn.pack(Endpoint.type, value)));
    "#, "");
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    drop(mir);
    let result = execute(artifact).unwrap();
    assert_eq!(result.value().as_str().unwrap().as_str(), "localhost:8080");
}

#[test]
fn solved_dyn_members_consume_applied_layouts() {
    for source in [
        r#"import "std/dyn" as dyn; type Box(T) = struct { z: String, a: T };
            def value: Box(Int) = {a: 42, z: "other"};
            export def answer = take(dyn.project_with(Int.type, dyn.get_field_value(dyn.pack(Box(Int).type, value), 0)));"#,
        r#"import "std/dyn" as dyn; type Box(T) = struct(T);
            def items = get(dyn.tuple_items(dyn.pack(Box(Int).type, Box(42))));
            export def answer = take(dyn.project_with(Int.type, items[0]));"#,
        r#"import "std/dyn" as dyn; type Tree(T) = enum { Empty, Leaf(T), Branch(Array(Tree(T))) };
            def value = dyn.pack(Tree(Int).type, Tree.Leaf(42));
            def child = take(dyn.get_variant_payload(value, 2));
            export def answer = if dyn.get_variant_index(value) == 2 { take(dyn.project_with(Int.type, child)) } else { 0 };"#,
        r#"import "std/dyn" as dyn; type Item = enum { Empty, Full(Int) };
            def value = dyn.pack(Item.type, Item.Empty);
            export def answer = if dyn.get_variant_payload(value, 0) == None && dyn.kind(value) == dyn.ValueKind.Atom { 42 } else { 0 };"#,
        r#"import "std/dyn" as dyn; def value = dyn.pack(Dict(Int).type, {a: 42});
            export def answer = take(dyn.project_with(Int.type, get(dyn.field(value, "a"))));"#,
        r#"import "std/dyn" as dyn; def value = dyn.pack((Int, String).type, (42, "other"));
            export def answer = take(dyn.project_with(Int.type, get(dyn.tuple_items(value))[0]));"#,
        r#"import "std/dyn" as dyn; def value = dyn.pack(Array(Int).type, [42]);
            export def answer = take(dyn.project_with(Int.type, get(dyn.array_items(value))[0]));"#,
        r#"import "std/dyn" as dyn; def value = dyn.pack(Option(Int).type, Some(42));
            export def answer = if get(dyn.tag(value)) == "Some" { take(dyn.project_with(Int.type, take(get(dyn.payload(value))))) } else { 0 };"#,
        r#"import "std/dyn" as dyn; type Box(T) = struct { value: T };
            def value: Box(Int) = {value: 42}; def fields = get(dyn.fields(dyn.pack(Box(Int).type, value)));
            export def answer = if fields[0].0 == "value" { take(dyn.project_with(Int.type, fields[0].1)) } else { 0 };"#,
        r#"import "std/dyn" as dyn; export def answer = match dyn.field(dyn.pack(Int.type, 1), "missing") { Err(_) => 42, _ => 0 };"#,
    ] {
        let source = &format!("def take: for(T) Fn(Option(T)) -> T = fn(value) {{ match value {{ Some(value) => value, None => fail!(\"missing value\") }} }}; def get: for(T, E) Fn(Result(T, E)) -> T = fn(value) {{ match value {{ Ok(value) => value, Err(_) => fail!(\"access failed\") }} }}; {source}");
        let mir = graph(source, "");
        assert!(mir.diagnostics.is_empty(), "{source}\n{:?}", mir.diagnostics);
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        drop(mir);
        let result = execute(artifact).unwrap_or_else(|e| panic!("{source}\n{e}"));
        assert_eq!(result.value().as_int(), Some(42), "{source}");
    }
}

#[test]
fn solved_dyn_and_actor_service_keep_type_witnesses_in_the_vm() {
    for source in [
        "import \"std/dyn\" as dyn; export def answer = match dyn.project_with(Int.type, dyn.pack(Int.type, 42)) { Some(value) => value, None => 0 };",
        "import \"std/dyn\" as dyn; export def answer = match dyn.project_with(String.type, dyn.pack(Int.type, 1)) { Some(_) => 0, None => 42 };",
        "import \"std/dyn\" as dyn; type Alias = Int; export def answer = if dyn.desc(dyn.pack(Alias.type, 1)) == Int.type { 42 } else { 0 };",
        "import \"std/dyn\" as dyn; type Count = struct(Int); export def answer = match dyn.check_int(dyn.pack(Count.type, Count(1))) { Some(_) => 0, None => 42 };",
        "import \"std/dyn\" as dyn; type A = struct(Int); type B = struct(Int); export def answer = match dyn.project_with(B.type, dyn.pack(A.type, A(1))) { Some(_) => 0, None => 42 };",
        "import \"std/dyn\" as dyn; export def answer = match dyn.check_int(dyn.pack(Int.type, 42)) { Some(value) => value, None => 0 };",
        "import \"std/actor\" as actor; import \"std/value\" {Value}; import \"std/dyn\" as dyn; def service = actor.service(Array(Int).type, [42], fn(state, event) { (state, []) }); def transition = service.reduce((service.state, actor.Event.Request({id: \"request\", input: Value.Int(1)}))); export def answer = match dyn.project_with(Array(Int).type, transition.0) { Some(values) => values[0], None => 0 };",
        "import \"std/entry\" as entry; import \"std/ees\" as ees; import \"std/dyn\" as dyn; def app = entry.run(Int.type, { sources: [], envs: [], args: False }, ees.none, fn(ctx) { (42, fn(state, event) { (state, []) }) }); def service = app.start({ sources: {}, env: {}, args: [] }); export def answer = match dyn.project_with(Int.type, service.state) { Some(value) => value, None => 0 };",
    ] {
        let mir = graph(source, "");
        let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{:?}", mir.diagnostics));
        let artifact = compile(sealed, entry(&mir)).unwrap_or_else(|d| panic!("{source}\n{d:?}"));
        let result = execute(artifact).unwrap_or_else(|d| panic!("{source}\n{d}"));
        assert_eq!(result.value().as_int(), Some(42), "{source}");
    }
}
#[test]
fn sequence_spreads_close_element_slots_and_preserve_evaluation_order() {
    for source in [
        "def empty = []; def a = [...empty, 42]; def b = [42, ...empty]; export def answer = if a == b { a[0] } else { 0 };",
        "def value = [...[[]], [42]]; export def answer = value[1][0];",
        "def value = (1, \"ok\"); export def answer = if (...(), ...value, 42, ...()) == (1, \"ok\", 42) { 42 } else { 0 };",
        "type Item = struct {value: Int}; def values: (Int, Item, String) = (...(1, {value: 42}), \"ok\"); export def answer = values.1.value;",
        "type Item = struct {value: Int}; def values: (Item, Int) = (...(...({value: 42},), 3)); export def answer = values.0.value;",
        "def append: for(T) Fn((T, String)) -> (T, String, Int) = fn(value) { (...value, 42) }; export def answer = append((1, \"ok\")).2;",
        "def copy: for(T) Fn(Array(T)) -> Array(T) = fn(value) { [...value] }; export def answer = copy([42])[0];",
    ] {
        let mir = graph(source, "");
        let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
        let artifact = compile(sealed, entry(&mir)).unwrap();
        assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42), "{source}");
    }
    for source in [
        "def stop: Fn() -> Array(Int) = fn() { fail!(\"first operand\") }; export def answer = [...stop(), fail!(\"later operand\")];",
        "def stop: Fn() -> (Int,) = fn() { fail!(\"first operand\") }; export def answer = (...stop(), fail!(\"later operand\"));",
        "export def answer = (...fail!(\"first operand\"), 42);",
    ] {
        let mir = graph(source, "");
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        assert!(execute(artifact).err().expect("eager failure").to_string().contains("first operand"));
    }
}

#[test]
fn record_spreads_keep_winning_types_and_evaluate_overwritten_expressions() {
    for source in [
        "type Full = struct {x: Int, label: String}; type Count = struct {x: Int}; type Wrong = struct {x: String}; def base: Full = {x: 1, label: \"base\"}; def count: Count = {x: 42}; def wrong: Wrong = {x: \"ignored\"}; export def answer = (base <~ {x: \"ignored\", ...count} <~ {...wrong, x: 42}).x;",
        "type Full = struct {x: Int, label: String}; type Count = struct {x: Int}; def count: Count = {x: 42}; def value: Full = {...count, label: \"ok\"}; export def answer = value.x;",
        "type Box(T) = struct {value: T}; def copy: for(T) Fn(Box(T)) -> Box(T) = fn(value) { {...value} }; def value: Box(Int) = {value: 42}; export def answer = copy(value).value;",
        "type Box(T) = struct {value: T}; decl copy: for(T) Fn(Box(T)) -> Box(T); def copy = fn(value) { {...value} }; def number: Box(Int) = {value: 42}; def text: Box(String) = {value: \"ok\"}; export def answer = if copy(text).value == \"ok\" { copy(number).value } else { 0 };",
        "def left: Dict(Int) = {x: 1}; def right: Dict(Int) = {x: 42}; def result = {...left, ...right}; export def answer = result.x;",
        "def result: Dict(Int) = {...{x: 42}}; export def answer = result.x;",
    ] {
        let mir = graph(source, "");
        let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
        let artifact = compile(sealed, entry(&mir)).unwrap();
        assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42), "{source}");
    }
    let source = "type Item = struct {x: Int}; def base: Item = {x: 42}; export def answer = base <~ {x: fail!(\"overwritten failure\"), ...base};";
    let mir = graph(source, "");
    let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    assert!(execute(artifact).err().expect("eager failure").to_string().contains("overwritten failure"));
}

#[test]
fn struct_updates_preserve_identity_and_contextual_field_types() {
    for source in [
        "type Full = struct {x: Int, label: String}; type Patch = struct {x: Int}; def base: Full = {x: 1, label: \"base\"}; def patch: Patch = {x: 20}; def updated = base <~ patch <~ {x: 42}; export def answer = if base.x == 1 && updated.label == \"base\" { updated.x } else { 0 };",
        "type Child = struct {value: Int}; type Parent = struct {child: Child, items: Array(Int)}; def base: Parent = {child: {value: 1}, items: [1]}; def updated = base <~ {child: {value: 42}, items: []}; export def answer = updated.child.value;",
        "type Box(T) = struct {value: T}; def replace: for(T) Fn(Box(T), T) -> Box(T) = fn(base, value) { base <~ {value: value} }; def base: Box(Int) = {value: 1}; export def answer = replace(base, 42).value;",
        "type Source = struct {x: Int, y: String}; type Target = struct {value: Int}; def source: Source = {x: 42, y: \"x\"}; def base: Target = {value: 1}; export def answer = (base <~ source.{x as value}).value;",
    ] {
        let mir = graph(source, "");
        let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
        let artifact = compile(sealed, entry(&mir)).unwrap();
        assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42), "{source}");
    }
    let source = "@check(fn(value) { if value.x > 0 { Ok(()) } else { Err(blame!(\"update rejected\", value)) } }) type Item = struct {x: Int}; def base: Item = {x: 1}; export def answer = base <~ {x: 0};";
    let mir = graph(source, "");
    let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    assert!(execute(artifact).err().expect("construction rejection").to_string().contains("update rejected"));
}

#[test]
fn field_projection_uses_solved_nominal_shapes_and_checks() {
    for source in [
        "type Source = struct {x: Int, y: String}; type Target = struct {value: Int}; def source: Source = {x: 42, y: \"x\"}; def selected: Target = source.{x as value}; export def answer = selected.value;",
        "type Source(T) = struct {value: T}; type Target(T) = struct {item: T}; def select: for(T) Fn(Source(T)) -> Target(T) = fn(value) { value.{value as item} }; def source: Source(Int) = {value: 42}; export def answer = select(source).item;",
        "type Source(T) = struct {value: T}; type Target(T) = struct {item: T}; export def answer = do { let source: Source(Int) = {value: 42}; let projected: Target(Int) = source.{value as item}; projected.item };",
        "type Source = struct {x: Int}; type Target = struct {a: Int, b: Int}; def source: Source = {x: 21}; def selected: Target = source.{x as a, x as b}; export def answer = selected.a + selected.b;",
        "type Source = struct {x: Int}; type Empty = struct {}; def source: Source = {x: 42}; def selected: Empty = source.{}; export def answer = if selected == {} { 42 } else { 0 };",
    ] {
        let mir = graph(source, "");
        let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
        let artifact = compile(sealed, entry(&mir)).unwrap();
        assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42), "{source}");
    }
    let source = "type Source = struct {x: Int}; @check(fn(value) { Err(blame!(\"projection rejected\", value)) }) type Target = struct {x: Int}; def source: Source = {x: 42}; export def answer: Target = source.{x};";
    let mir = graph(source, "");
    let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    assert!(execute(artifact).err().expect("construction rejection").to_string().contains("projection rejected"));
}

#[test]
fn interpolation_resolves_display_calls_before_codegen() {
    for source in [
        r#"export def answer = if `n=\{42}` == "n=42" { 42 } else { 0 };"#,
        r#"import "std/fmt" as fmt; type Item = struct {value: Int}; impl fmt.Display for Item { display: fn(value) { fmt.from_string("item") } }; def value: Item = {value: 1}; export def answer = if `\{value}` == "item" { 42 } else { 0 };"#,
        r#"import "std/fmt" as fmt; def render: for(T: fmt.Display) Fn(T) -> String = fn(value) { `\{value}` }; export def answer = if render(42) == "42" && render("ok") == "ok" { 42 } else { 0 };"#,
        r#"def Display = 0; export def answer = if `\{42}` == "42" { 42 } else { 0 };"#,
    ] {
        let mir = graph(source, "");
        let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
        let artifact = compile(sealed, entry(&mir)).unwrap_or_else(|d| panic!("{source}\n{d:?}"));
        assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42), "{source}");
    }
    let missing = graph(r#"type Item = struct {value: Int}; def value: Item = {value: 1}; export def answer = `\{value}`;"#, "");
    assert!(missing.seal().is_err());
    assert!(missing.bound_requirements.iter().any(|bound| !bound.state.is_proven()));
}

#[test]
fn property_evidence_demands_the_statically_proven_value() {
    let source = r#"
        import "std/type-property" as prop;
        @property(PropertyTarget.Type) type Mark = struct {value: Int};
        def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { {value: 42} };
        @mark type Item = struct {x: Int};
        def read: for(T: Property(Mark)) Fn(TypeOf(T)) -> Int = fn(owner) {
            let get = prop.evidence;
            get(owner, Mark.type).value
        };
        export def answer = read(Item.type);
    "#;
    let mir = graph(source, "");
    let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
    assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
    let missing = graph(&source.replace("@mark type Item", "type Item"), "");
    assert!(missing.seal().is_err());
    let failed = graph(&source.replace("{value: 42}", "fail!(\"provider failed\")"), "");
    let artifact = compile(failed.seal().unwrap(), entry(&failed)).unwrap();
    assert!(execute(artifact).err().expect("provider failure").to_string().contains("provider failed"));
}

#[test]
fn block_bottoms_preserve_unit_tails_and_contextual_types() {
    for source in [
        "export def answer = if False { fail!(\"unreachable\"); } else { 42 };",
        "export def answer = if True { 42 } else { let x = fail!(\"unreachable\"); };",
        "export def answer = if True { 42 } else { let x: Int = fail!(\"unreachable\"); };",
        "def early: Fn() -> Int = fn() { 1; return 42; }; export def answer = early();",
        "def stop: Fn() -> Never = fn() { fail!(\"unreachable\") }; export def answer = if True { 42 } else { stop(); };",
        "def copy = fn(x) { let y = x; y }; export def answer = copy(42);",
        "def value: Fn() -> Int = fn() { let x = [42]; x[0] }; export def answer = value();",
        "export def answer = if (do {42;}) == () { 42 } else { 0 };",
    ] {
        let mir = graph(source, "");
        let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
        let artifact = compile(sealed, entry(&mir)).unwrap();
        let result = execute(artifact).unwrap();
        assert_eq!(result.value().as_int(), Some(42), "{source}");
    }
}

#[test]
fn local_recursive_functions_capture_block_slots_and_invocation_values() {
    for source in [
        "export def answer = do { def down: Fn(Int) -> Int = fn(n) { if n == 0 { 42 } else { down(n - 1) } }; down(4) };",
        "export def answer = do { def even: Fn(Int) -> Bool = fn(n) { if n == 0 { True } else { odd(n - 1) } }; def odd: Fn(Int) -> Bool = fn(n) { if n == 0 { False } else { even(n - 1) } }; if even(4) && odd(3) { 42 } else { 0 } };",
        "def make: Fn(Int) -> Fn(Int) -> Int = fn(base) { def walk: Fn(Int) -> Int = fn(n) { if n == 0 { base } else { walk(n - 1) } }; walk }; def first = make(20); def second = make(22); export def answer = first(3) + second(4);",
        "export def answer = do { decl next: Fn(Int) -> Int; def next = fn(n) { if n == 0 { 42 } else { next(n - 1) } }; next(3) };",
    ] {
        let mir = graph(source, "");
        let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
        let artifact = compile(sealed, entry(&mir)).unwrap_or_else(|d| panic!("{source}\n{d:?}"));
        let result = execute(artifact).unwrap_or_else(|d| panic!("{source}\n{d}"));
        assert_eq!(result.value().as_int(), Some(42), "{source}");
    }
}

#[test]
fn unary_operators_consume_the_solved_operand_family() {
    for source in [
        "export def answer = 43 & 42;",
        "export def answer = 40 | 2;",
        "export def answer = 40 ^ 2;",
        "export def answer = if !False { 42 } else { 0 };",
        "export def answer = !(-43);",
        "export def answer = -(-42);",
        "export def answer = if -1.5 < 0.0 { 42 } else { 0 };",
        "def invert: Fn(Int) -> Int = fn(value) { !value }; export def answer = invert(-43);",
    ] {
        let mir = graph(source, "");
        let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}"));
        let artifact = compile(sealed, entry(&mir)).unwrap();
        let result = execute(artifact).unwrap();
        assert_eq!(result.value().as_int(), Some(42), "{source}");
    }
    let mir = graph("export def answer = !1.5;", "");
    assert!(mir.seal().is_err());
    assert!(mir.diagnostics.iter().any(|d| d.message.starts_with("! requires Int or Bool, found ")));
}

#[test]
fn newtypes_and_selected_trait_implementations_execute_from_solved_ids() {
    for source in [
        "type Count = struct(Int); def count = Count(42); export def answer = count.0;",
        "type Box(T) = struct(T); type IntBox = Box(Int); def read: for(T) Fn(Box(T)) -> T = fn(value) { value.0 }; export def answer = read(IntBox(42));",
        "type Inner = struct(Int); type Outer = struct(Inner); export def answer = Outer(Inner(42)).0.0;",
        "type Inner = struct(Array(Int)); type Outer = struct(Inner); def input = [20, 22]; export def answer = match Outer(Inner(input)) { Outer(Inner(items)) => items[0] + items[1] };",
        "type Box(T) = struct(T); type IntBox = Box(Int); export def answer = match IntBox(42) { IntBox(value) => value };",
        "type A = struct(Int); type B = struct(A); export def answer = if B(A(42)) == B(A(42)) { 42 } else { 0 };",
        "trait Name { name: Fn(Self) -> Int }; impl Name for Int { name: fn(value) { value + 1 } }; impl Name for String { name: fn(value) { 42 } }; export def answer = Name.name(\"input\");",
        "trait Name { name: Fn(Self) -> Int }; impl Name for Int { name: fn(value) { base + value } }; def base = 40; def method = Name.name; export def answer = method(2);",
        "trait Count { step: Fn(Self, Int) -> Int }; impl Count for Int { step: fn(value, n) { if n == 0 { value } else { Count.step(value + 1, n - 1) } } }; export def answer = Count.step(0, 42);",
        "@property(PropertyTarget.Type) type Tag = struct(Int); def tag: Fn(Type, Option(Tag)) -> Tag = fn(owner, previous) { Tag(1) }; @tag type Item = struct(Int); trait Name { name: Fn(Self) -> Int }; impl(T: Property(Tag)) Name for T { name: fn(value) { 42 } }; export def answer = Name.name(Item(1));",
    ] {
        let mir = graph(source, "");
        let sealed = mir
            .seal()
            .unwrap_or_else(|d| panic!("{source}\n{d:?}\n{:?}", mir.diagnostics));
        let artifact =
            compile(sealed, entry(&mir)).unwrap_or_else(|d| panic!("{source}\n{d:?}"));
        let result = execute(artifact).unwrap_or_else(|d| panic!("{source}\n{d}"));
        assert_eq!(result.value().as_int(), Some(42), "{source}");
    }
}
#[test]
fn check_root_initializes_all_globals_and_properties_without_calling_functions() {
    for (source, expected) in [
        (
            "def unused = 1 / 0; export def answer = 42;",
            Some("division"),
        ),
        (
            "def unused: Fn() -> Int = fn() { fail!(\"do not call\") }; export def answer = 42;",
            None,
        ),
        (
            "@property(PropertyTarget.Type) type Mark = struct { value: Int }; def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { fail!(\"property root sentinel\") }; @mark type Item = struct { x: Int }; export def answer = 42;",
            Some("property root sentinel"),
        ),
        (
            "def a: Int = b; def b: Int = a; export def answer = 42;",
            Some("cyclic demand"),
        ),
    ] {
        let mut mir = graph(source, "");
        let artifact =
            compile_check(mir.seal().unwrap()).unwrap_or_else(|d| panic!("{source}\n{d:?}"));
        let linked = crate::execution_link::link_entry(artifact).unwrap();
        let diagnostics = crate::Vm::new().check_linked(
            linked,
            crate::Quota::with_fuel(10000),
            crate::DataLimits::default(),
            &mut mir.sources,
        );
        if let Some(message) = expected {
            assert!(
                diagnostics.iter().any(|d| d.message.contains(message)),
                "{source}\n{diagnostics:?}"
            );
        } else {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
        }
    }
}
#[test]
fn solved_patterns_and_native_variants_execute_with_lexical_scopes() {
    for source in [
        "export def answer = match Some((20, 22)) { Some((x, y)) => x + y, None => 0 };",
        "export def answer = match Result(Int, String).Err(\"bad\") { Ok(x) => x, Err(\"bad\") => 42, _ => 0 };",
        "type E = enum { A(Int), B }; export def answer = match E.A(21) { E.B => 0, E.A(x) if x < 0 => 1, E.A(x) => x * 2 };",
        "export def answer = if let Some(x) = Some(42) { x } else { 0 };",
        "def absent: Option(Int) = None; export def answer = if let Some(x) = absent { x } else { 42 };",
        "export def answer = do { let Some(x) = Some(42) else { fail!(\"absent\") }; x };",
        "type Rec = struct { x: Int, y: String }; def v: Rec = { x: 42, y: \"unused\" }; export def answer = match v { { x: n } => n };",
        "import \"std/array\" { fold_control }; type Control = FoldControl(Int, Int); export def answer = match fold_control([20, 22, 99], 0, fn(a, b) { if a == 42 { Control.Break(a) } else { Control.Continue(a + b) } }) { Control.Break(x) => x, Control.Continue(x) => x };",
        "def crash: Fn() -> Bool = fn() { fail!(\"must remain lazy\") }; export def answer = if (False && crash()) || (True || crash()) { 42 } else { 0 };",
        "def f: Fn(Option(Int)) -> Int = fn(v) { let Some(x) = v else { return 42; }; x }; export def answer = f(None);",
        "export def answer = (match Some(21) { Some(x) if False => fn() { 0 }, Some(x) => fn() { x * 2 }, None => fn() { 0 } })();",
        "import \"std/option\" { map, unwrap_or }; export def answer = unwrap_or(map(Some(21), fn(x) { x * 2 }), 0);",
        "import \"std/prelude\" { Some as Present, None as Absent }; export def answer = match Present(42) { Present(x) => x, Absent => 0 };",
        "import \"std/type-property\" { get_type_prop }; @property(PropertyTarget.Type) type Mark = struct { value: Int }; def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { { value: 21 + match previous { Some(p) => p.value, None => 0 } } }; @mark @mark type Item = struct { x: Int }; export def answer = match get_type_prop(Item.type, Mark.type) { Some(p) => p.value, None => 0 };",
    ] {
        let mir = graph(source, "");
        let sealed = mir
            .seal()
            .unwrap_or_else(|d| panic!("{source}\n{d:?}\n{:?}", mir.diagnostics));
        let artifact =
            compile(sealed, entry(&mir)).unwrap_or_else(|d| panic!("{source}\n{d:?}"));
        let result = execute(artifact).unwrap_or_else(|d| panic!("{source}\n{d}"));
        assert_eq!(result.value().as_int(), Some(42), "{source}");
    }
}
#[test]
fn record_pattern_fields_are_checked_before_codegen() {
    let mir = graph(
        "type Rec = struct { x: Int }; def v: Rec = { x: 1 }; export def answer = match v { { missing: n } => n };",
        "",
    );
    assert!(mir.seal().is_err());
    assert!(
        mir.diagnostics
            .iter()
            .any(|d| d.message.contains("missing")),
        "{:?}",
        mir.diagnostics
    );
}
