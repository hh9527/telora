//! Test scheduling and reports are Host protocol; callbacks stay in Wasm.
use super::test_fixtures::{Fixtures, Input, diagnostic, location};
use telora_core::{
    DataLimits, Diagnostic, Loc, SourceDatabase, TestContext,
    source::Severity,
    test_plan::{TestNotice, TestPlan, TestReport, TestResult},
};
use telora_wasm::{
    session::Session,
    testing::{Description, TestSession},
    transport::Value,
};

enum Work {
    Value {
        value: Value,
        result: TestResult,
        depth: usize,
        origin: Option<Loc>,
    },
    Fixture {
        plan: Result<Input, Vec<Diagnostic>>,
        factory: Value,
        result: TestResult,
        depth: usize,
        origin: Option<Loc>,
    },
}

impl Work {
    fn root(&mut self) -> &mut Value {
        match self {
            Self::Value { value, .. } => value,
            Self::Fixture { factory, .. } => factory,
        }
    }
}

pub(crate) fn run(
    session: Session,
    plan: TestPlan,
    sources: &mut SourceDatabase,
    mut context: TestContext<'_>,
    limits: DataLimits,
) -> Result<TestReport, String> {
    let mut testing = TestSession::new(session)?;
    let mut report = TestReport::default();
    let mut pending = vec![];
    for case in plan.exports.into_iter().rev() {
        pending.push(Work::Value {
            value: testing
                .session()
                .initialized_global(case.target.index() as u32)?,
            result: TestResult {
                name: case.name,
                fixtures: vec![],
                sources: vec![],
                phase: "initialization",
                passed: false,
                diagnostics: vec![],
            },
            depth: 0,
            origin: Some(case.location),
        });
    }
    let mut fixtures = Fixtures {
        context: &mut context,
        limits,
        sources,
        admitted_bytes: 0,
    };
    let mut expanded = 0;
    while !pending.is_empty() {
        if report.aborted {
            break;
        }
        // Keep only pending descriptions/factories and their exact closure graph.
        // Consumed case temporaries and diagnostics cannot accumulate between cases.
        let roots = pending
            .iter_mut()
            .map(|work| *work.root())
            .collect::<Vec<_>>();
        match testing.session_mut().collect_work(&roots) {
            Ok((relocated, _)) => {
                for (work, root) in pending.iter_mut().zip(relocated) {
                    *work.root() = root;
                }
            }
            Err(message) => {
                report.diagnostics.push(super::error(message));
                report.aborted = true;
                break;
            }
        }
        let work = pending.pop().unwrap();
        let (value, mut result, depth, fallback) = match work {
            Work::Value {
                value,
                result,
                depth,
                origin,
            } => (value, result, depth, origin),
            Work::Fixture {
                plan,
                factory,
                mut result,
                depth,
                origin,
            } => {
                if expanded >= fixtures.context.limits.cases {
                    fail(
                        &mut report,
                        result,
                        "test expansion limit exceeded",
                        origin,
                        true,
                    );
                    continue;
                }
                let plan = match plan {
                    Ok(plan) => plan,
                    Err(diagnostics) => {
                        expanded += 1;
                        result.diagnostics.extend(diagnostics);
                        report.cases.push(result);
                        continue;
                    }
                };
                let input = testing
                    .session_mut()
                    .parse_data_source(fixtures.sources.get(plan.source), plan.format);
                let input = match input {
                    Ok(Ok(input)) => input,
                    Ok(Err(events)) => {
                        expanded += 1;
                        result
                            .diagnostics
                            .extend(super::diagnostics::parsed(events, fixtures.sources)?);
                        report.cases.push(result);
                        continue;
                    }
                    Err(message) => {
                        fail(&mut report, result, message, origin, true);
                        continue;
                    }
                };
                result.phase = "factory";
                let execution = testing.invoke(factory, &[input])?;
                result.diagnostics.extend(super::diagnostics::convert(
                    execution.diagnostics,
                    fixtures.sources,
                ));
                flush_debug(testing.session())?;
                if let Some(message) = execution.terminal {
                    fail(&mut report, result, message, origin, true);
                } else if let Some(value) = execution.value {
                    notice(&mut report, &mut result);
                    result.phase = "discovery";
                    pending.push(Work::Value {
                        value,
                        result,
                        depth: depth + 1,
                        origin,
                    });
                } else {
                    expanded += 1;
                    report.cases.push(result);
                }
                continue;
            }
        };
        expanded += 1;
        if expanded > fixtures.context.limits.cases || depth > fixtures.context.limits.depth {
            fail(
                &mut report,
                result,
                "test expansion limit exceeded",
                fallback,
                true,
            );
            continue;
        }
        let description = testing.describe(value)?;
        let origin = location(fixtures.sources, description.origin).or(fallback);
        if let Description::Fixtures { sources, factory } = description.kind {
            result.phase = "discovery";
            if sources.is_empty() {
                fail(&mut report, result, "no fixtures", origin, false);
                continue;
            }
            if sources.len() > fixtures.context.limits.cases.saturating_sub(expanded) {
                fail(
                    &mut report,
                    result,
                    "test expansion limit exceeded",
                    origin,
                    true,
                );
                continue;
            }
            notice(&mut report, &mut result);
            // Snapshot every immediate input before invoking any factory.
            let mut cache = std::collections::HashMap::new();
            let mut children = vec![];
            for (index, label) in sources.iter().enumerate() {
                let mut child = result.clone();
                child.fixtures.push(index);
                child.sources.push(label.clone());
                child.phase = "fixture";
                let plan = fixtures.prepare(&plan.module_name, &child, label, origin, &mut cache);
                if fixtures.admitted_bytes > fixtures.context.limits.fixture_bytes {
                    child.diagnostics.extend(plan.err().unwrap_or_default());
                    report.cases.push(child);
                    report.aborted = true;
                    break;
                }
                children.push(Work::Fixture {
                    plan,
                    factory,
                    result: child,
                    depth,
                    origin,
                });
            }
            pending.extend(children.into_iter().rev());
            continue;
        }
        result.phase = "execution";
        let (callback, expected) = match description.kind {
            Description::ShouldOk(callback) => (callback, None),
            Description::ShouldFail(callback) => (callback, Some(None)),
            Description::ShouldFailWith(callback, text) => (callback, Some(Some(text))),
            Description::Fixtures { .. } => unreachable!(),
        };
        let execution = testing.invoke(callback, &[])?;
        flush_debug(testing.session())?;
        let terminal = execution.terminal.is_some();
        let language_failed = execution.value.is_none() && !terminal;
        result.passed = !terminal
            && match &expected {
                None => !language_failed,
                Some(None) => language_failed,
                Some(Some(text)) => {
                    language_failed
                        && execution
                            .diagnostics
                            .iter()
                            .any(|d| !d.warning && d.message.contains(text))
                }
            };
        let diagnostics = super::diagnostics::convert(execution.diagnostics, fixtures.sources);
        result.diagnostics.extend(
            diagnostics
                .into_iter()
                .filter(|d| !result.passed || expected.is_none() || d.severity != Severity::Error),
        );
        if let Some(message) = execution.terminal {
            result.diagnostics.push(diagnostic(message, origin));
        }
        if !result.passed
            && !terminal
            && let Some(expected) = expected
        {
            let message = expected
                .map(|text| format!("expected a recoverable failure containing {text:?}"))
                .unwrap_or_else(|| {
                    "expected a recoverable failure, but thunk returned normally".into()
                });
            result.diagnostics.push(diagnostic(message, origin));
        }
        report.aborted |= terminal;
        report.cases.push(result);
    }
    Ok(report)
}

fn flush_debug(session: &Session) -> Result<(), String> {
    for event in session.take_debug_events()? {
        eprintln!(
            "{}",
            serde_json::to_string(&event).map_err(|e| e.to_string())?
        );
    }
    Ok(())
}

fn notice(report: &mut TestReport, result: &mut TestResult) {
    if !result.diagnostics.is_empty() {
        report.notices.push(TestNotice {
            before_case: report.cases.len(),
            context: result.clone(),
        });
        result.diagnostics.clear();
    }
}

fn fail(
    report: &mut TestReport,
    mut result: TestResult,
    message: impl Into<String>,
    origin: Option<Loc>,
    terminal: bool,
) {
    result.diagnostics.push(diagnostic(message, origin));
    report.cases.push(result);
    report.aborted |= terminal;
}
