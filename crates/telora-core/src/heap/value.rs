use crate::bytecode::Constant;
use crate::source::Loc;
use crate::{BuiltinAtom, BytecodeFunction, FuncByteCode, NativeFunction};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

const INLINE_TEXT_BYTES: usize = 7;

const KIND_BITS: u32 = 6;
const SUB_KIND_BITS: u32 = 6;
const KIND_MASK: u32 = (1 << KIND_BITS) - 1;
const SUB_KIND_MASK: u32 = ((1 << SUB_KIND_BITS) - 1) << KIND_BITS;
const TRAIT_SHIFT: u32 = KIND_BITS + SUB_KIND_BITS;
const PROVENANCE_SHIFT: u32 = 28;
const PROVENANCE_MASK: u32 = 0b11 << PROVENANCE_SHIFT;

const TRAIT_REFERENCE: u16 = 1 << 0;
const TRAIT_TEXT: u16 = 1 << 1;
const TRAIT_INLINE: u16 = 1 << 2;
const TRAIT_HEAP: u16 = 1 << 3;
const TRAIT_TYPE_SLOT: u16 = 1 << 4;
const TRAIT_TRACE: u16 = 1 << 5;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum Storage {
    Work,
    Main,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum FlatKind {
    Never,
    Int,
    Float,
    InlineString,
    String,
    InlineAtom,
    Atom,
    NativeType,
    Heap,
    TypeSlot,
    FuncRef,
    Invalid = 63,
}

impl FlatKind {
    fn from_bits(bits: u32) -> Self {
        match bits {
            0 => Self::Never,
            1 => Self::Int,
            2 => Self::Float,
            3 => Self::InlineString,
            4 => Self::String,
            5 => Self::InlineAtom,
            6 => Self::Atom,
            7 => Self::NativeType,
            8 => Self::Heap,
            9 => Self::TypeSlot,
            10 => Self::FuncRef,
            _ => Self::Invalid,
        }
    }

    const fn traits(self) -> u16 {
        match self {
            Self::Never | Self::Int | Self::Float | Self::NativeType | Self::FuncRef => {
                TRAIT_INLINE
            }
            Self::InlineString | Self::InlineAtom => TRAIT_INLINE | TRAIT_TEXT,
            Self::String | Self::Atom => TRAIT_REFERENCE | TRAIT_TEXT,
            Self::Heap => TRAIT_REFERENCE | TRAIT_HEAP,
            Self::TypeSlot => TRAIT_REFERENCE | TRAIT_TYPE_SLOT | TRAIT_TRACE,
            Self::Invalid => 0,
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum HeapKind {
    None,
    Bytes,
    DeclaredType,
    Opaque,
    Array,
    Tuple,
    Tagged,
    Dict,
    Func,
    Dyn,
    Module,
    SymbolicType,
}

impl HeapKind {
    fn from_bits(bits: u32) -> Self {
        match bits {
            1 => Self::Bytes,
            2 => Self::DeclaredType,
            3 => Self::Opaque,
            4 => Self::Array,
            5 => Self::Tuple,
            6 => Self::Tagged,
            7 => Self::Dict,
            8 => Self::Func,
            9 => Self::Dyn,
            10 => Self::Module,
            11 => Self::SymbolicType,
            _ => Self::None,
        }
    }

    const fn traits(self) -> u16 {
        match self {
            Self::Array
            | Self::Tuple
            | Self::Tagged
            | Self::Dict
            | Self::Func
            | Self::DeclaredType
            | Self::SymbolicType
            | Self::Dyn
            | Self::Module => TRAIT_TRACE,
            Self::None | Self::Bytes | Self::Opaque => 0,
        }
    }
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Meta(u32);

impl Meta {
    fn new(kind: FlatKind, sub_kind: HeapKind, provenance: Provenance) -> Self {
        let traits = kind.traits() | sub_kind.traits();
        Self(
            kind as u32
                | ((sub_kind as u32) << KIND_BITS)
                | ((traits as u32) << TRAIT_SHIFT)
                | ((provenance as u32) << PROVENANCE_SHIFT),
        )
    }

    fn kind(self) -> FlatKind {
        FlatKind::from_bits(self.0 & KIND_MASK)
    }

    fn sub_kind(self) -> HeapKind {
        HeapKind::from_bits((self.0 & SUB_KIND_MASK) >> KIND_BITS)
    }

    fn traits(self) -> u16 {
        ((self.0 >> TRAIT_SHIFT) & u16::MAX as u32) as u16
    }

    fn provenance(self) -> Provenance {
        match (self.0 & PROVENANCE_MASK) >> PROVENANCE_SHIFT {
            1 => Provenance::Original,
            2 => Provenance::Generated,
            _ => Provenance::Unknown,
        }
    }

    fn with_provenance(self, provenance: Provenance) -> Self {
        Self((self.0 & !PROVENANCE_MASK) | ((provenance as u32) << PROVENANCE_SHIFT))
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct PackedLoc {
    source: u32,
    start: u32,
    end: u32,
}

impl PackedLoc {
    const UNKNOWN: Self = Self {
        source: 0,
        start: 0,
        end: 0,
    };

    fn new(loc: Option<Loc>) -> Self {
        loc.map_or(Self::UNKNOWN, |loc| Self {
            source: loc.source.get(),
            start: loc.start,
            end: loc.end,
        })
    }

    fn get(self) -> Option<Loc> {
        Some(Loc {
            source: crate::SourceId::from_raw(self.source)?,
            start: self.start,
            end: self.end,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Handle {
    storage: Storage,
    slot: u32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct InternId {
    storage: Storage,
    slot: u32,
}

const SCOPED_ID_WORK_BIT: u32 = 1 << 31;
const SCOPED_ID_SLOT_MASK: u32 = SCOPED_ID_WORK_BIT - 1;

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ScopedId(u32);

impl ScopedId {
    fn new(storage: Storage, slot: u32) -> Self {
        assert!(
            slot <= SCOPED_ID_SLOT_MASK,
            "arena slot exceeds scoped ID range"
        );
        Self(
            slot | if storage == Storage::Work {
                SCOPED_ID_WORK_BIT
            } else {
                0
            },
        )
    }

    fn storage(self) -> Storage {
        if self.0 & SCOPED_ID_WORK_BIT == 0 {
            Storage::Main
        } else {
            Storage::Work
        }
    }

    fn slot(self) -> u32 {
        self.0 & SCOPED_ID_SLOT_MASK
    }

    fn from_raw(raw: u64) -> Self {
        Self(u32::try_from(raw).expect("scoped reference exceeds 32 bits"))
    }

    fn raw(self) -> u64 {
        u64::from(self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct InlineText {
    bytes: [u8; 8],
}

impl InlineText {
    fn new(text: &str) -> Option<Self> {
        if text.len() > INLINE_TEXT_BYTES {
            return None;
        }
        let mut bytes = [0; 8];
        bytes[..text.len()].copy_from_slice(text.as_bytes());
        bytes[7] = text.len() as u8;
        Some(Self { bytes })
    }

    fn from_raw(raw: u64) -> Self {
        Self {
            bytes: raw.to_le_bytes(),
        }
    }

    fn raw(self) -> u64 {
        u64::from_le_bytes(self.bytes)
    }

    pub(crate) fn as_str(&self) -> &str {
        let len = self.bytes[7] as usize;
        debug_assert!(len <= INLINE_TEXT_BYTES);
        std::str::from_utf8(&self.bytes[..len]).expect("inline text must be valid UTF-8")
    }
}

#[derive(Clone, Copy, Debug)]
enum TextRefInner<'a> {
    Inline(InlineText),
    Borrowed(&'a str),
}

#[derive(Clone, Copy, Debug)]
pub struct TextRef<'a>(TextRefInner<'a>);

impl<'a> TextRef<'a> {
    pub(crate) fn inline(text: InlineText) -> Self {
        Self(TextRefInner::Inline(text))
    }

    pub(crate) fn borrowed(text: &'a str) -> Self {
        Self(TextRefInner::Borrowed(text))
    }

    pub fn as_str(&self) -> &str {
        match &self.0 {
            TextRefInner::Inline(text) => text.as_str(),
            TextRefInner::Borrowed(text) => text,
        }
    }
}

impl AsRef<str> for TextRef<'_> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::ops::Deref for TextRef<'_> {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl PartialEq for TextRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for TextRef<'_> {}

impl PartialEq<&str> for TextRef<'_> {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<String> for TextRef<'_> {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<TextRef<'_>> for String {
    fn eq(&self, other: &TextRef<'_>) -> bool {
        self == other.as_str()
    }
}

impl fmt::Display for TextRef<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<TextRef<'_>> for String {
    fn from(value: TextRef<'_>) -> Self {
        value.as_str().to_owned()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ShapeId {
    storage: Storage,
    slot: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DecodedValue {
    Failed(u32),
    Int(i64),
    Float(f64),
    BuiltinAtom(BuiltinAtom),
    InlineAtom(InlineText),
    Atom(InternId),
    InlineString(InlineText),
    ShortString(InternId),
    Bytes(Handle),
    NativeType(crate::value::NativeTypeId),
    DeclaredType(Handle),
    SymbolicType(Handle),
    Opaque(Handle),
    Array(Handle),
    Tuple(Handle),
    Tagged(Handle),
    Dict(Handle),
    Func(Handle),
    Dyn(Handle),
    Module(Handle),
    TypeSlot(Handle),
    FuncRef(crate::FuncId),
}

impl DecodedValue {
    fn encode(self) -> (Meta, u64) {
        let (kind, sub_kind, raw) = match self {
            Self::Failed(id) => (FlatKind::Never, HeapKind::None, ((id as u64) << 1) | 1),
            Self::Int(value) => (FlatKind::Int, HeapKind::None, value as u64),
            Self::Float(value) => (FlatKind::Float, HeapKind::None, value.to_bits()),
            Self::BuiltinAtom(atom) => (
                FlatKind::InlineAtom,
                HeapKind::None,
                InlineText::new(atom.name())
                    .expect("built-in Atoms fit inline")
                    .raw(),
            ),
            Self::InlineAtom(text) => (FlatKind::InlineAtom, HeapKind::None, text.raw()),
            Self::Atom(id) => (
                FlatKind::Atom,
                HeapKind::None,
                ScopedId::new(id.storage, id.slot).raw(),
            ),
            Self::InlineString(text) => (FlatKind::InlineString, HeapKind::None, text.raw()),
            Self::ShortString(id) => (
                FlatKind::String,
                HeapKind::None,
                ScopedId::new(id.storage, id.slot).raw(),
            ),
            Self::Bytes(handle) => heap_parts(handle, HeapKind::Bytes),
            Self::NativeType(id) => (
                FlatKind::NativeType,
                HeapKind::None,
                u64::from(id.module.0) | (u64::from(id.local) << 32),
            ),
            Self::DeclaredType(handle) => heap_parts(handle, HeapKind::DeclaredType),
            Self::SymbolicType(handle) => heap_parts(handle, HeapKind::SymbolicType),
            Self::Opaque(handle) => heap_parts(handle, HeapKind::Opaque),
            Self::Array(handle) => heap_parts(handle, HeapKind::Array),
            Self::Tuple(handle) => heap_parts(handle, HeapKind::Tuple),
            Self::Tagged(handle) => heap_parts(handle, HeapKind::Tagged),
            Self::Dict(handle) => heap_parts(handle, HeapKind::Dict),
            Self::Func(handle) => heap_parts(handle, HeapKind::Func),
            Self::Dyn(handle) => heap_parts(handle, HeapKind::Dyn),
            Self::Module(handle) => heap_parts(handle, HeapKind::Module),
            Self::TypeSlot(handle) => (
                FlatKind::TypeSlot,
                HeapKind::None,
                ScopedId::new(handle.storage, handle.slot).raw(),
            ),
            Self::FuncRef(id) => (
                FlatKind::FuncRef,
                HeapKind::None,
                u64::from(id.module.raw()) | (u64::from(id.local) << 32),
            ),
        };
        (Meta::new(kind, sub_kind, Provenance::Unknown), raw)
    }
}

fn heap_parts(handle: Handle, sub_kind: HeapKind) -> (FlatKind, HeapKind, u64) {
    (
        FlatKind::Heap,
        sub_kind,
        ScopedId::new(handle.storage, handle.slot).raw(),
    )
}

#[repr(C, align(8))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Val {
    loc: PackedLoc,
    meta: Meta,
    ty: u32,
    narrow: u32,
    raw: u64,
}

const _: [(); 32] = [(); std::mem::size_of::<Val>()];
const _: [(); 8] = [(); std::mem::align_of::<Val>()];

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Provenance {
    Unknown,
    Original,
    Generated,
}

impl Val {
    pub(crate) fn new(value: DecodedValue, loc: Option<Loc>) -> Self {
        let (meta, raw) = value.encode();
        Self {
            loc: PackedLoc::new(loc),
            meta: meta.with_provenance(if loc.is_some() {
                Provenance::Generated
            } else {
                Provenance::Unknown
            }),
            ty: 0,
            narrow: 0,
            raw,
        }
    }

    pub(crate) fn original(value: DecodedValue, loc: Option<Loc>) -> Self {
        let (meta, raw) = value.encode();
        Self {
            loc: PackedLoc::new(loc),
            meta: meta.with_provenance(if loc.is_some() {
                Provenance::Original
            } else {
                Provenance::Unknown
            }),
            ty: 0,
            narrow: 0,
            raw,
        }
    }

    pub(crate) fn unknown(value: DecodedValue) -> Self {
        Self::new(value, None)
    }

    pub(crate) fn with_loc(self, loc: Option<Loc>) -> Self {
        if self.loc() == loc {
            self
        } else {
            Self {
                loc: PackedLoc::new(loc),
                meta: self.meta.with_provenance(if loc.is_some() {
                    Provenance::Generated
                } else {
                    Provenance::Unknown
                }),
                ..self
            }
        }
    }

    pub(crate) fn rebase_generated(self, call_site: Option<Loc>) -> Self {
        match self.meta.provenance() {
            Provenance::Original => self,
            Provenance::Unknown | Provenance::Generated => self.with_loc(call_site),
        }
    }

    pub(crate) fn loc(self) -> Option<Loc> {
        match self.meta.provenance() {
            Provenance::Unknown => None,
            Provenance::Original | Provenance::Generated => self.loc.get(),
        }
    }

    pub(crate) fn value(self) -> DecodedValue {
        debug_assert_eq!(self.narrow, 0, "narrowing evidence is not implemented");
        debug_assert_eq!(
            self.meta.traits(),
            self.meta.kind().traits() | self.meta.sub_kind().traits(),
            "runtime Meta traits disagree with its exact classification"
        );
        let scoped_id = || ScopedId::from_raw(self.raw);
        let handle = || Handle {
            storage: scoped_id().storage(),
            slot: scoped_id().slot(),
        };
        match (self.meta.kind(), self.meta.sub_kind()) {
            (FlatKind::Never, _) => DecodedValue::Failed((self.raw >> 1) as u32),
            (FlatKind::Int, _) => DecodedValue::Int(self.raw as i64),
            (FlatKind::Float, _) => DecodedValue::Float(f64::from_bits(self.raw)),
            (FlatKind::InlineAtom, _) => {
                let text = InlineText::from_raw(self.raw);
                builtin_atom(text.as_str())
                    .map(DecodedValue::BuiltinAtom)
                    .unwrap_or(DecodedValue::InlineAtom(text))
            }
            (FlatKind::InlineString, _) => {
                DecodedValue::InlineString(InlineText::from_raw(self.raw))
            }
            (FlatKind::Atom, _) => DecodedValue::Atom(InternId {
                storage: scoped_id().storage(),
                slot: scoped_id().slot(),
            }),
            (FlatKind::String, _) => DecodedValue::ShortString(InternId {
                storage: scoped_id().storage(),
                slot: scoped_id().slot(),
            }),
            (FlatKind::Heap, HeapKind::Bytes) => DecodedValue::Bytes(handle()),
            (FlatKind::NativeType, _) => DecodedValue::NativeType(crate::value::NativeTypeId {
                module: crate::value::NativeModuleId(self.raw as u32),
                local: (self.raw >> 32) as u32,
            }),
            (FlatKind::Heap, HeapKind::DeclaredType) => DecodedValue::DeclaredType(handle()),
            (FlatKind::Heap, HeapKind::SymbolicType) => DecodedValue::SymbolicType(handle()),
            (FlatKind::Heap, HeapKind::Opaque) => DecodedValue::Opaque(handle()),
            (FlatKind::Heap, HeapKind::Array) => DecodedValue::Array(handle()),
            (FlatKind::Heap, HeapKind::Tuple) => DecodedValue::Tuple(handle()),
            (FlatKind::Heap, HeapKind::Tagged) => DecodedValue::Tagged(handle()),
            (FlatKind::Heap, HeapKind::Dict) => DecodedValue::Dict(handle()),
            (FlatKind::Heap, HeapKind::Func) => DecodedValue::Func(handle()),
            (FlatKind::Heap, HeapKind::Dyn) => DecodedValue::Dyn(handle()),
            (FlatKind::Heap, HeapKind::Module) => DecodedValue::Module(handle()),
            (FlatKind::TypeSlot, _) => DecodedValue::TypeSlot(handle()),
            (FlatKind::FuncRef, _) => DecodedValue::FuncRef(crate::FuncId {
                module: crate::ModuleId::from_raw(self.raw as u32),
                local: (self.raw >> 32) as u32,
            }),
            _ => unreachable!("invalid runtime Meta combination"),
        }
    }

    pub(crate) fn type_id(self) -> Option<crate::TypeId> {
        crate::TypeId::from_raw(self.ty)
    }

    pub(crate) fn with_type_id(self, ty: crate::TypeId) -> Self {
        Self {
            ty: ty.raw(),
            ..self
        }
    }

    pub(crate) fn without_type_id(self) -> Self {
        Self { ty: 0, ..self }
    }

    pub(crate) fn with_value(self, value: DecodedValue) -> Self {
        let (meta, raw) = value.encode();
        Self {
            meta: meta.with_provenance(self.meta.provenance()),
            raw,
            ..self
        }
    }

    #[cfg(test)]
    fn is_original(self) -> bool {
        matches!(self.meta.provenance(), Provenance::Original)
    }
}

impl PartialEq for Val {
    fn eq(&self, other: &Self) -> bool {
        self.meta.kind() == other.meta.kind()
            && self.meta.sub_kind() == other.meta.sub_kind()
            && self.ty == other.ty
            && self.narrow == other.narrow
            && self.raw == other.raw
    }
}

impl From<DecodedValue> for Val {
    fn from(value: DecodedValue) -> Self {
        Self::unknown(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PersistentValue(Val);

impl PersistentValue {
    pub(crate) fn export_get(self, heap: &Heap, name: &str) -> Result<Option<Self>, HeapError> {
        if heap.storage != Storage::Main {
            return Err(HeapError("persistent values require a Main world"));
        }
        let (shape, values) = match self.0.value() {
            DecodedValue::Module(handle) => {
                let Object::Module { exports } = heap.object(handle)? else {
                    return Err(HeapError(
                        "persistent Module handle has another object kind",
                    ));
                };
                (exports.shape, exports.values.as_ref())
            }
            DecodedValue::Dict(handle) => {
                let Object::Dict { shape, values } = heap.object(handle)? else {
                    return Err(HeapError("persistent Dict handle has another object kind"));
                };
                (*shape, values.as_ref())
            }
            _ => return Err(HeapError("persistent value has no exports")),
        };
        for (field, value) in heap.shape(shape)?.iter().zip(values) {
            if heap.resolve_text(*field)? == name {
                return Ok(Some(Self(*value)));
            }
        }
        Ok(None)
    }
}

