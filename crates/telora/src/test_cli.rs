use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use telora_core::module::{TestHost, TestLimits, TestSource};
use telora_core::{ModuleResolver, ResolvedWorkspace};

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
    let workspace = crate::package_host::prepare(&context)?;
    let resolver =
        ModuleResolver::from_workspace(Arc::clone(&workspace), &context, &format!("@test/{name}"))
            .map_err(|e| e.to_string())?;
    let mut warnings = Vec::new();
    for (crate_name, _) in workspace.crates() {
        for undeclared in workspace
            .undeclared_modules(crate_name)
            .map_err(|e| e.to_string())?
        {
            warnings.push(format!(
                "crate {:?} contains undeclared module file {}; add {:?} to telora-crate.json modules",
                undeclared.crate_name, undeclared.relative_path.display(), undeclared.selector,
            ));
        }
    }
    let outcome = crate::engine()
        .test_with_resolver(
            resolver,
            &mut FileTestHost { workspace },
            TestLimits::default(),
        )
        .map_err(|e| e.to_string())?;
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
                let source = outcome.sources.get(label.location.source);
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
        json!({"schema": "telora.test/v2", "record": "diagnostic", "module": outcome.module,
            "severity": severity, "message": diagnostic.message, "labels": labels, "notes": diagnostic.notes})
    };
    for message in warnings {
        crate::emit(json!({"schema": "telora.test/v2", "record": "diagnostic",
            "module": outcome.module, "severity": "warning", "message": message,
            "labels": [], "notes": []}))?;
    }
    for diagnostic in &outcome.diagnostics {
        crate::emit(diagnostic_record(diagnostic))?;
    }
    let emit_case_diagnostics = |case: &telora_core::module::TestCase| -> Result<(), String> {
        for diagnostic in &case.diagnostics {
            let mut record = diagnostic_record(diagnostic);
            record["test"] = json!(case.test);
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
            json!({"schema": "telora.test/v2", "record": "case", "module": outcome.module,
            "test": case.test, "fixtures": case.fixtures, "sources": case.sources,
            "status": if case.passed { "passed" } else { "failed" }}),
        )?;
    }
    let passed = outcome.cases.iter().filter(|case| case.passed).count();
    crate::emit(
        json!({"schema": "telora.test/v2", "record": "summary", "module": outcome.module,
        "status": if outcome.passed() { "ok" } else { "error" }, "total": outcome.cases.len(),
        "passed": passed, "failed": outcome.cases.len() - passed, "aborted": outcome.aborted }),
    )?;
    Ok(i32::from(!outcome.passed()))
}
