//! Build immutable layout/callback metadata; service control flow lives in Rust RT.
use crate::{
    abi::*,
    artifact::{Kind, Manifest},
};
use telora_wasm_shared::service::Contract;

pub(crate) fn contract(manifest: &Manifest, initialize: u32) -> Result<Contract, String> {
    let entry = &manifest.types[manifest.entry_type as usize];
    if entry.kind != Kind::Tuple || entry.fields.len() != 2 {
        return Err("Wasm: service entry requires the sealed Plan tuple".into());
    }
    let names = &entry.fields[0];
    let names_type = &manifest.types[names.ty as usize];
    if names_type.kind != Kind::Array
        || names_type.arguments.len() != 1
        || manifest.types[names_type.arguments[0] as usize].kind != Kind::String
    {
        return Err("Wasm: service Plan sources require Array(String)".into());
    }
    let initializer = &entry.fields[1];
    let signature = &manifest.types[initializer.ty as usize];
    if signature.kind != Kind::Function || signature.arguments.len() != 2 {
        return Err("Wasm: service initializer requires Fn(Context) -> Handler".into());
    }
    let ctx_ty = signature.arguments[0];
    let ctx = &manifest.types[ctx_ty as usize];
    if ctx.kind != Kind::Record || ctx.bytes != SCALAR_BYTES || ctx.fields.len() != 1 {
        return Err("Wasm: invalid sealed service Context".into());
    }
    let sources = &ctx.fields[0];
    let dict = &manifest.types[sources.ty as usize];
    if sources.name != "sources"
        || dict.kind != Kind::Dict
        || dict.bytes != STRING_BYTES
        || dict.arguments.len() != 1
        || Some(dict.arguments[0]) != manifest.value_type
    {
        return Err("Wasm: service Context requires sources: Dict(Value)".into());
    }
    let handler = &manifest.types[signature.arguments[1] as usize];
    if handler.kind != Kind::Function
        || handler.arguments.len() != 2
        || Some(handler.arguments[0]) != manifest.value_type
        || manifest.types[handler.arguments[1] as usize].kind != Kind::String
    {
        return Err("Wasm: service Handler requires Fn(Value) -> String".into());
    }
    Ok(Contract {
        initialize,
        entry: initialize + 1,
        materialize: initialize + 3,
        names_offset: names.offset,
        initializer_offset: initializer.offset,
        context_type: ctx_ty,
        dict_type: sources.ty,
        handler_type: signature.arguments[1],
        value_bytes: manifest.types[dict.arguments[0] as usize].bytes,
        sources_offset: sources.offset,
    })
}
