//! Isolated storage experiment for RFC 0281. Never used by the existing VM.
//! All words encode the candidate little-endian ABI; no pointer casts/unsafe.
use crate::{
    candidate_layout::{self, Extent, State, Storage},
    mir::{SealedMir, TypeConstructor as T, TypeId},
};
use std::sync::atomic::{AtomicU64, Ordering};

pub type Location = [u32; 3];
type Result<T> = std::result::Result<T, String>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Scalar,
    String,
    Array,
    Tuple,
    Record,
    Dict,
    Other,
}
struct Layout {
    kind: Kind,
    words: usize,
    arguments: Vec<TypeId>,
    fields: Vec<(TypeId, usize)>,
}

/// Owning a descriptor never owns or deep-copies its referenced objects.
/// Arena identity is access context, not part of the encoded ABI header.
#[derive(Clone, Debug)]
pub struct Value {
    arena: u64,
    words: Vec<u64>,
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
            words: self.words.to_vec(),
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
        let id = u32::try_from(self.entries.len()).map_err(|_| "HeapId overflow")?;
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
struct RawStringTable {
    entries: Vec<RawStringItem>,
}
impl RawStringTable {
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

/// One isolated arena. Values from another arena are rejected; world publication
/// and cross-arena copying are intentionally not part of this experiment.
pub struct Arena {
    identity: u64,
    layouts: Vec<Option<Layout>>,
    strings: RawStringTable,
    tuples: WordTable,
    records: WordTable,
    arrays: WordTable,
    dicts: WordTable,
}
static NEXT_ARENA: AtomicU64 = AtomicU64::new(1);
impl Arena {
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
                T::String => Kind::String,
                T::Array => Kind::Array,
                T::Dict => Kind::Dict,
                T::Tuple => Kind::Tuple,
                _ if shape.table == Some("RecordTable") => Kind::Record,
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
                words: usize::try_from(shape.value_bytes / 8).map_err(|_| "value size overflow")?,
                arguments: ty.arguments.clone(),
                fields,
            }));
        }
        let identity = NEXT_ARENA
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| "arena identity overflow")?;
        Ok(Self {
            identity,
            layouts,
            strings: RawStringTable::default(),
            tuples: WordTable::default(),
            records: WordTable::default(),
            arrays: WordTable::default(),
            dicts: WordTable::default(),
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
            return Err("value/type layout mismatch".into());
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
            words,
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
            let id = self.strings.push(text.as_bytes())?;
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
                    .strings
                    .get(id)?
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
        if !matches!(layout.kind, Kind::Tuple | Kind::Record) {
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
        let id = if kind == Kind::Tuple {
            self.tuples.push(words)?
        } else {
            self.records.push(words)?
        };
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
            Kind::Tuple => self.tuples.get(value.words[2] as u32)?,
            Kind::Record => self.records.get(value.words[2] as u32)?,
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
        let id = self.arrays.push(words)?;
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
        let backing = self.arrays.get(id)?;
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
            .arrays
            .get(id)?
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

#[path = "layout_runtime/dict.rs"]
mod dict;
#[cfg(test)]
#[path = "layout_runtime/tests.rs"]
mod tests;
