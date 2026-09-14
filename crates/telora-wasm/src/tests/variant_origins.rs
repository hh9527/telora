use super::*;
use std::collections::BTreeMap;

fn sources(eol: &str) -> BTreeMap<String, String> {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/variant-origins");
    ["main", "model", "alias", "reexport"]
        .into_iter()
        .map(|name| {
            (
                format!("origins/{name}"),
                std::fs::read_to_string(directory.join(format!("{name}.telora")))
                    .unwrap()
                    .replace('\n', eol),
            )
        })
        .collect()
}

fn build(sources: &BTreeMap<String, String>) -> Mir {
    let inventory = sources
        .keys()
        .map(String::as_str)
        .chain(static_sources::BUILTINS.iter().map(|(name, _)| *name))
        .map(|name| ModuleSpec {
            name: name.into(),
            kind: telora_core::mir::ModuleKind::Source,
            native: static_sources::native_module(name),
            implicit_imports: if name == "std/prelude" {
                vec![]
            } else {
                vec!["std/prelude".into()]
            },
        })
        .collect();
    let mut mir = module_resolve::resolve(inventory, &["origins/main".into()], |_, name| {
        Ok(sources.get(name).cloned().unwrap_or_else(|| {
            static_sources::BUILTINS
                .iter()
                .find(|(module, _)| *module == name)
                .unwrap()
                .1
                .into()
        }))
    });
    symbol_resolve::resolve(&mut mir);
    type_resolve::resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    mir
}

fn expected(mir: &Mir, sources: &BTreeMap<String, String>) -> BTreeMap<String, [u32; 3]> {
    let mut markers = BTreeMap::new();
    for (name, text) in sources {
        let source = mir
            .sources
            .files()
            .find(|file| file.name.as_ref() == name)
            .unwrap();
        let mut offset = 0;
        for line in text.split_inclusive(['\r', '\n']) {
            if let Some((code, marker)) = line.split_once("# origin:") {
                let (label, token) = marker.trim().split_once(':').unwrap();
                let start = offset + code.rfind(token).unwrap();
                let location = telora_core::source::Location {
                    source: source.id(),
                    start: start as u32,
                    end: (start + token.len()) as u32,
                };
                markers.insert(label.into(), source.compact(location).0);
            }
            offset += line.len();
        }
    }
    markers
}

#[test]
fn variants_materialize_at_use_and_value_forwarding_preserves_origins() {
    let mut previous = None;
    for eol in ["\n", "\r\n", "\r"] {
        let sources = sources(eol);
        let mir = build(&sources);
        let expected = expected(&mir, &sources);
        if let Some(previous) = &previous {
            assert_eq!(&expected, previous);
        }
        previous = Some(expected.clone());
        for name in ["observe", "rejected"] {
            let symbol = mir
                .exports
                .iter()
                .flatten()
                .copied()
                .find(|id| mir.symbols[id.index()].name == name)
                .unwrap();
            let bytes = crate::compile_executable(&mir.seal_export(symbol).unwrap()).unwrap();
            let mut session = crate::session::Session::load(&bytes, 20_000_000).unwrap();
            session.initialize().unwrap();
            if name == "rejected" {
                assert!(session.call(&[]).is_err());
                let diagnostics = session.diagnostics().unwrap();
                assert_eq!(diagnostics.len(), 1);
                assert_eq!(diagnostics[0].subjects, [expected["rejected"]]);
                let source = mir
                    .sources
                    .files()
                    .find(|source| source.name.as_ref() == "origins/main")
                    .unwrap();
                let location = source
                    .byte_location(telora_core::source::CompactLoc(diagnostics[0].origin))
                    .unwrap();
                assert!(
                    source
                        .slice(location)
                        .unwrap()
                        .contains("fail!(\"expect True\"")
                );
                continue;
            }
            assert_eq!(session.call(&[]).unwrap(), 42);
            let diagnostics = session.diagnostics().unwrap();
            let cases: &[(&str, &[&str])] = &[
                ("bool", &["first", "second", "first"]),
                ("values", &["disabled", "saved"]),
                ("empty", &["empty", "direct"]),
                ("aliases", &["yes", "missing", "present"]),
                ("none", &["absent", "absent"]),
                ("constructors", &["ctor", "ctor", "wrap", "immediate"]),
                ("payload", &["payload"]),
                ("mapped", &["mapped"]),
                ("mapped-payload", &["payload"]),
                ("generic", &["optional", "optional"]),
                ("widened", &["widened"]),
                ("metadata", &["metadata"]),
                ("explicit", &["explicit"]),
                ("shadow", &["first"]),
            ];
            assert_eq!(diagnostics.len(), cases.len());
            for (diagnostic, (message, origins)) in diagnostics.iter().zip(cases) {
                assert!(diagnostic.warning);
                assert_eq!(&diagnostic.message, message);
                let mut locations = Vec::new();
                for label in *origins {
                    let location = expected[*label];
                    if !locations.contains(&location) {
                        locations.push(location);
                    }
                }
                assert_eq!(diagnostic.subjects, locations, "{message}");
            }
        }
    }
}

#[test]
fn seal_requires_materialization_evidence_and_preserves_value_references() {
    use telora_core::mir::{TypeState, ValueMaterialization};
    let sources = sources("\n");
    let mut mir = build(&sources);
    mir.seal().unwrap();
    let module = mir
        .modules
        .iter()
        .position(|module| module.name == "origins/main")
        .unwrap();
    // Source ranges distinguish the expression reference from binding names.
    let find = |text: &str| {
        mir.hir
            .iter()
            .enumerate()
            .find(|(_, node)| {
                node.module.index() == module
                    && node.resolution.is_some()
                    && mir
                        .sources
                        .get(node.location.source)
                        .slice(node.location)
                        .is_some_and(|slice| slice == text)
            })
            .map(|(index, _)| index)
            .unwrap()
    };
    let literal = find("False");
    let value = find("disabled");
    assert_eq!(
        mir.value_materializations[literal],
        Some(ValueMaterialization::Boolean(false))
    );
    assert_eq!(mir.value_materializations[value], None);
    let fact = mir.value_materializations[literal].take();
    assert!(
        mir.seal().is_err(),
        "missing literal evidence must reject seal"
    );
    mir.value_materializations[literal] = fact;
    mir.value_materializations[value] = fact;
    assert!(
        mir.seal().is_err(),
        "ordinary values cannot acquire literal evidence"
    );
    mir.value_materializations[value] = None;
    let original = mir.ty_slots[literal];
    let wrong = mir
        .types
        .iter()
        .position(|ty| ty.constructor == telora_core::mir::TypeConstructor::Int)
        .unwrap();
    // Obtain an existing TypeId without manufacturing IDs outside the core.
    let wrong = mir
        .ty_slots
        .iter()
        .find_map(|state| match state {
            TypeState::Known(ty) if ty.index() == wrong => Some(*ty),
            _ => None,
        })
        .unwrap();
    mir.ty_slots[literal] = TypeState::Known(wrong);
    assert!(
        mir.seal().is_err(),
        "literal signature must match its identity"
    );
    mir.ty_slots[literal] = original;
    mir.seal().unwrap();
}
