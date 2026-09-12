//! Test-only execution audit of existing language assets, not a native test CLI.
use super::*;
use std::path::Path;

fn sources(directory: &Path, root: &Path, files: &mut Vec<(String, String)>, data: &mut Vec<String>) {
    let mut entries = std::fs::read_dir(directory).unwrap().map(|entry| entry.unwrap().path()).collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() { sources(&path, root, files, data); continue; }
        let relative = path.strip_prefix(root).unwrap().to_str().unwrap();
        if let Some(name) = relative.strip_suffix(".telora") {
            files.push((format!("@src/{name}"), std::fs::read_to_string(path).unwrap()));
        } else if matches!(path.extension().and_then(|extension| extension.to_str()), Some("json" | "yaml" | "yml" | "toml")) {
            data.push(format!("@src/{relative}"));
        }
    }
}

#[test]
#[ignore = "run scripts/test-language.sh first; executes its non-fixture observations"]
fn published_language_test_closures_match_default_observations() {
    // Match the CLI's 8 MiB stack while compiling large modules. The standard
    // Rust test worker stack is too small for this whole-language audit.
    std::thread::Builder::new().stack_size(8 * 1024 * 1024)
        .spawn(audit).unwrap().join().unwrap();
}

fn audit() {
    use crate::{abi::CallContext, jit};
    use telora_core::{data_plan, static_sources};
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source_root = repository.join("tests/language/src");
    let observations = repository.join("target/language-tests/actual");
    let mut files = Vec::new();
    let mut data = Vec::new();
    sources(&source_root, &source_root, &mut files, &mut data);
    let dependencies = files.iter().map(|(name, text)| (name.as_str(), text.as_str()))
        .chain(static_sources::BUILTINS.iter().copied()).collect::<Vec<_>>();
    let data_names = data.iter().map(String::as_str).collect::<Vec<_>>();
    let mut checked = 0;
    let mut fixtures = 0;
    let mut failures = Vec::new();
    for (name, _) in files.iter().filter(|(name, _)| name.starts_with("@src/test/") && name.ends_with("/testee")) {
        let case = name.strip_prefix("@src/test/").unwrap().strip_suffix("/testee").unwrap();
        let observed = std::fs::read_to_string(observations.join(format!("test__{}.stdout.jsonl", case.replace('/', "__")))).unwrap();
        let cases = observed.lines().map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|record| record["record"] == "case").collect::<Vec<_>>();
        if cases.is_empty() { continue; } // Syntax/initialization rejection, covered by CLI checks.
        eprintln!("native language compile: {case}");
        let mut mir = crate::test_support::graph_with_data(&format!("import \"{name}\" as subject; export def answer = ();"), &dependencies, &data_names);
        let module = mir.modules.iter().position(|module| module.name == *name).unwrap();
        let mut plans = Vec::new();
        for entry in &mir.modules {
            if !matches!(entry.state, telora_core::mir::ModuleState::Data { .. }) { continue; }
            let relative = entry.name.strip_prefix("@src/").unwrap();
            let text = std::fs::read_to_string(source_root.join(relative)).unwrap();
            let source = mir.sources.add(entry.name.clone(), &text);
            let format = match Path::new(relative).extension().unwrap().to_str().unwrap() {
                "json" => data_plan::Format::Json, "toml" => data_plan::Format::Toml, _ => data_plan::Format::Yaml,
            };
            plans.push((entry.name.clone(), data_plan::parse_registered(&mir.sources, source, format).unwrap()));
        }
        let sealed = mir.seal().unwrap();
        let module_id = mir.hir.iter().find(|node| node.module.index() == module).unwrap().module;
        let executable = sealed.seal_modules(&[module_id]).unwrap();
        let compiled = match jit::compile_executable(&executable) {
            Ok(compiled) => compiled,
            Err(error) => { failures.push(format!("{case}: compile: {error}")); continue; }
        };
        for record in cases {
            if !record["fixtures"].as_array().unwrap().is_empty() { fixtures += 1; continue; }
            let name = record["test"].as_str().unwrap();
            let runtime = Runtime::new(executable.sealed_mir()).unwrap().with_allocation_limit(256 * 1024 * 1024);
            let mut context = CallContext::with_runtime(runtime).with_fuel(1_000_000).with_stack_limit(65_536);
            for module in compiled.data_modules() {
                let plan = &plans.iter().find(|(name, _)| *name == module.name).unwrap().1;
                compiled.inject_data(&mut context, module.symbol, plan).unwrap();
            }
            context.runtime_mut().unwrap().register_source_names(&mir.sources);
            if let Err(error) = compiled.initialize(&mut context) {
                failures.push(format!("{case}/{name}: initialize: {error}: {:?}", context.diagnostics()));
                break;
            }
            let symbol = *mir.exports[module].iter().find(|symbol| mir.symbols[symbol.index()].name == name).unwrap();
            let test = compiled.export(&mut context, symbol).unwrap();
            let rt = context.runtime().unwrap();
            let description = rt.test_description(&test).unwrap();
            if description.operation == 3 { fixtures += 1; continue; }
            let operation = description.operation;
            let thunk = Value { arena: rt.identity, words: description.inputs[0].clone() };
            let expected_message = if operation == 2 {
                Some(rt.text(ValueRef { arena: rt.identity, words: &description.inputs[1] }).unwrap().as_str().to_owned())
            } else { None };
            let start = context.diagnostics().len();
            eprintln!("native language call: {case}/{name}");
            let result = compiled.call_closure(&mut context, &thunk, &[]);
            let reports = &context.diagnostics()[start..];
            let passed = match operation {
                0 => result.is_ok(),
                1 => result.is_err() && !context.is_aborted(),
                2 => result.is_err() && !context.is_aborted() && reports.iter().any(|report| report.message.contains(expected_message.as_deref().unwrap())),
                _ => unreachable!(),
            };
            checked += 1;
            if passed != (record["status"] == "passed") {
                failures.push(format!("{case}/{name}: expected {}, native passed={passed}, result={:?}, reports={reports:?}", record["status"], result.as_ref().err()));
            }
        }
    }
    eprintln!("native language closures: checked={checked}, fixture cases outside this harness={fixtures}");
    assert!(checked > 0, "no language observations were executed");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
