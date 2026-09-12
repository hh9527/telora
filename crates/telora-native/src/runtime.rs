//! Independent native object runtime, using the sealed ABI and two worlds.
//! All words encode the candidate little-endian ABI; no pointer casts/unsafe.
use crate::abi::{HeapRef, TypeKey as TypeId, Value, World};
use std::sync::atomic::{AtomicU64, Ordering};
use telora_core::{
    candidate_layout::{self, State},
    mir::{SealedMir, TypeConstructor as T},
};

pub type Location = [u32; 3];
type Result<T> = std::result::Result<T, String>;
mod debug;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Scalar,
    Metadata,
    String,
    Bytes,
    Array,
    Tuple,
    Newtype,
    Record,
    Dict,
    Enum,
    Function,
    Dyn,
    Format,
    Regex,
    Hash,
    Blame,
    Test,
    Other,
}
struct Variant {
    name: String,
    payload: Option<TypeId>,
    storage: &'static str,
}
struct Layout {
    kind: Kind,
    nominal: bool,
    unchecked: Option<TypeId>,
    result: bool,
    construction_checks: Vec<u64>,
    optional: bool,
    dynamic_kind: Option<&'static str>,
    field_names: Vec<String>,
    words: usize,
    arguments: Vec<TypeId>,
    fields: Vec<(TypeId, usize)>,
    variants: Vec<Variant>,
}

#[derive(Clone, Copy)]
pub struct ValueRef<'a> {
    arena: u64,
    words: &'a [u64],
}
impl Value {
    pub fn as_ref(&self) -> ValueRef<'_> {
        ValueRef {
            arena: self.arena,
            words: &self.words,
        }
    }
    pub fn location(&self) -> Location {
        self.as_ref().location()
    }
    pub fn type_id(&self) -> TypeId {
        self.as_ref().type_id()
    }
}
impl<'a> ValueRef<'a> {
    pub fn location(self) -> Location {
        [
            self.words[0] as u32,
            (self.words[0] >> 32) as u32,
            self.words[1] as u32,
        ]
    }
    pub fn type_id(self) -> TypeId {
        TypeId((self.words[1] >> 32) as u32)
    }
    pub fn words(self) -> &'a [u64] {
        self.words
    }
    pub fn to_owned(self) -> Value {
        Value {
            arena: self.arena,
            words: self.words.to_vec().into_boxed_slice(),
        }
    }
}
/// Fixed-width slot metadata owns this object's allocation. HeapId identifies
/// the slot, independently of the buffer's address or the table's capacity.
struct WordItem {
    words: Vec<u64>,
}
#[derive(Default)]
struct WordTable {
    entries: Vec<WordItem>,
}
impl WordTable {
    fn push(&mut self, words: Vec<u64>) -> Result<u32> {
        let id = HeapRef::new(
            World::Work,
            u32::try_from(self.entries.len()).map_err(|_| "HeapId overflow")?,
        )?
        .slot();
        self.entries.push(WordItem { words });
        Ok(id)
    }
    fn get(&self, id: u32) -> Result<&[u64]> {
        let item = self
            .entries
            .get(id as usize)
            .ok_or("invalid table HeapId")?;
        Ok(&item.words)
    }
}
struct RawStringItem {
    bytes: Vec<u8>,
}
#[derive(Default)]
struct RawByteTable {
    entries: Vec<RawStringItem>,
}
impl RawByteTable {
    fn push(&mut self, bytes: &[u8]) -> Result<u32> {
        let id = u32::try_from(self.entries.len()).map_err(|_| "HeapId overflow")?;
        self.entries.push(RawStringItem {
            bytes: bytes.to_vec(),
        });
        Ok(id)
    }
    fn get(&self, id: u32) -> Result<&[u8]> {
        let item = self
            .entries
            .get(id as usize)
            .ok_or("invalid StringTable HeapId")?;
        Ok(&item.bytes)
    }
}
pub enum Text<'a> {
    Inline { bytes: [u8; 14], len: usize },
    Heap(&'a str),
}
impl Text<'_> {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Inline { bytes, len } => {
                std::str::from_utf8(&bytes[..*len]).expect("validated inline UTF-8")
            }
            Self::Heap(s) => s,
        }
    }
}

/// One session owns immutable main data and mutable work allocations.
#[derive(Default)]
struct Tables {
    strings: RawByteTable,
    bytes: RawByteTable,
    records: WordTable,
    newtypes: WordTable,
    arrays: WordTable,
    values: WordTable,
    environments: WordTable,
    formats: WordTable,
    regexes: Vec<pattern::CompiledRegex>,
    hashes: Vec<sha256::Context>,
    blames: Vec<blame::Blame>,
    tests: Vec<test_description::TestDescription>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Table {
    Records,
    Newtypes,
    Arrays,
    Values,
    Environments,
    Formats,
}

pub struct Runtime {
    allocation: allocation::Allocation,
    source_names: std::collections::BTreeMap<u32, String>,
    data_contract: Option<DataContract>,
    type_info: Vec<reflection::TypeInfo>,
    property_presence: std::collections::BTreeSet<(TypeId, TypeId)>,
    demands: std::collections::BTreeMap<DemandKey, demands::DemandSlot>,
    demand_keys: Vec<DemandKey>,
    code_plan: Option<u64>,
    identity: u64,
    layouts: Vec<Option<Layout>>,
    main: Tables,
    work: Tables,
    published: bool,
}
static NEXT_ARENA: AtomicU64 = AtomicU64::new(1);
impl Runtime {
    pub fn is_published(&self) -> bool {
        self.published
    }
    pub(crate) fn bind_code_plan(
        &mut self,
        identity: u64,
        demands: &[(DemandKey, TypeId)],
    ) -> Result<()> {
        match self.code_plan {
            Some(previous) if previous != identity => {
                Err("native runtime belongs to another code plan".into())
            }
            Some(_) => Ok(()),
            None => {
                for &(key, ty) in demands {
                    self.layout(ty)?;
                    if self.demands.contains_key(&key) {
                        return Err("native code plan demand already registered".into());
                    }
                }
                for &(key, ty) in demands {
                    self.register_demand(key, ty)?;
                }
                self.demand_keys = demands.iter().map(|&(key, _)| key).collect();
                self.code_plan = Some(identity);
                Ok(())
            }
        }
    }
    pub(crate) fn check_argument(&self, value: &Value) -> Result<()> {
        if value.arena == 0 {
            // Host-created scalar and Unit values carry no heap references.
            let layout = self.layout(value.type_key())?;
            if layout.kind == Kind::Scalar
                || (layout.kind == Kind::Tuple && layout.fields.is_empty())
            {
                if value.words.len() == layout.words {
                    return Ok(());
                }
            }
        }
        self.validate(value.as_ref(), value.type_key())
    }
    pub fn identity(&self) -> u64 {
        self.identity
    }
    fn tables(&self, reference: HeapRef) -> &Tables {
        match reference.world() {
            World::Main => &self.main,
            World::Work => &self.work,
        }
    }
    fn string_bytes(&self, raw: u32) -> Result<&[u8]> {
        let reference = HeapRef::from_raw(raw);
        self.tables(reference).strings.get(reference.slot())
    }
    fn object_words(&self, table: Table, raw: u32) -> Result<&[u64]> {
        let reference = HeapRef::from_raw(raw);
        let tables = self.tables(reference);
        match table {
            Table::Records => tables.records.get(reference.slot()),
            Table::Newtypes => tables.newtypes.get(reference.slot()),
            Table::Arrays => tables.arrays.get(reference.slot()),
            Table::Values => tables.values.get(reference.slot()),
            Table::Environments => tables.environments.get(reference.slot()),
            Table::Formats => tables.formats.get(reference.slot()),
        }
    }
    fn push_words(&mut self, table: Table, words: Vec<u64>) -> Result<u32> {
        self.charge_allocation(words.len(), 8, std::mem::size_of::<WordItem>())?;
        self.push_precharged_words(table, words)
    }
    /// The caller must charge payload words and the WordItem before insertion.
    fn push_precharged_words(&mut self, table: Table, words: Vec<u64>) -> Result<u32> {
        let slot = match table {
            Table::Records => self.work.records.push(words)?,
            Table::Newtypes => self.work.newtypes.push(words)?,
            Table::Arrays => self.work.arrays.push(words)?,
            Table::Values => self.work.values.push(words)?,
            Table::Environments => self.work.environments.push(words)?,
            Table::Formats => self.work.formats.push(words)?,
        };
        Ok(HeapRef::new(World::Work, slot)?.raw())
    }
    pub fn new(sealed: &SealedMir<'_>) -> Result<Self> {
        let entries = candidate_layout::calculate(sealed)?;
        let mut layouts = Vec::with_capacity(entries.len());
        for entry in entries {
            let State::Known { shape } = entry.layout else {
                layouts.push(None);
                continue;
            };
            let ty = &sealed.types().types[entry.type_id];
            let kind = match &ty.constructor {
                T::Int | T::Float | T::Bool => Kind::Scalar,
                T::Type | T::TypeOf => Kind::Metadata,
                T::String => Kind::String,
                T::Bytes => Kind::Bytes,
                T::Array => Kind::Array,
                T::Dict => Kind::Dict,
                T::Tuple => Kind::Tuple,
                T::Function => Kind::Function,
                T::Dyn => Kind::Dyn,
                T::Native(native) if (native.module, native.slot) == (20, 1) => Kind::Format,
                T::Native(native) if (native.module, native.slot) == (19, 0) => Kind::Regex,
                T::Native(native) if (native.module, native.slot) == (16, 3) => Kind::Hash,
                T::Native(native) if (native.module, native.slot) == (33, 0) => Kind::Test,
                T::Native(native) if (native.module, native.slot) == (34, 0) => Kind::Blame,
                _ if !entry.variants.is_empty() => Kind::Enum,
                _ if shape.table == Some("RecordTable") => Kind::Record,
                _ if shape.table == Some("NewtypeTable") => Kind::Newtype,
                _ => Kind::Other,
            };
            let fields = entry
                .object
                .as_ref()
                .map(|o| {
                    o.members
                        .iter()
                        .map(|f| {
                            Ok((
                                TypeId(
                                    u32::try_from(f.type_id.ok_or("field has no type")?)
                                        .map_err(|_| "TypeId overflow")?,
                                ),
                                usize::try_from(f.offset.ok_or("field has no offset")? / 8)
                                    .map_err(|_| "offset overflow")?,
                            ))
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()?
                .unwrap_or_default();
            layouts.push(Some(Layout {
                kind,
                nominal: matches!(ty.constructor, T::Nominal(_)),
                unchecked: if ty.constructor == T::Unchecked { Some(TypeId::try_from(ty.arguments[0])?) } else { None },
                result: ty.constructor == T::Result,
                construction_checks: sealed.mir().construction_checks.iter()
                    .filter(|check| check.concrete && check.owner.index() == entry.type_id)
                    .map(|check| match check.site { telora_core::mir::PropertySite::Type => 0, telora_core::mir::PropertySite::Variant(index) => u64::from(index) + 1, _ => unreachable!("sealed checker site") }).collect(),
                optional: ty.constructor == T::Option,
                field_names: entry
                    .object
                    .as_ref()
                    .map(|o| o.members.iter().map(|m| m.name.clone()).collect())
                    .unwrap_or_default(),
                dynamic_kind: match &ty.constructor {
                    T::Int => Some("Int"),
                    T::Float => Some("Float"),
                    T::String => Some("String"),
                    T::Bytes => Some("Bytes"),
                    T::Type | T::TypeOf => Some("Type"),
                    T::Native(_) => Some("Opaque"),
                    T::Record(_) | T::Dict => Some("Dict"),
                    T::Array => Some("Array"),
                    T::Tuple => Some("Tuple"),
                    T::Function => Some("Func"),
                    T::Dyn => Some("Dyn"),
                    T::Bool | T::PropertyTarget => Some("Atom"),
                    T::Nominal(symbol) => {
                        match sealed.types().definition(*symbol).map(|d| d.operation) {
                            Some(telora_core::mir::TypeOperation::Struct) => Some("Dict"),
                            Some(telora_core::mir::TypeOperation::Newtype) => Some("Tuple"),
                            _ => None,
                        }
                    }
                    _ => None,
                },
                words: usize::try_from(shape.value_bytes / 8).map_err(|_| "value size overflow")?,
                arguments: ty
                    .arguments
                    .iter()
                    .copied()
                    .map(TypeId::try_from)
                    .collect::<Result<Vec<_>>>()?,
                fields,
                variants: entry
                    .variants
                    .into_iter()
                    .map(|v| {
                        Ok(Variant {
                            name: v.name,
                            payload: v
                                .type_id
                                .map(|i| {
                                    u32::try_from(i)
                                        .map(TypeId)
                                        .map_err(|_| "variant TypeId overflow")
                                })
                                .transpose()?,
                            storage: v.storage,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
            }));
        }
        let identity = NEXT_ARENA
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| "arena identity overflow")?;
        Ok(Self {
            allocation: allocation::Allocation::default(),
            source_names: sealed.mir().sources.files().map(|file| (file.id().get(), file.name.to_string())).collect(),
            data_contract: if sealed.mir().modules.iter().any(|m| m.native.as_ref().is_some_and(|n| n.id == 23) && matches!(m.state, telora_core::mir::ModuleState::Source { .. })) { Some(DataContract::from_mir(sealed)?) } else { None },
            type_info: reflection::build(sealed.types())?,
            property_presence: sealed
                .mir()
                .properties
                .iter()
                .filter(|p| p.concrete && p.site == telora_core::mir::PropertySite::Type)
                .map(|p| Ok((TypeId::try_from(p.owner)?, TypeId::try_from(p.property)?)))
                .collect::<Result<_>>()?,
            demands: std::collections::BTreeMap::new(),
            demand_keys: vec![],
            code_plan: None,
            identity,
            layouts,
            main: Tables::default(),
            work: Tables::default(),
            published: false,
        })
    }
    fn layout(&self, ty: TypeId) -> Result<&Layout> {
        self.layouts
            .get(ty.index())
            .and_then(Option::as_ref)
            .ok_or_else(|| "type has no materializable layout".into())
    }
    fn expect(&self, ty: TypeId, kind: Kind) -> Result<&Layout> {
        let layout = self.layout(ty)?;
        if layout.kind != kind {
            return Err("operation/type mismatch".into());
        }
        Ok(layout)
    }
    fn validate(&self, value: ValueRef<'_>, ty: TypeId) -> Result<()> {
        if value.arena != self.identity {
            return Err("value belongs to a different arena".into());
        }
        if value.type_id() != ty || value.words.len() != self.layout(ty)?.words {
            return Err(format!(
                "value/type layout mismatch: actual type {} ({} words), expected type {} ({} words)",
                value.type_id().raw(),
                value.words.len(),
                ty.raw(),
                self.layout(ty)?.words
            ));
        }
        Ok(())
    }
    fn pack(&self, ty: TypeId, loc: Location, data: &[u64]) -> Result<Value> {
        let expected = self.layout(ty)?.words;
        if expected != data.len() + 2 {
            return Err("incorrect data width".into());
        }
        let mut words = Vec::with_capacity(expected);
        words.push(u64::from(loc[0]) | (u64::from(loc[1]) << 32));
        words.push(u64::from(loc[2]) | ((ty.index() as u64) << 32));
        words.extend_from_slice(data);
        Ok(Value {
            arena: self.identity,
            words: words.into_boxed_slice(),
        })
    }
    pub fn scalar(&self, ty: TypeId, loc: Location, bits: u64) -> Result<Value> {
        self.expect(ty, Kind::Scalar)?;
        self.pack(ty, loc, &[bits])
    }
    pub fn scalar_bits(&self, value: ValueRef<'_>) -> Result<u64> {
        self.validate(value, value.type_id())?;
        self.expect(value.type_id(), Kind::Scalar)?;
        Ok(value.words[2])
    }
    pub fn string(&mut self, ty: TypeId, loc: Location, text: &str) -> Result<Value> {
        self.expect(ty, Kind::String)?;
        let len = u32::try_from(text.len()).map_err(|_| "string length overflow")?;
        let mut bytes = [0u8; 16];
        if text.len() <= 14 {
            bytes[1] = text.len() as u8;
            bytes[2..2 + text.len()].copy_from_slice(text.as_bytes());
        } else {
            bytes[0] = 1;
            self.charge_allocation(text.len(), 1, std::mem::size_of::<RawStringItem>())?;
            let id = HeapRef::new(World::Work, self.work.strings.push(text.as_bytes())?)?.raw();
            bytes[4..8].copy_from_slice(&id.to_le_bytes());
            bytes[12..16].copy_from_slice(&len.to_le_bytes());
        }
        self.pack(
            ty,
            loc,
            &[
                u64::from_le_bytes(bytes[..8].try_into().unwrap()),
                u64::from_le_bytes(bytes[8..].try_into().unwrap()),
            ],
        )
    }
    pub fn text<'a>(&'a self, value: ValueRef<'_>) -> Result<Text<'a>> {
        self.validate(value, value.type_id())?;
        self.expect(value.type_id(), Kind::String)?;
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&value.words[2].to_le_bytes());
        bytes[8..].copy_from_slice(&value.words[3].to_le_bytes());
        match bytes[0] {
            0 => {
                let len = bytes[1] as usize;
                if len > 14 {
                    return Err("invalid inline string length".into());
                }
                let mut data = [0; 14];
                data.copy_from_slice(&bytes[2..]);
                std::str::from_utf8(&data[..len]).map_err(|_| "invalid UTF-8")?;
                Ok(Text::Inline { bytes: data, len })
            }
            1 => {
                let id = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
                let start = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
                let end = u32::from_le_bytes(bytes[12..].try_into().unwrap()) as usize;
                let data = self
                    .string_bytes(id)?
                    .get(start..end)
                    .ok_or("invalid string slice")?;
                Ok(Text::Heap(
                    std::str::from_utf8(data).map_err(|_| "invalid UTF-8")?,
                ))
            }
            _ => Err("invalid string tag".into()),
        }
    }
    /// Tuple and Record share all fixed-field storage and access logic.
    pub fn aggregate(&mut self, ty: TypeId, loc: Location, values: &[Value]) -> Result<Value> {
        let layout = self.layout(ty)?;
        if !matches!(layout.kind, Kind::Tuple | Kind::Record | Kind::Newtype) {
            return Err("not a fixed-field aggregate".into());
        }
        if values.len() != layout.fields.len() {
            return Err("incorrect field count".into());
        }
        let kind = layout.kind;
        let mut words = vec![];
        for (value, (field, offset)) in values.iter().zip(&layout.fields) {
            self.validate(value.as_ref(), *field)?;
            if *offset != words.len() {
                return Err("non-contiguous candidate fields".into());
            }
            words.extend_from_slice(&value.words);
        }
        if kind == Kind::Tuple && values.is_empty() {
            return self.pack(ty, loc, &[]);
        }
        let id = self.push_words(if kind == Kind::Newtype { Table::Newtypes } else { Table::Records }, words)?;
        self.pack(ty, loc, &[u64::from(id)])
    }
    pub fn field<'a>(&'a self, value: &Value, index: usize) -> Result<ValueRef<'a>> {
        self.validate(value.as_ref(), value.type_id())?;
        let layout = self.layout(value.type_id())?;
        let (ty, offset) = *layout
            .fields
            .get(index)
            .ok_or("field index out of bounds")?;
        let words = match layout.kind {
            Kind::Tuple | Kind::Record => {
                self.object_words(Table::Records, value.words[2] as u32)?
            }
            Kind::Newtype => self.object_words(Table::Newtypes, value.words[2] as u32)?,
            _ => return Err("not an aggregate".into()),
        };
        let result = ValueRef {
            arena: self.identity,
            words: words
                .get(offset..offset + self.layout(ty)?.words)
                .ok_or("invalid field span")?,
        };
        self.validate(result, ty)?;
        Ok(result)
    }
    pub fn replace_field(
        &mut self,
        value: &Value,
        index: usize,
        replacement: &Value,
        loc: Location,
    ) -> Result<Value> {
        let len = self.layout(value.type_id())?.fields.len();
        if index >= len {
            return Err("field index out of bounds".into());
        }
        let mut fields = (0..len)
            .map(|i| self.field(value, i).map(ValueRef::to_owned))
            .collect::<Result<Vec<_>>>()?;
        fields[index] = replacement.clone();
        self.aggregate(value.type_id(), loc, &fields)
    }
    pub fn array(&mut self, ty: TypeId, loc: Location, values: &[Value]) -> Result<Value> {
        let element = self.expect(ty, Kind::Array)?.arguments[0];
        let len = u32::try_from(values.len()).map_err(|_| "array length overflow")?;
        let mut words = vec![];
        for value in values {
            self.validate(value.as_ref(), element)?;
            words.extend_from_slice(&value.words);
        }
        let id = self.push_words(Table::Arrays, words)?;
        self.pack(ty, loc, &[u64::from(id), u64::from(len)])
    }
    fn array_range(&self, value: &Value) -> Result<(u32, u32, u32, TypeId)> {
        self.validate(value.as_ref(), value.type_id())?;
        let element = self.expect(value.type_id(), Kind::Array)?.arguments[0];
        let id = value.words[2] as u32;
        let start = (value.words[2] >> 32) as u32;
        let end = value.words[3] as u32;
        if start > end {
            return Err("invalid slice range".into());
        }
        let backing = self.object_words(Table::Arrays, id)?;
        if backing.is_empty() && end != 0 {
            return Err("slice beyond empty storage".into());
        }
        if start != end || !backing.is_empty() {
            let stride = self.layout(element)?.words;
            if end as usize > backing.len() / stride {
                return Err("slice beyond backing storage".into());
            }
        }
        Ok((id, start, end, element))
    }
    pub fn array_len(&self, value: &Value) -> Result<usize> {
        let (_, start, end, _) = self.array_range(value)?;
        Ok((end - start) as usize)
    }
    pub fn slice(&self, value: &Value, start: u32, end: u32, loc: Location) -> Result<Value> {
        let (id, base, limit, _) = self.array_range(value)?;
        if start > end || end > limit - base {
            return Err("slice out of bounds".into());
        }
        self.pack(
            value.type_id(),
            loc,
            &[
                u64::from(id) | (u64::from(base + start) << 32),
                u64::from(base + end),
            ],
        )
    }
    pub fn array_get<'a>(&'a self, value: &Value, index: usize) -> Result<ValueRef<'a>> {
        let (id, start, end, element) = self.array_range(value)?;
        if index >= (end - start) as usize {
            return Err("array index out of bounds".into());
        }
        let stride = self.layout(element)?.words;
        let offset = (start as usize + index)
            .checked_mul(stride)
            .ok_or("array offset overflow")?;
        let words = self
            .object_words(Table::Arrays, id)?
            .get(offset..offset + stride)
            .ok_or("invalid element span")?;
        let result = ValueRef {
            arena: self.identity,
            words,
        };
        self.validate(result, element)?;
        Ok(result)
    }
    pub fn array_set(
        &mut self,
        value: &Value,
        index: usize,
        replacement: &Value,
        loc: Location,
    ) -> Result<Value> {
        let len = self.array_len(value)?;
        if index >= len {
            return Err("array index out of bounds".into());
        }
        let mut values = (0..len)
            .map(|i| self.array_get(value, i).map(ValueRef::to_owned))
            .collect::<Result<Vec<_>>>()?;
        values[index] = replacement.clone();
        self.array(value.type_id(), loc, &values)
    }
}

#[path = "runtime/bytes.rs"]
mod bytes;
#[path = "runtime/closures.rs"]
mod closures;
#[path = "runtime/data.rs"]
mod data;
mod codec;
mod cast;
mod codec_parse;
#[path = "runtime/demands.rs"]
mod demands;
#[path = "runtime/dynamic.rs"]
mod dynamic;
#[path = "runtime/format.rs"]
mod format;
#[path = "runtime/pattern.rs"]
mod pattern;
#[path = "runtime/text_ops.rs"]
mod text_ops;
mod reflection;
mod schema;
mod path;
mod hash;
mod sha256;
mod diagnostics;
mod equality;
mod test_description;
mod allocation;
mod array_ops;
mod blame;
pub use dynamic::DynamicQuery;
#[path = "runtime/metadata.rs"]
mod metadata;
pub use data::DataContract;
#[path = "runtime/json.rs"]
mod json;
pub use demands::{Demand, DemandKey};
#[path = "runtime/dict.rs"]
mod dict;
#[path = "runtime/enums.rs"]
mod enums;
#[cfg(feature = "jit")]
#[path = "runtime/helpers.rs"]
pub(crate) mod helpers;
#[path = "runtime/publish.rs"]
mod publish;
#[cfg(test)]
#[path = "runtime/tests.rs"]
mod tests;
