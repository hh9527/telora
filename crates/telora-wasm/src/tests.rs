use telora_core::{
    mir::Mir,
    module_resolve::{self, ModuleSpec},
    static_sources, symbol_resolve, type_resolve,
};

fn graph(source: &str) -> Mir {
    let inventory = ["@src/main", "std/prelude"]
        .into_iter()
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
    let mut mir = module_resolve::resolve(inventory, &["@src/main".into()], |_, name| {
        Ok(if name == "std/prelude" {
            "native type Int @4; export { Int };".into()
        } else {
            source.into()
        })
    });
    symbol_resolve::resolve(&mut mir);
    type_resolve::resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    mir
}

fn compile(source: &str) -> Result<Vec<u8>, String> {
    let mir = graph(source);
    let export = mir
        .exports
        .iter()
        .flatten()
        .copied()
        .find(|id| mir.symbols[id.index()].name == "answer")
        .unwrap();
    let executable = mir.seal_export(export).unwrap();
    super::compile_scalar(&executable)
}

#[test]
fn sealed_export_runs_without_mir_or_host_imports() {
    // compile() drops the entire source/MIR before the engine sees the bytes.
    let bytes = compile("export def answer = 42;").unwrap();
    assert_eq!(bytes, compile("export def answer = 42;").unwrap());
    let engine = wasmi::Engine::default();
    let module = wasmi::Module::new(&engine, &bytes[..]).unwrap();
    assert_eq!(module.imports().count(), 0);
    let mut store = wasmi::Store::new(&engine, ());
    let linker = wasmi::Linker::new(&engine);
    let instance = linker.instantiate_and_start(&mut store, &module).unwrap();
    let entry = instance
        .get_typed_func::<(), i64>(&store, "telora_entry")
        .unwrap();
    assert_eq!(entry.call(&mut store, ()).unwrap(), 42);
}

#[test]
fn unsupported_expression_is_not_executed_by_another_backend() {
    assert!(
        compile("export def answer = 1 + 2;")
            .unwrap_err()
            .contains("does not support")
    );
}
