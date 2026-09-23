//! Static service entry plan, shared by execution backends before code generation.
use crate::mir::{Mir, ResolveState, SymbolId, TypeConstructor, TypeId, TypeOperation, TypeState};

/// Closed field identities of an exported, concrete service collection.
/// The order is the sealed struct layout order, not source spelling order.
pub fn collection_fields(mir: &Mir, export: SymbolId) -> Result<Vec<(String, TypeId)>, String> {
    let ResolveState::Bound(symbol) = mir.symbols[export.index()].resolution else {
        return Err("MainService export is not resolved".into());
    };
    let TypeState::Known(meta) = mir.ty_slots[mir.symbol_types[symbol.index()].index()] else {
        return Err("MainService type is not closed".into());
    };
    let wrapper = &mir.types[meta.index()];
    if wrapper.constructor != TypeConstructor::Meta || wrapper.arguments.len() != 1 {
        return Err("MainService must export a concrete type".into());
    }
    let ty = wrapper.arguments[0];
    let TypeConstructor::Nominal(definition) = mir.types[ty.index()].constructor else {
        return Err("service collection must be a nominal struct".into());
    };
    let declared = mir
        .type_definitions
        .iter()
        .find(|declared| declared.symbol == definition)
        .ok_or("service collection has no type definition")?;
    if declared.operation != TypeOperation::Struct {
        return Err("service collection must be a struct".into());
    }
    let layout = mir.type_layouts[ty.index()]
        .as_ref()
        .ok_or("service collection has no closed field layout")?;
    if layout.members.len() != declared.members.len() {
        return Err("service collection field layout does not match its declaration".into());
    }
    declared
        .members
        .iter()
        .zip(&layout.members)
        .map(|(member, &ty)| {
            Ok((
                member.name.clone(),
                ty.ok_or("service collection field has no concrete type")?,
            ))
        })
        .collect()
}

/// MainService is a closed collection type with one static service plan.
pub fn transform_adapter(module: &str) -> Result<String, String> {
    if module.is_empty() {
        return Err("service module name is empty".into());
    }
    Ok(format!(
        r#"
        mod application;
        use self::application::{{ MainService }};
        use std::_entry::collection as entry;
        pub def main: entry::Plan = entry::prepare(MainService.type);
    "#
    ))
}
