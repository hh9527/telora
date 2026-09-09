#[test]
fn result_propagation_keeps_error_returns_before_never_tails() {
    for source in [
        r#"
        def check: Fn(Result(Int, String)) -> Result((), String) = fn(value) {
            value?;
            panic!("success cannot return")
        };
        check
        "#,
        r#"
        def inferred = fn(value: Result(Int, String)) {
            value?;
            panic!("success cannot return");
        };
        def checked: Fn(Result(Int, String)) -> Result((), String) = inferred;
        checked
        "#,
    ] {
        analyze_source("result-never.telora", source)
            .unwrap_or_else(|error| panic!("{source}: {error:?}"));
    }
    let invalid = r#"
        def check: Fn(Result(Int, String)) -> Result((), Int) = fn(value) {
            value?;
            panic!("success cannot return")
        };
        check
    "#;
    assert!(analyze_source("result-never-error.telora", invalid).is_err());
}
