//! Test-only audit of existing language callbacks against default observations.
use super::*;
use crate::{abi::*, output::Output, session::Session};
use std::{collections::BTreeMap, path::Path};

fn sources(
    directory: &Path,
    root: &Path,
    files: &mut BTreeMap<String, String>,
    data: &mut Vec<String>,
) {
    let mut entries = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            sources(&path, root, files, data);
            continue;
        }
        let relative = path.strip_prefix(root).unwrap().to_str().unwrap();
        if let Some(name) = relative.strip_suffix(".telora") {
            files.insert(
                format!("@src/{name}"),
                std::fs::read_to_string(path).unwrap(),
            );
        } else if matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("json" | "yaml" | "yml" | "toml")
        ) {
            data.push(format!("@src/{relative}"));
        }
    }
}

#[test]
#[ignore = "run scripts/test-language.sh first; audits its case observations"]
fn published_language_callbacks_match_default_observations() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(audit)
        .unwrap()
        .join()
        .unwrap();
}

fn audit() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source_root = repository.join("tests/language/src");
    let observations = repository.join("target/language-tests/actual");
    let mut files = BTreeMap::new();
    let mut data = Vec::new();
    sources(&source_root, &source_root, &mut files, &mut data);
    for &(name, source) in static_sources::BUILTINS {
        files.insert(name.into(), source.into());
    }
    let filter = std::env::var("TELORA_WASM_LANGUAGE_FILTER").unwrap_or_default();
    let mut checked = 0;
    let mut fixtures = 0;
    let mut failures = Vec::new();
    for name in files
        .keys()
        .filter(|name| name.starts_with("@src/test/") && name.ends_with("/testee"))
    {
        let case = name
            .strip_prefix("@src/test/")
            .unwrap()
            .strip_suffix("/testee")
            .unwrap();
        if !case.contains(&filter) {
            continue;
        }
        let observed = std::fs::read_to_string(
            observations.join(format!("test__{}.stdout.jsonl", case.replace('/', "__"))),
        )
        .unwrap();
        let cases = observed
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|record| record["record"] == "case")
            .collect::<Vec<_>>();
        if cases.is_empty() {
            continue;
        }
        eprintln!("wasm language compile: {case}");
        let result = module(
            &source_root,
            &files,
            &data,
            name,
            &cases,
            &mut checked,
            &mut fixtures,
            &mut failures,
        );
        if let Err(error) = result {
            eprintln!("wasm language rejected: {case}: {error}");
            failures.push(format!("{case}: {error}"));
        }
    }
    eprintln!("wasm language audit: checked={checked}, including fixture cases={fixtures}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(checked > 0, "no observations executed");
}

fn module(
    root: &Path,
    files: &BTreeMap<String, String>,
    data: &[String],
    name: &str,
    cases: &[serde_json::Value],
    checked: &mut usize,
    fixtures: &mut usize,
    failures: &mut Vec<String>,
) -> Result<(), String> {
    let inventory = files
        .keys()
        .chain(data)
        .map(|name| ModuleSpec {
            name: name.clone(),
            kind: if data.contains(name) {
                telora_core::mir::ModuleKind::Data
            } else {
                telora_core::mir::ModuleKind::Source
            },
            native: static_sources::native_module(name),
            implicit_imports: if name == "std/prelude" {
                vec![]
            } else {
                vec!["std/prelude".into()]
            },
        })
        .collect();
    let mut mir = module_resolve::resolve(inventory, &[name.into()], |_, name| {
        files
            .get(name)
            .cloned()
            .ok_or_else(|| format!("missing source {name}"))
    });
    symbol_resolve::resolve(&mut mir);
    type_resolve::resolve(&mut mir);
    let mut plans = Vec::new();
    let loaded = mir
        .modules
        .iter()
        .filter(|module| matches!(module.state, telora_core::mir::ModuleState::Data { .. }))
        .map(|module| module.name.clone())
        .collect::<Vec<_>>();
    for name in loaded {
        let relative = name.strip_prefix("@src/").ok_or("unexpected data module")?;
        let text = std::fs::read_to_string(root.join(relative)).map_err(|e| e.to_string())?;
        let source = mir.sources.add(name.clone(), &text);
        let format = match Path::new(relative).extension().and_then(|ext| ext.to_str()) {
            Some("json") => telora_core::data_plan::Format::Json,
            Some("toml") => telora_core::data_plan::Format::Toml,
            _ => telora_core::data_plan::Format::Yaml,
        };
        let plan = telora_core::data_plan::parse_registered(&mir.sources, source, format)
            .map_err(|ds| format!("data: {ds:?}"))?;
        plans.push((name, plan));
    }
    let sealed = mir.seal().map_err(|ds| format!("seal: {ds:?}"))?;
    let telora_core::mir::ModuleTarget::Bound(module) = mir.roots[0] else {
        return Err("root unresolved".into());
    };
    let tests = telora_core::test_plan::TestPlan::from_mir(&sealed, module)
        .map_err(|ds| format!("tests: {ds:?}"))?;
    // Match check --wasm: initialize the loaded graph, including capability
    // properties and construction checkers declared in dependency modules.
    let modules = mir
        .hir
        .iter()
        .map(|node| node.module)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let executable = sealed
        .seal_modules(&modules)
        .map_err(|ds| format!("executable: {ds:?}"))?;
    let plan = crate::plan::Plan::new(&executable)?;
    let bytes = crate::compile_executable(&executable)?;
    for record in cases {
        let name = record["test"].as_str().ok_or("missing test name")?;
        let export = tests
            .exports
            .iter()
            .find(|export| export.name == name)
            .ok_or("missing test export")?;
        let mut session = Session::load(&bytes, 100_000_000)?;
        for data in session.manifest.data_modules.clone() {
            let input = &plans
                .iter()
                .find(|(name, _)| *name == data.name)
                .ok_or("missing data plan")?
                .1;
            session.inject_data(data.symbol, input)?;
        }
        session.initialize()?;
        let offset = plan.demands[&plan.globals[&export.target]];
        let output = Output {
            memory: session.memory.data(&session.store),
            manifest: &session.manifest,
        };
        if output.word(offset as u64)? != 2 {
            return Err("test export not initialized".into());
        }
        let value = output.word(offset as u64 + 4)? as u64;
        let selection =
            super::language_fixtures::select(&mut session, value, record, root, &mir.sources)?;
        if !record["fixtures"]
            .as_array()
            .ok_or("missing fixture list")?
            .is_empty()
        {
            *fixtures += 1;
        }
        let Some(value) = selection else {
            *checked += 1;
            if record["status"] != "failed" {
                failures.push(format!(
                    "{}/{name}: fixture failed before leaf execution",
                    tests.module_name
                ));
            }
            continue;
        };
        let output = Output {
            memory: session.memory.data(&session.store),
            manifest: &session.manifest,
        };
        let (description, _) = output.payload(TESTS, output.word(value + DATA)?)?;
        let operation = output.word(description)?;
        let callback = output.word(description + 8)?;
        let expected = if operation == 2 {
            Some(output.text(output.word(description + 12)? as u64)?)
        } else {
            None
        };
        let start = session.diagnostics()?.len();
        eprintln!("wasm language call: {}/{name}", tests.module_name);
        let invoke = session
            .instance
            .get_typed_func::<(i32, i32), i32>(&session.store, "telora_invoke")
            .map_err(|e| e.to_string())?;
        let result = invoke.call(&mut session.store, (callback as i32, 0));
        let reports = session.diagnostics()?;
        let reports = &reports[start..];
        let passed = match operation {
            0 => result.as_ref().is_ok_and(|value| *value != 0),
            1 => result.as_ref().is_ok_and(|value| *value == 0),
            2 => {
                result.as_ref().is_ok_and(|value| *value == 0)
                    && reports
                        .iter()
                        .any(|report| report.message.contains(expected.as_deref().unwrap()))
            }
            _ => return Err("invalid test operation".into()),
        };
        *checked += 1;
        if result.is_err() || passed != (record["status"] == "passed") {
            failures.push(format!("{}/{name}: expected {}, Wasm passed={passed}, result={result:?}, reports={reports:?}", tests.module_name, record["status"]));
        }
    }
    Ok(())
}
