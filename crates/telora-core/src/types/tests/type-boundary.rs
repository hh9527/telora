#[test]
fn tuple_type_syntax_and_explicit_metadata_are_distinct() {
    for source in [
        "type Pair = (Int, String); let pair: Pair = (1, \"a\"); pair",
        "let pair: (Int, String) = (1, \"a\"); pair",
        "def f: Fn((Int, String)) -> (String, Int) = fn(pair) { (pair.1, pair.0) }; f((1, \"a\"))",
        "type Pair(A) = (A, String); let pair: Pair(Int) = (1, \"a\"); pair",
        "let metadata: TypeOf(Int) = Int.type; metadata",
        "let metadata: TypeOf((Int, String)) = (Int, String).type; metadata",
        "let metadata: TypeOf(Array(Int)) = Array(Int).type; metadata",
        "let metadata: TypeOf(Fn(Int) -> String) = (Fn(Int) -> String).type; metadata",
        "let metadata = (Int.type, String.type); metadata",
        "type lower = Int; let UPPER = 1; (lower.type, UPPER)",
        "let Int = 1; (Int, 2)",
        "let Tuple = 1; let Func = 2; let value: (Int, String) = (1, \"a\"); let f: Fn(Int) -> Int = fn(x) { x }; (value, f(1))",
        "let value: (Int,) = (1,); value",
        "let value: ((Int, String), ()) = ((1, \"a\"), ()); value",
        "(1, \"a\").ty!((Int, String))",
        "(1, \"a\").cast!((Int, String))",
        "type Wrapped = struct(Int); type Alias = Wrapped; let make = Alias; make(1)",
        "type lower = Int; def identity: for(lower) Fn(lower) -> lower = fn(x) { x }; identity(1)",
    ] {
        analyze_source("type-boundary.telora", source)
            .unwrap_or_else(|error| panic!("{source}: {error:?}"));
    }
}

#[test]
fn metadata_data_cannot_reenter_static_types() {
    for source in [
        "let metadata = Int.type; type Bad = metadata; 0",
        "def make: Fn() -> Type = fn() { Int.type }; type Bad = make(); 0",
        "type Bad = if Bool.True { Int } else { String }; 0",
        "type Bad = Int.type; 0",
        "let Bad = (Int, String); Bad",
        "(Int, 1)",
        "(Int.type, String)",
        "let value = 1; value.type",
        "let value: TypeOf(Int) = Int.type; value.type",
        "def use: Fn(Type) -> Int = fn(metadata) { 1 }; use(Int)",
        "type Wrap = struct(Int); def use: Fn(Type) -> Int = fn(metadata) { 1 }; use(Wrap)",
        "type Pair = (...(Int, String), Bool); 0",
        "let value = (Int.type, String.type); type Bad = value; 0",
        "type Wrapped = struct(Type); type Bad = Wrapped(Int.type).0; 0",
        "def metadata: Fn(Type) -> Type = fn(value) { value }; let x: metadata(Int.type) = 1; x",
        "let fn_type = Fn(Int) -> String; fn_type",
    ] {
        assert!(
            analyze_source("type-boundary-invalid.telora", source).is_err(),
            "{source}"
        );
    }
}
