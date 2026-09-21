//! Persistent metadata contains identities and positions, never source text.
use serde::{Deserialize, Serialize};
use telora_core::mir::{SealedExecutable, TypeConstructor as T, TypeState};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub abi: u32,
    pub entry_type: u32,
    pub types: Vec<TypeDesc>,
    pub sources: Vec<Source>,
    pub value_type: Option<u32>,
    pub data_modules: Vec<DataModule>,
    pub debug_sites: Vec<DebugSite>,
    pub globals: Vec<Global>,
    pub initialization_roots: Vec<InitializationRoot>,
}

/// The statically selected demand whose execution emitted an event.
/// Node covers property/anonymous demands; symbol identifies named globals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitializationRoot {
    pub node: u32,
    pub module: String,
    pub symbol: Option<u32>,
    pub name: Option<String>,
}

/// Closed global identity and its fixed initialization cell in linear memory.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Global {
    pub symbol: u32,
    pub name: String,
    pub ty: u32,
    pub demand: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DebugSite {
    pub node: u32,
    pub ty: u32,
    pub origin: [u32; 3],
    pub name: String,
    pub message: Option<String>,
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
    pub resource_table: Option<u32>,
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
    Dyn,
    Unsupported,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Source {
    pub id: u32,
    pub name: String,
    #[serde(skip)]
    pub lines: Vec<[u32; 2]>,
}

impl Source {
    pub(crate) fn from_file(file: &telora_core::source::SourceFile) -> Self {
        Self {
            id: file.id().get(),
            name: file.name.to_string(),
            lines: file.line_index().ranges().collect(),
        }
    }

    /// One-based line and UTF-8 byte column for diagnostic display.
    pub fn position(&self, point: u64) -> (usize, usize) {
        let (line, column) = telora_core::source::SourceCoordinates::position(point);
        (line as usize + 1, column as usize + 1)
    }
}

impl Manifest {
    pub(crate) fn build(
        executable: &SealedExecutable<'_>,
        layouts: &[telora_core::candidate_layout::Entry],
    ) -> Result<Self, String> {
        let mir = executable.sealed_mir().mir();
        let executable_nodes = executable
            .closure()
            .nodes()
            .iter()
            .map(|node| node.node.index())
            .collect::<std::collections::BTreeSet<_>>();
        let value_type = exported_type(mir, 23, "Value");
        let TypeState::Known(entry) = mir.ty_slots[executable.root().index()] else {
            return Err("Wasm: entry has no sealed type".into());
        };
        let sources = mir.sources.files().map(Source::from_file).collect();
        let types = mir
            .types
            .iter()
            .enumerate()
            .map(|(index, ty)| TypeDesc {
                resource_table: match ty.constructor {
                    T::Native(id) => match (id.module, id.slot) {
                        (19, 0) => Some(crate::abi::REGEXES),
                        (20, 1) => Some(crate::abi::FORMATS),
                        (16, 3) => Some(crate::abi::HASHES),
                        (33, 0) => Some(crate::abi::TESTS),
                        (34, 0) => Some(crate::abi::BLAMES),
                        _ => None,
                    },
                    _ => None,
                },
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
                        T::Dyn => Kind::Dyn,
                        T::Tuple => Kind::Tuple,
                        T::Record(_) => Kind::Record,
                        // Unchecked only admits named structs. It keeps its own
                        // TypeId but uses the owner's closed record layout.
                        T::Unchecked => Kind::Record,
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
            globals: vec![],
            initialization_roots: vec![],
            abi: crate::abi::VERSION,
            entry_type: entry.index() as u32,
            types,
            sources,
            value_type,
            debug_sites: mir
                .hir
                .iter()
                .enumerate()
                .filter_map(|(index, node)| {
                    if !executable_nodes.contains(&index) {
                        return None;
                    }
                    let telora_core::mir::HirKind::Debug {
                        message,
                        expression,
                    } = &node.kind
                    else {
                        return None;
                    };
                    Some(DebugSite {
                        node: index as u32,
                        ty: match mir.ty_slots[index] {
                            TypeState::Known(ty) => ty.index() as u32,
                            _ => return None,
                        },
                        origin: [
                            node.location.source.get(),
                            node.location.start,
                            node.location.end,
                        ],
                        name: expression.replace("\r\n", "\n").replace('\r', "\n"),
                        message: message.clone(),
                    })
                })
                .collect(),
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
                    telora_data::json_serde::from_slice::<Self>(section.data())
                        .map_err(|e| e.to_string())?,
                );
            }
        }
        let manifest = manifest.ok_or("Wasm: missing manifest")?;
        if manifest.abi != crate::abi::VERSION {
            return Err("Wasm: unsupported artifact ABI version".into());
        }
        if manifest
            .sources
            .iter()
            .any(|source| source.id == 0 || source.id > u16::MAX as u32)
        {
            return Err("Wasm: invalid source position index".into());
        }
        if manifest
            .data_modules
            .iter()
            .any(|m| Some(m.ty) != manifest.value_type)
            || manifest
                .value_type
                .into_iter()
                .any(|ty| ty as usize >= manifest.types.len())
            || manifest.entry_type as usize >= manifest.types.len()
            || manifest
                .globals
                .iter()
                .any(|global| global.ty as usize >= manifest.types.len() || global.demand % 4 != 0)
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
pub(crate) fn exported_type(
    mir: &telora_core::mir::Mir,
    module_id: u32,
    name: &str,
) -> Option<u32> {
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
