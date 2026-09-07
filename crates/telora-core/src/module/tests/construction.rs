#[test]
fn recursive_construction_and_codec_trials_preserve_resource_limits() {
    let directory = fixture_dir();
    let path = directory.join("main.telora");
    let source = include_str!("fixtures/construction-recursion.telora");
    for expression in ["Loop(0)", "decode()"] {
        fs::write(&path, format!("{source}\n{expression}\n")).unwrap();
        let module = load_module(&path, BTreeMap::new(), 1_000_000).unwrap();
        for (quota, expected) in [
            (Quota::new(100, usize::MAX, u64::MAX), crate::RuntimeErrorKind::FuelExhausted),
            (Quota::new(1_000_000, 256, u64::MAX), crate::RuntimeErrorKind::StackLimitExceeded),
            (Quota::new(1_000_000, usize::MAX, 8_192), crate::RuntimeErrorKind::AllocationQuotaExceeded),
            (Quota::new(1_000_000, usize::MAX, u64::MAX), crate::RuntimeErrorKind::CallDepthExceeded),
        ] {
            let error = module.execute_with_quota(quota).err().expect("recursive check must exhaust its quota");
            assert_eq!(error.kind, expected, "{expression}: {error}");
            assert!(error.trace.iter().any(|frame| frame.function == "@check"), "{expression}: {error}");
        }
    }
    fs::remove_dir_all(directory).unwrap();
}
