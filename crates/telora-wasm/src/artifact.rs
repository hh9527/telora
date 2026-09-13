//! Persistent metadata contains identities and positions, never source text.
use serde::{Deserialize, Serialize};
use telora_core::mir::{SealedExecutable, TypeConstructor as T, TypeState};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub abi: u32,
    pub entry_type: u32,
    pub types: Vec<TypeDesc>,
    pub sources: Vec<Source>,
    pub locations: Vec<Location>,
    pub value_type: Option<u32>,
    pub eval_type: Option<u32>,
    pub data_modules: Vec<DataModule>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DataModule {
    pub symbol: u32,
    pub name: String,
    pub ty: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TypeDesc {
    pub kind: Kind,
    pub arguments: Vec<u32>,
    pub bytes: u32,
    pub fields: Vec<Field>,
    pub variants: Vec<Variant>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Variant {
    pub name: String,
    pub ty: Option<u32>,
    pub boxed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub ty: u32,
    pub offset: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Int,
    Float,
    Bool,
    Unit,
    Function,
    String,
    Bytes,
    Value,
    Array,
    Tuple,
    Record,
    Dict,
    Enum,
    Option,
    Newtype,
    Metadata,
    Unsupported,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Source {
    pub id: u32,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Location {
    pub source: u32,
    pub start: u32,
    pub end: u32,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

impl Manifest {
    pub(crate) fn build(
        executable: &SealedExecutable<'_>,
        layouts: &[telora_core::candidate_layout::Entry],
    ) -> Result<Self, String> {
        let mir = executable.sealed_mir().mir();
        let value_type = exported_type(mir, 23, "Value");
        let eval_type = exported_type(mir, 32, "Eval");
        let TypeState::Known(entry) = mir.ty_slots[executable.root().index()] else {
            return Err("Wasm: entry has no sealed type".into());
        };
        let sources = mir
            .sources
            .files()
            .map(|file| Source {
                id: file.id().get(),
                name: file.name.to_string(),
            })
            .collect();
        let mut positions = std::collections::BTreeSet::new();
        let mut locations = vec![];
        for node in &mir.hir {
            let loc = node.location;
            if !positions.insert((loc.source.get(), loc.start, loc.end)) {
                continue;
            }
            let file = mir.sources.get(loc.source);
            let start = file.position(loc.start);
            let end = file.position(loc.end);
            locations.push(Location {
                source: loc.source.get(),
                start: loc.start,
                end: loc.end,
                line: start.line,
                column: start.column,
                end_line: end.line,
                end_column: end.column,
            });
        }
        locations.sort_by_key(|loc| (loc.source, loc.start, loc.end));
        let types = mir
            .types
            .iter()
            .enumerate()
            .map(|(index, ty)| TypeDesc {
                kind: if value_type == Some(index as u32) {
                    Kind::Value
                } else {
                    match ty.constructor {
                        T::Int => Kind::Int,
                        T::Float => Kind::Float,
                        T::Bool => Kind::Bool,
                        T::Tuple if ty.arguments.is_empty() => Kind::Unit,
                        T::Function => Kind::Function,
                        T::String => Kind::String,
                        T::Bytes => Kind::Bytes,
                        T::Array => Kind::Array,
                        T::Dict => Kind::Dict,
                        T::Option => Kind::Option,
                        T::Result | T::FoldControl | T::PropertyTarget | T::Enum(_) => Kind::Enum,
                        T::Newtype => Kind::Newtype,
                        T::Type | T::TypeOf => Kind::Metadata,
                        T::Tuple => Kind::Tuple,
                        T::Record(_) => Kind::Record,
                        T::Nominal(symbol) => match executable
                            .sealed_mir()
                            .types()
                            .definition(symbol)
                            .map(|d| d.operation)
                        {
                            Some(telora_core::mir::TypeOperation::Struct) => Kind::Record,
                            Some(telora_core::mir::TypeOperation::Tuple) => Kind::Tuple,
                            Some(telora_core::mir::TypeOperation::Unit) => Kind::Unit,
                            Some(telora_core::mir::TypeOperation::Enum) => Kind::Enum,
                            Some(telora_core::mir::TypeOperation::Newtype) => Kind::Newtype,
                            _ => Kind::Unsupported,
                        },
                        _ => Kind::Unsupported,
                    }
                },
                arguments: ty.arguments.iter().map(|id| id.index() as u32).collect(),
                bytes: match &layouts[index].layout {
                    telora_core::candidate_layout::State::Known { shape } => {
                        shape.value_bytes as u32
                    }
                    _ => 0,
                },
                fields: layouts[index]
                    .object
                    .iter()
                    .flat_map(|object| &object.members)
                    .filter_map(|member| {
                        Some(Field {
                            name: member.name.clone(),
                            ty: member.type_id? as u32,
                            offset: member.offset? as u32,
                        })
                    })
                    .collect(),
                variants: layouts[index]
                    .variants
                    .iter()
                    .map(|v| Variant {
                        name: v.name.clone(),
                        ty: v.type_id.map(|t| t as u32),
                        boxed: v.storage == "heap_id",
                    })
                    .collect(),
            })
            .collect();
        Ok(Self {
            abi: crate::abi::VERSION,
            entry_type: entry.index() as u32,
            types,
            sources,
            locations,
            value_type,
            eval_type,
            data_modules: executable
                .globals()
                .iter()
                .filter_map(|symbol| {
                    let module = &mir.modules[mir.symbols[symbol.index()].module?.index()];
                    if module.kind != telora_core::mir::ModuleKind::Data {
                        return None;
                    }
                    let TypeState::Known(ty) =
                        mir.ty_slots[mir.symbol_types[symbol.index()].index()]
                    else {
                        return None;
                    };
                    Some(DataModule {
                        symbol: symbol.index() as u32,
                        name: module.name.clone(),
                        ty: ty.index() as u32,
                    })
                })
                .collect(),
        })
    }

    pub fn read(bytes: &[u8]) -> Result<Self, String> {
        let mut manifest = None;
        for payload in wasmparser::Parser::new(0).parse_all(bytes) {
            if let wasmparser::Payload::CustomSection(section) =
                payload.map_err(|e| e.to_string())?
                && section.name() == "telora.manifest"
            {
                if manifest.is_some() {
                    return Err("Wasm: duplicate manifest".into());
                }
                manifest = Some(
                    serde_json::from_slice::<Self>(section.data()).map_err(|e| e.to_string())?,
                );
            }
        }
        let manifest = manifest.ok_or("Wasm: missing manifest")?;
        if manifest.abi != crate::abi::VERSION {
            return Err("Wasm: unsupported artifact ABI version".into());
        }
        if manifest
            .data_modules
            .iter()
            .any(|m| Some(m.ty) != manifest.value_type)
            || manifest
                .value_type
                .into_iter()
                .chain(manifest.eval_type)
                .any(|ty| ty as usize >= manifest.types.len())
            || manifest.entry_type as usize >= manifest.types.len()
            || manifest.types.iter().any(|ty| {
                ty.arguments
                    .iter()
                    .any(|&arg| arg as usize >= manifest.types.len())
                    || ty
                        .fields
                        .iter()
                        .any(|field| field.ty as usize >= manifest.types.len())
                    || ty.variants.iter().any(|variant| {
                        variant
                            .ty
                            .is_some_and(|ty| ty as usize >= manifest.types.len())
                    })
            })
        {
            return Err("Wasm: invalid manifest TypeId".into());
        }
        Ok(manifest)
    }
}

/// Contracts are resolved from the admitted module's exports, never user type names.
fn exported_type(mir: &telora_core::mir::Mir, module_id: u32, name: &str) -> Option<u32> {
    let module = mir.modules.iter().position(|module| {
        module
            .native
            .as_ref()
            .is_some_and(|native| native.id == module_id)
    })?;
    let symbol = mir.exports[module]
        .iter()
        .find(|id| mir.symbols[id.index()].name == name)?;
    let TypeState::Known(meta) = mir.ty_slots[mir.symbol_types[symbol.index()].index()] else {
        return None;
    };
    let shape = &mir.types[meta.index()];
    (shape.constructor == T::Meta && shape.arguments.len() == 1)
        .then(|| shape.arguments[0].index() as u32)
}
