//! CLI contracts are checked against admitted standard exports before codegen.
use telora_core::mir::{Mir, ResolveState, SymbolId, TypeConstructor, TypeState};

pub(super) fn validate(mir: &Mir, symbol: SymbolId) -> Result<(), String> {
    let ResolveState::Bound(target) = mir.symbols[symbol.index()].resolution else {
        return Err("unresolved eval export".into());
    };
    if !mir.symbol_generics[target.index()].is_empty() {
        return Err("eval export must not be polymorphic".into());
    }
    let message = "eval export: expected Value (std/value.Value)";
    let (module, name) = ("std/value", "Value");
    let expected = mir
        .modules
        .iter()
        .position(|m| m.name == module)
        .and_then(|module| {
            mir.exports[module]
                .iter()
                .find(|id| mir.symbols[id.index()].name == name)
        })
        .and_then(
            |id| match mir.ty_slots[mir.symbol_types[id.index()].index()] {
                TypeState::Known(meta)
                    if mir.types[meta.index()].constructor == TypeConstructor::Meta =>
                {
                    mir.types[meta.index()].arguments.first().copied()
                }
                _ => None,
            },
        )
        .ok_or(message)?;
    if mir.ty_slots[mir.symbol_types[target.index()].index()] != TypeState::Known(expected) {
        return Err(message.into());
    }
    Ok(())
}
