#[derive(Default)]
struct FixtureSpy {
    reads: Vec<String>,
}

impl TestHost for FixtureSpy {
    fn resolve(&mut self, _: &str, _: Option<&Path>, source: &str) -> Result<TestSource, String> {
        Ok(TestSource {
            key: source.into(),
            format: SystemDataFormat::Json,
        })
    }
    fn read(&mut self, source: &TestSource, _: usize) -> Result<String, String> {
        self.reads.push(source.key.clone());
        Ok("42".into())
    }
}

fn run_deferred(
    source: &str,
    quota: Quota,
    limits: TestLimits,
    host: &mut dyn TestHost,
) -> TestOutcome {
    let directory = fixture_dir();
    let path = directory.join("main.telora");
    fs::write(&path, source).unwrap();
    let engine = Engine::new(EngineConfig {
        session_quota: quota,
        ..recovery_engine().config()
    });
    let outcome = engine
        .test_with_resolver(ModuleResolver::standalone(&path).unwrap(), host, limits)
        .unwrap();
    fs::remove_dir_all(directory).unwrap();
    outcome
}

#[test]
fn deferred_tests_share_quota_and_bound_groups() {
    let code = r#"
import "std/test" as test;
def loop: Fn(Int) -> Int = fn(n) { if n == 0 { 0 } else { loop(n - 1) } };
export def a = test.should_ok(fn() { loop(20) });
export def b = test.should_fail(fn() { loop(20) });
export def c = test.should_ok(fn() { 1 });
"#;
    let outcome = run_deferred(
        code,
        Quota::with_fuel(35),
        TestLimits::default(),
        &mut FixtureSpy::default(),
    );
    assert_eq!(outcome.cases.len(), 2);
    assert!(outcome.cases[0].passed);
    assert!(outcome.aborted);
    assert!(!outcome.cases[1].passed);
    assert!(
        outcome.cases[1]
            .diagnostics
            .iter()
            .any(|d| d.message.contains("fuel"))
    );
    let nested = r#"
import "std/test" as test;
def group: Fn() -> test.Test = fn() { test.with_fixtures(["input"], fn(value) { group() }) };
export def cases = group();
"#;
    let outcome = run_deferred(
        nested,
        Quota::with_fuel(100_000),
        TestLimits {
            depth: 3,
            ..TestLimits::default()
        },
        &mut FixtureSpy::default(),
    );
    assert!(outcome.aborted);
    assert_eq!(outcome.cases.len(), 1);
    assert!(
        outcome.cases[0]
            .diagnostics
            .iter()
            .any(|d| d.message.contains("expansion limit"))
    );
}

#[test]
fn deferred_tests_cache_inputs_and_enforce_aggregate_and_allocation_budgets() {
    let code = r#"
import "std/test" as test;
export def cases = test.with_fixtures(["one", "one", "two"], fn(value) { test.should_ok(fn() { value }) });
"#;
    let mut host = FixtureSpy::default();
    let outcome = run_deferred(
        code,
        Quota::with_fuel(100_000),
        TestLimits::default(),
        &mut host,
    );
    assert!(outcome.passed(), "{:?}", outcome.diagnostics);
    assert_eq!(outcome.cases.len(), 3);
    assert_eq!(host.reads, ["one", "two"]);
    let outcome = run_deferred(
        code,
        Quota::with_fuel(100_000),
        TestLimits {
            fixture_bytes: 1,
            ..TestLimits::default()
        },
        &mut FixtureSpy::default(),
    );
    assert!(outcome.aborted);
    assert_eq!(outcome.cases.len(), 1);
    assert_eq!(outcome.cases[0].phase, "fixture");
    let outcome = run_deferred(
        code,
        Quota::new(100_000, 10_000, 1),
        TestLimits::default(),
        &mut FixtureSpy::default(),
    );
    assert!(outcome.aborted);
    assert!(!outcome.cases[0].passed);
}

#[test]
fn deferred_tests_prepare_the_group_before_any_factory() {
    struct Observer(Arc<Mutex<Vec<String>>>);
    impl DebugSink for Observer {
        fn emit(&self, _: crate::DebugEvent) {
            self.0.lock().unwrap().push("factory".into());
        }
    }
    impl TestHost for Observer {
        fn resolve(
            &mut self,
            _: &str,
            _: Option<&Path>,
            source: &str,
        ) -> Result<TestSource, String> {
            Ok(TestSource {
                key: source.into(),
                format: SystemDataFormat::Json,
            })
        }
        fn read(&mut self, source: &TestSource, _: usize) -> Result<String, String> {
            self.0.lock().unwrap().push(source.key.clone());
            Ok("1".into())
        }
    }
    let directory = fixture_dir();
    let path = directory.join("main.telora");
    fs::write(
        &path,
        r#"
import "std/test" as test;
export def group = test.with_fixtures(["a", "b", "a"], fn(value) {
    let observed = dbg!(value);
    test.should_ok(fn() { observed })
});
"#,
    )
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let engine = recovery_engine().with_debug_sink(Arc::new(Observer(Arc::clone(&events))));
    let outcome = engine
        .test_with_resolver(
            ModuleResolver::standalone(&path).unwrap(),
            &mut Observer(Arc::clone(&events)),
            TestLimits::default(),
        )
        .unwrap();
    assert!(outcome.passed(), "{:?}", outcome.diagnostics);
    assert_eq!(
        *events.lock().unwrap(),
        ["a", "b", "factory", "factory", "factory"]
    );
    fs::remove_dir_all(directory).unwrap();
}
