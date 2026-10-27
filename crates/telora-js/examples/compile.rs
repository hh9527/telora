//! cargo run -p telora-js --example compile -- input.telora export output.mjs
use telora_core::{
    mir::*,
    module_resolve::{self, ModuleSpec},
    static_sources, symbol_resolve, type_resolve,
};
fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("expected INPUT.telora EXPORT OUTPUT.mjs".into());
    }
    let source = std::fs::read_to_string(&args[0]).map_err(|e| e.to_string())?;
    let inventory = std::iter::once("@src/main")
        .chain(static_sources::BUILTINS.iter().map(|(name, _)| *name))
        .map(|name| ModuleSpec {
            name: name.into(),
            kind: ModuleKind::Source,
            native: static_sources::native_module(name),
            implicit_imports: if name == "std/prelude" {
                vec![]
            } else {
                vec!["std/prelude".into()]
            },
        })
        .collect();
    let mut mir = module_resolve::resolve(inventory, &["@src/main".into()], |_, name| {
        if name == "@src/main" {
            Ok(source.clone())
        } else {
            static_sources::BUILTINS
                .iter()
                .find(|(module, _)| *module == name)
                .map(|(_, text)| (*text).to_owned())
                .ok_or_else(|| format!("unknown module {name}"))
        }
    });
    symbol_resolve::resolve(&mut mir);
    type_resolve::resolve(&mut mir);
    if !mir.diagnostics.is_empty() {
        return Err(mir
            .diagnostics
            .iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n"));
    }
    let export = mir
        .exports
        .iter()
        .flatten()
        .copied()
        .find(|id| mir.symbols[id.index()].name == args[1])
        .ok_or("missing export")?;
    let executable = mir.seal_export(export).map_err(|ds| {
        ds.iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let js = telora_js::compile_executable(&executable)?;
    std::fs::write(&args[2], js).map_err(|e| e.to_string())
}
