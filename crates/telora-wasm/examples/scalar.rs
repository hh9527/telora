//! Build the initial portable scalar fixture, without the CLI integration.
use telora_core::{
    module_resolve::{self, ModuleSpec},
    static_sources, symbol_resolve, type_resolve,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("expected output .wasm filename")?;
    let source = std::env::args_os()
        .nth(2)
        .map(std::fs::read_to_string)
        .transpose()?
        .unwrap_or_else(|| "export def answer = 42;".into());
    let inventory = std::iter::once("@src/main")
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
    let mut mir = module_resolve::resolve(inventory, &["@src/main".into()], |_, name| {
        Ok(if name == "@src/main" {
            &source
        } else {
            static_sources::BUILTINS
                .iter()
                .find(|(module, _)| *module == name)
                .ok_or_else(|| format!("missing builtin module {name}"))?
                .1
        }
        .into())
    });
    symbol_resolve::resolve(&mut mir);
    type_resolve::resolve(&mut mir);
    let export = mir
        .exports
        .iter()
        .flatten()
        .copied()
        .find(|id| mir.symbols[id.index()].name == "answer")
        .ok_or("missing export")?;
    let executable = mir
        .seal_export(export)
        .map_err(|diagnostics| format!("{diagnostics:?}"))?;
    let bytes = telora_wasm::compile_executable(&executable)?;
    std::fs::write(path, bytes)?;
    Ok(())
}
