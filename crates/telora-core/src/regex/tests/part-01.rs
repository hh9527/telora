    #[test]
    fn float_plan_rejects_non_finite_results() {
        for input in ["NaN", "inf", "-inf", "1e9999"] {
            let Err(error) = execute_plan(&ParsePlan::Float, input) else {
                panic!("accepted non-finite Float {input}")
            };
            assert!(error.contains("finite Float"), "{input}: {error}");
        }
        assert!(matches!(
            execute_plan(&ParsePlan::Float, "1.5"),
            Ok(ParsedValue::Float(1.5))
        ));
    }
