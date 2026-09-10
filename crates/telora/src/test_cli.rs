use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use telora_core::test_plan::{TestReport, TestResult};
use telora_core::{ResolvedWorkspace, TestContext, TestHost, TestSource};

struct FileTestHost {
    workspace: Arc<ResolvedWorkspace>,
}

impl TestHost for FileTestHost {
    fn resolve(
        &mut self,
        _module: &str,
        declaring_path: Option<&Path>,
        source: &str,
    ) -> Result<TestSource, String> {
        let spec = crate::source_arg::parse_fixture_source(source)?;
        let path = match spec.src.split_once("://") {
            Some((scheme, path)) if scheme.starts_with("file+") => path,
            Some(_) => return Err("fixtures require local file sources".into()),
            None => spec.src.as_str(),
        };
        let path = Path::new(path);
        if path.is_absolute() {
            return Err("absolute fixture paths are not permitted".into());
        }
        let declaring = declaring_path.ok_or("fixture has no physical module base")?;
        let owner = self
            .workspace
            .crate_for_path(declaring)
            .map_err(|_| "fixture has no declaring crate")?;
        let root = self
            .workspace
            .crate_root(owner)
            .ok_or("fixture has no declaring crate")?;
        let root = root
            .canonicalize()
            .map_err(|_| "cannot resolve fixture crate root")?;
        let resolved = declaring
            .parent()
            .ok_or("fixture has no module directory")?
            .join(path)
            .canonicalize()
            .map_err(|_| "cannot resolve fixture file")?;
        if !resolved.starts_with(&root) {
            return Err("fixture path escapes its declaring crate".into());
        }
        if !resolved
            .metadata()
            .map_err(|_| "cannot inspect fixture file")?
            .is_file()
        {
            return Err("fixture source must be a regular file".into());
        }
        Ok(TestSource {
            key: resolved.to_string_lossy().into_owned(),
            format: spec.format,
        })
    }

    fn read(&mut self, source: &TestSource, max_bytes: usize) -> Result<String, String> {
        let canonical = Path::new(&source.key)
            .canonicalize()
            .map_err(|_| "cannot resolve fixture file")?;
        if canonical != Path::new(&source.key) {
            return Err("fixture file changed after resolution".into());
        }
        let file = std::fs::File::open(&source.key).map_err(|_| "cannot read fixture file")?;
        let bytes = crate::source_arg::read_limited(file, max_bytes, "fixture source")?;
        String::from_utf8(bytes).map_err(|_| "fixture source is not UTF-8".into())
    }
}

pub(crate) fn run(context: PathBuf, name: &str) -> Result<i32, String> {
    let mut inventory = crate::static_input::Inventory::new(&context, false)?;
    let root = inventory.select(&format!("@test/{name}"))?;
    if !inventory.entries.contains_key(&root) {
        return Err(format!("unknown test module {root:?}"));
    }
    let warnings = inventory.undeclared_warnings()?;
    let mut mir = inventory.solve(&root);
    let compiled = mir.seal().and_then(|sealed| {
        let telora_core::mir::ModuleTarget::Bound(module) = mir.roots[0] else {
            unreachable!("sealed root must be resolved");
        };
        telora_core::codegen::compile_tests(sealed, module)
    });
    let compiled = match compiled {
        Ok(compiled) => compiled,
        Err(diagnostics) => {
            for diagnostic in diagnostics {
                if !mir.diagnostics.contains(&diagnostic) {
                    mir.diagnostics.push(diagnostic);
                }
            }
            return emit_report(
                &root,
                &mir.sources,
                TestReport {
                    diagnostics: mir.diagnostics,
                    aborted: true,
                    ..Default::default()
                },
                warnings,
            );
        }
    };
    let config = crate::engine_config();
    let linked =
        match telora_core::execution_link::link_entry_with_data(compiled.bootstrap, |link| {
            inventory.read_data(link, config.data_limits.file_size)
        }) {
            Ok(linked) => linked,
            Err(diagnostics) => {
                return emit_report(
                    &root,
                    &mir.sources,
                    TestReport {
                        diagnostics,
                        aborted: true,
                        ..Default::default()
                    },
                    warnings,
                );
            }
        };
    let mut host = FileTestHost {
        workspace: inventory.workspace().ok_or("test requires a workspace")?,
    };
    let mut report = telora_core::Vm::new()
        .with_debug_sink(Arc::new(crate::StderrDebugSink))
        .test_linked(
            linked,
            compiled.plan,
            config.session_quota,
            config.data_limits,
            &mut mir.sources,
            TestContext {
                host: Some(&mut host),
                module_paths: inventory.module_paths(),
                ..Default::default()
            },
        )?;
    report.diagnostics.splice(0..0, mir.diagnostics);
    emit_report(&root, &mir.sources, report, warnings)
}

fn emit_report(
    module: &str,
    sources: &telora_core::SourceDatabase,
    outcome: TestReport,
    warnings: Vec<String>,
) -> Result<i32, String> {
    let diagnostic_record = |diagnostic: &telora_core::source::Diagnostic| {
        let severity = match diagnostic.severity {
            telora_core::source::Severity::Error => "error",
            telora_core::source::Severity::Warning => "warning",
            telora_core::source::Severity::Info => "info",
        };
        let labels = diagnostic
            .labels
            .iter()
            .map(|label| {
                let source = sources.get(label.location.source);
                let start = source
                    .text()
                    .position(label.location.start, telora_core::PositionEncoding::Utf8)
                    .unwrap();
                let end = source
                    .text()
                    .position(label.location.end, telora_core::PositionEncoding::Utf8)
                    .unwrap();
                json!({"source": source.name.as_ref(), "location": {
                "line": start.line + 1, "column": start.character,
                "end_line": end.line + 1, "end_column": end.character,
            }, "message": label.message, "primary": label.primary})
            })
            .collect::<Vec<_>>();
        json!({"schema": "telora.test/v2", "record": "diagnostic", "module": module,
            "severity": severity, "message": diagnostic.message, "labels": labels, "notes": diagnostic.notes})
    };
    for message in warnings {
        crate::emit(json!({"schema": "telora.test/v2", "record": "diagnostic",
            "module": module, "severity": "warning", "message": message,
            "labels": [], "notes": []}))?;
    }
    for diagnostic in &outcome.diagnostics {
        crate::emit(diagnostic_record(diagnostic))?;
    }
    let emit_case_diagnostics = |case: &TestResult| -> Result<(), String> {
        for diagnostic in &case.diagnostics {
            let mut record = diagnostic_record(diagnostic);
            record["test"] = json!(case.name);
            record["fixtures"] = json!(case.fixtures);
            record["sources"] = json!(case.sources);
            record["phase"] = json!(case.phase);
            crate::emit(record)?;
        }
        Ok(())
    };
    let mut notices = outcome.notices.iter().peekable();
    for (index, case) in outcome.cases.iter().enumerate() {
        while notices
            .peek()
            .is_some_and(|notice| notice.before_case == index)
        {
            emit_case_diagnostics(&notices.next().unwrap().context)?;
        }
        emit_case_diagnostics(case)?;
        crate::emit(
            json!({"schema": "telora.test/v2", "record": "case", "module": module,
            "test": case.name, "fixtures": case.fixtures, "sources": case.sources,
            "status": if case.passed { "passed" } else { "failed" }}),
        )?;
    }
    for notice in notices {
        emit_case_diagnostics(&notice.context)?;
    }
    let passed = outcome.cases.iter().filter(|case| case.passed).count();
    crate::emit(
        json!({"schema": "telora.test/v2", "record": "summary", "module": module,
        "status": if outcome.passed() { "ok" } else { "error" }, "total": outcome.cases.len(),
        "passed": passed, "failed": outcome.cases.len() - passed, "aborted": outcome.aborted }),
    )?;
    Ok(i32::from(!outcome.passed()))
}
