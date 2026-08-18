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

impl Heap {
    fn record_value(
        &mut self,
        entries: impl IntoIterator<Item = (String, Val)>,
    ) -> Result<Val, HeapError> {
        let mut entries = entries.into_iter().collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let fields = entries
            .iter()
            .map(|(name, _)| self.intern(name))
            .collect::<Vec<_>>();
        let values = entries
            .into_iter()
            .map(|(_, value)| value)
            .collect::<Vec<_>>();
        let shape = self.intern_shape(fields);
        Ok(Val::unknown(DecodedValue::Dict(self.allocate(
            Object::Dict {
                shape,
                values: values.into_boxed_slice(),
            },
        ))))
    }

    pub(crate) fn normalized_bool_type_value(
        &mut self,
        background: Option<&Heap>,
    ) -> Result<Val, HeapError> {
        fn wrap(heap: &mut Heap, background: Option<&Heap>, inner: Val) -> Result<Val, HeapError> {
            let attributes = heap.record_value([])?;
            let kind = Val::unknown(heap.atom(background, "WithAttributes"));
            heap.record_value([
                ("attributes".into(), attributes),
                ("inner".into(), inner),
                ("kind".into(), kind),
            ])
        }

        let none = Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::None));
        let false_variant = wrap(self, background, none)?;
        let true_variant = wrap(self, background, none)?;
        let variants = self.record_value([
            ("False".into(), false_variant),
            ("True".into(), true_variant),
        ])?;
        let kind = Val::unknown(self.atom(background, "Enum"));
        let metadata = self.record_value([("kind".into(), kind), ("variants".into(), variants)])?;
        wrap(self, background, metadata)
    }

    pub(crate) fn type_descriptor_value(
        &mut self,
        background: Option<&Heap>,
        descriptor: &crate::types::TypeDescriptor,
    ) -> Result<Val, HeapError> {
        fn record(
            heap: &mut Heap,
            entries: impl IntoIterator<Item = (String, Val)>,
        ) -> Result<Val, HeapError> {
            let mut entries = entries.into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            let fields = entries
                .iter()
                .map(|(name, _)| heap.intern(name))
                .collect::<Vec<_>>();
            let values = entries
                .into_iter()
                .map(|(_, value)| value)
                .collect::<Vec<_>>();
            let shape = heap.intern_shape(fields);
            Ok(Val::unknown(DecodedValue::Dict(heap.allocate(
                Object::Dict {
                    shape,
                    values: values.into_boxed_slice(),
                },
            ))))
        }

        fn build(
            heap: &mut Heap,
            background: Option<&Heap>,
            descriptor: &crate::types::TypeDescriptor,
            declared: &mut HashMap<crate::value::DeclaredTypeId, Val>,
        ) -> Result<Val, HeapError> {
            use crate::types::TypeDescriptor as T;

            let atom = |heap: &mut Heap, text: &str| Val::unknown(heap.atom(background, text));
            let kind = |heap: &mut Heap, name: &str| {
                let name = atom(heap, name);
                record(heap, [("kind".into(), name)])
            };
            match descriptor {
                T::Bound(parameter) => {
                    let kind = atom(heap, "Bound");
                    record(
                        heap,
                        [
                            ("kind".into(), kind),
                            (
                                "parameter".into(),
                                Val::unknown(DecodedValue::Int(i64::from(parameter.index()))),
                            ),
                        ],
                    )
                }
                T::Inference(_) => Err(HeapError(
                    "non-concrete type metadata cannot enter the runtime",
                )),
                T::Named(name) => {
                    let kind = atom(heap, "Named");
                    let name = Val::unknown(heap.string(background, name));
                    record(heap, [("kind".into(), kind), ("name".into(), name)])
                }
                T::Declared(value) => {
                    if let Some(existing) = declared.get(&value.id) {
                        return Ok(*existing);
                    }
                    let placeholder = kind(heap, "Any")?;
                    let owner = heap.reserve_type_metadata(
                        value.id.clone(),
                        value.name.as_str(),
                        placeholder,
                    )?;
                    declared.insert(value.id.clone(), owner);
                    let body = build(heap, background, &value.body, declared)?;
                    heap.seal_type_ref(owner, body)
                }
                T::Any => kind(heap, "Any"),
                T::Never => kind(heap, "Never"),
                T::Type => kind(heap, "Type"),
                T::Dyn => kind(heap, "Dyn"),
                T::Int => kind(heap, "Int"),
                T::Float => kind(heap, "Float"),
                T::String => kind(heap, "String"),
                T::Bytes => kind(heap, "Bytes"),
                T::TypeOf(instance) => {
                    let instance = build(heap, background, instance, declared)?;
                    let kind = atom(heap, "TypeOf");
                    record(heap, [("kind".into(), kind), ("instance".into(), instance)])
                }
                T::Opaque(native) => Ok(heap.native_type_value(native.clone())),
                T::Atom(tag) => {
                    let kind = atom(heap, "Atom");
                    let tag = atom(heap, tag.name());
                    record(heap, [("kind".into(), kind), ("tag".into(), tag)])
                }
                T::Array(item) | T::Dict(item) => {
                    let item = build(heap, background, item, declared)?;
                    let name = if matches!(descriptor, T::Array(_)) {
                        "Array"
                    } else {
                        "Dict"
                    };
                    let kind = atom(heap, name);
                    record(heap, [("kind".into(), kind), ("item".into(), item)])
                }
                T::Tagged { tag, payload } => {
                    let payload = build(heap, background, payload, declared)?;
                    let kind = atom(heap, "Tagged");
                    let tag = atom(heap, tag.name());
                    record(
                        heap,
                        [
                            ("kind".into(), kind),
                            ("tag".into(), tag),
                            ("payload".into(), payload),
                        ],
                    )
                }
                T::Tuple(items) | T::Union(items) => {
                    let items = items
                        .iter()
                        .map(|item| build(heap, background, item, declared))
                        .collect::<Result<Vec<_>, _>>()?;
                    let items = Val::unknown(DecodedValue::Array(
                        heap.allocate(Object::Array(items.into_boxed_slice())),
                    ));
                    let (name, field) = if matches!(descriptor, T::Tuple(_)) {
                        ("Tuple", "items")
                    } else {
                        ("Union", "variants")
                    };
                    let kind = atom(heap, name);
                    record(heap, [("kind".into(), kind), (field.into(), items)])
                }
                T::Struct(fields) => {
                    let fields = fields
                        .iter()
                        .map(|(name, value)| {
                            build(heap, background, value, declared)
                                .map(|value| (name.clone(), value))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let fields = record(heap, fields)?;
                    let kind = atom(heap, "Struct");
                    record(heap, [("kind".into(), kind), ("fields".into(), fields)])
                }
                T::Enum(variants) => {
                    let variants = variants
                        .iter()
                        .map(|(name, payload)| {
                            let value = match payload {
                                Some(payload) => build(heap, background, payload, declared)?,
                                None => Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::None)),
                            };
                            Ok((name.clone(), value))
                        })
                        .collect::<Result<Vec<_>, HeapError>>()?;
                    let variants = record(heap, variants)?;
                    let kind = atom(heap, "Enum");
                    record(heap, [("kind".into(), kind), ("variants".into(), variants)])
                }
                T::Function { parameters, result } => {
                    let parameters = parameters
                        .iter()
                        .map(|item| build(heap, background, item, declared))
                        .collect::<Result<Vec<_>, _>>()?;
                    let parameters = Val::unknown(DecodedValue::Array(
                        heap.allocate(Object::Array(parameters.into_boxed_slice())),
                    ));
                    let result = build(heap, background, result, declared)?;
                    let kind = atom(heap, "Func");
                    record(
                        heap,
                        [
                            ("kind".into(), kind),
                            ("parameters".into(), parameters),
                            ("result".into(), result),
                        ],
                    )
                }
            }
        }

        build(self, background, descriptor, &mut HashMap::new())
    }

    pub(crate) fn int(&self, value: i64) -> Val {
        Val::unknown(DecodedValue::Int(value))
    }

    pub(crate) fn native_closure(
        &mut self,
        function: NativeFunction,
        upvalues: impl Into<Box<[Val]>>,
    ) -> Val {
        let handle = self.allocate(Object::Closure {
            identity: Arc::new(()),
            prototype: RuntimePrototype::Native(function),
            upvalues: upvalues.into(),
        });
        Val::unknown(DecodedValue::Func(handle))
    }

    pub(crate) fn native_type_value(&mut self, value: crate::NativeType) -> Val {
        let id = self.intern_native_type(value);
        Val::unknown(DecodedValue::NativeType(id))
    }

    pub(crate) fn persistent(&self, value: Val) -> Result<PersistentValue, HeapError> {
        if self.storage != Storage::Main {
            return Err(HeapError("persistent value must belong to the Main world"));
        }
        Ok(PersistentValue(value))
    }

    pub(crate) fn declare_type(
        &mut self,
        body: Val,
        module: crate::ModuleId,
        declaration: u32,
        name: impl Into<Arc<str>>,
    ) -> Result<Val, HeapError> {
        let id = crate::value::DeclaredTypeId::concrete(module, declaration);
        let type_id = self.canonical_declared_type_id(&id)?;
        let handle = self.allocate_declared_type(Object::DeclaredType {
            type_id,
            id,
            name: name.into(),
            body,
            sealed: true,
            application_arguments: None,
        });
        Ok(Val::unknown(DecodedValue::DeclaredType(handle)))
    }

    pub(crate) fn reserve_type_ref(
        &mut self,
        module: crate::ModuleId,
        declaration: u32,
        name: impl Into<Arc<str>>,
        placeholder: Val,
    ) -> Result<Val, HeapError> {
        let id = crate::value::DeclaredTypeId::concrete(module, declaration);
        self.reserve_declared_type(id, name, placeholder)
    }

    fn reserve_declared_type(
        &mut self,
        id: crate::value::DeclaredTypeId,
        name: impl Into<Arc<str>>,
        placeholder: Val,
    ) -> Result<Val, HeapError> {
        let type_id = self.canonical_declared_type_id(&id)?;
        let handle = self.allocate_declared_type(Object::DeclaredType {
            type_id,
            id,
            name: name.into(),
            body: placeholder,
            sealed: false,
            application_arguments: None,
        });
        Ok(Val::unknown(DecodedValue::DeclaredType(handle)))
    }

    fn reserve_type_metadata(
        &mut self,
        id: crate::value::DeclaredTypeId,
        name: impl Into<Arc<str>>,
        placeholder: Val,
    ) -> Result<Val, HeapError> {
        match self.canonical_declared_type_id(&id) {
            Ok(_) => self.reserve_declared_type(id, name, placeholder),
            Err(_)
                if id
                    .arguments()
                    .iter()
                    .any(crate::types::type_identity_is_symbolic) =>
            {
                let handle = self.allocate(Object::SymbolicType {
                    id,
                    name: name.into(),
                    body: placeholder,
                    sealed: false,
                    application_arguments: None,
                });
                Ok(Val::unknown(DecodedValue::SymbolicType(handle)))
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) fn seal_type_ref(&mut self, target: Val, body: Val) -> Result<Val, HeapError> {
        let handle = match target.value() {
            DecodedValue::DeclaredType(handle) | DecodedValue::SymbolicType(handle) => handle,
            _ => return Err(HeapError("type ref target is not declared type metadata")),
        };
        if handle.storage != Storage::Work {
            return Err(HeapError(
                "type refs can only be sealed in their Work world",
            ));
        }
        let (slot, sealed) = match self.object_mut(handle)? {
            Object::DeclaredType { body, sealed, .. }
            | Object::SymbolicType { body, sealed, .. } => (body, sealed),
            _ => return Err(HeapError("type ref target is not declared type metadata")),
        };
        if *sealed {
            return Err(HeapError("type ref is already sealed"));
        }
        *slot = body;
        *sealed = true;
        Ok(target)
    }
}

impl PersistentValue {
    pub(crate) const fn runtime(self) -> Val {
        self.0
    }

    pub(crate) fn without_location(self) -> Self {
        Self(self.0.with_loc(None))
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum RuntimePrototype {
    Bytecode(Handle),
    Native(NativeFunction),
}

#[derive(Clone, Debug)]
pub(crate) struct ExportTable {
    shape: ShapeId,
    values: Box<[Val]>,
}

#[derive(Clone, Debug)]
pub(crate) enum Object {
    Reserved,
    OpenFunc,
    Bytes(Box<[u8]>),
    DeclaredType {
        type_id: crate::TypeId,
        id: crate::value::DeclaredTypeId,
        name: Arc<str>,
        body: Val,
        sealed: bool,
        application_arguments: Option<Box<[Val]>>,
    },
    SymbolicType {
        id: crate::value::DeclaredTypeId,
        name: Arc<str>,
        body: Val,
        sealed: bool,
        application_arguments: Option<Box<[Val]>>,
    },
    Opaque(crate::value::OpaqueValue),
    Array(Box<[Val]>),
    Tuple(Box<[Val]>),
    Tagged {
        tag: Val,
        payload: Val,
    },
    Dict {
        shape: ShapeId,
        values: Box<[Val]>,
    },
    Module {
        exports: ExportTable,
    },
    Closure {
        identity: Arc<()>,
        prototype: RuntimePrototype,
        upvalues: Box<[Val]>,
    },
    Dyn {
        identity: Arc<()>,
        descriptor: Val,
        value: Val,
        scheme: Option<crate::TypeScheme>,
        origin: Option<Arc<str>>,
    },
    TypeSlot {
        value: Option<Val>,
    },
    ByteCodeProto {
        code: Arc<FuncByteCode>,
        values: Box<[Val]>,
        text: Box<[InternId]>,
        prototypes: Box<[RuntimePrototype]>,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct HeapError {
    message: std::borrow::Cow<'static, str>,
}

#[allow(non_snake_case)]
fn HeapError(message: &'static str) -> HeapError {
    HeapError::new(message)
}

impl HeapError {
    pub(crate) const fn new(message: &'static str) -> Self {
        Self {
            message: std::borrow::Cow::Borrowed(message),
        }
    }

    pub(crate) fn owned(message: String) -> Self {
        Self {
            message: std::borrow::Cow::Owned(message),
        }
    }
}

impl fmt::Display for HeapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Default)]
struct TextTable {
    values: Vec<Arc<str>>,
    slots: HashMap<Arc<str>, u32>,
}

impl TextTable {
    fn find(&self, text: &str) -> Option<u32> {
        self.slots.get(text).copied()
    }

    fn resolve(&self, slot: u32) -> Option<&str> {
        self.values.get(slot as usize).map(AsRef::as_ref)
    }

    fn insert(&mut self, text: &str) -> u32 {
        if let Some(slot) = self.find(text) {
            return slot;
        }
        let slot = self.values.len() as u32;
        let value: Arc<str> = text.into();
        self.values.push(value.clone());
        self.slots.insert(value, slot);
        slot
    }
}

pub(crate) struct Heap {
    storage: Storage,
    types: crate::type_store::SharedTypeStore,
    objects: Vec<Object>,
    text: TextTable,
    native_types: HashMap<crate::value::NativeTypeId, crate::NativeType>,
    shapes: Vec<Box<[InternId]>>,
    shape_slots: HashMap<Vec<InternId>, u32>,
    bootstrap_root: Option<PersistentValue>,
    functions: HashMap<crate::FuncId, Option<Val>>,
    declared_types: HashMap<crate::TypeId, Val>,
}

impl Heap {
    fn new(storage: Storage, types: crate::type_store::SharedTypeStore) -> Self {
        Self {
            storage,
            types,
            objects: Vec::new(),
            text: TextTable::default(),
            native_types: HashMap::new(),
            shapes: Vec::new(),
            shape_slots: HashMap::new(),
            bootstrap_root: None,
            functions: HashMap::new(),
            declared_types: HashMap::new(),
        }
    }

    pub(crate) fn preallocate_func(&mut self, id: crate::FuncId) -> Result<(), HeapError> {
        if self.storage != Storage::Main {
            return Err(HeapError(
                "static function slots must be preallocated in Main world",
            ));
        }
        if self.functions.insert(id, None).is_some() {
            return Err(HeapError("duplicate static function slot"));
        }
        Ok(())
    }

    pub(crate) fn seal_static_func(
        &mut self,
        id: crate::FuncId,
        value: Val,
    ) -> Result<(), HeapError> {
        if !matches!(
            value.value(),
            DecodedValue::Func(_) | DecodedValue::FuncRef(_)
        ) {
            return Err(HeapError(
                "static function definition did not produce a closure",
            ));
        }
        match self.functions.entry(id) {
            std::collections::hash_map::Entry::Vacant(entry) if self.storage == Storage::Work => {
                entry.insert(Some(value));
                Ok(())
            }
            std::collections::hash_map::Entry::Occupied(mut entry) if entry.get().is_none() => {
                entry.insert(Some(value));
                Ok(())
            }
            std::collections::hash_map::Entry::Vacant(_) => {
                Err(HeapError("unknown static function slot"))
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                Err(HeapError("static function slot is already sealed"))
            }
        }
    }

    pub(crate) fn static_func(&self, id: crate::FuncId) -> Option<Val> {
        self.functions.get(&id).copied().flatten()
    }

    pub(crate) fn bootstrap_root(&self) -> Option<PersistentValue> {
        self.bootstrap_root
    }

    pub(crate) fn set_bootstrap_root(&mut self, root: PersistentValue) {
        debug_assert!(self.bootstrap_root.is_none());
        self.bootstrap_root = Some(root);
    }

    pub(crate) fn module(
        &mut self,
        entries: impl IntoIterator<Item = (String, Val)>,
    ) -> Result<Val, HeapError> {
        let mut entries = entries.into_iter().collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        if entries.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(HeapError("Module exports contain a duplicate field"));
        }
        let mut fields = Vec::with_capacity(entries.len());
        let mut values = Vec::with_capacity(entries.len());
        for (field, value) in entries {
            fields.push(self.intern(&field));
            values.push(value);
        }
        let shape = self.intern_shape(fields);
        let handle = self.allocate(Object::Module {
            exports: ExportTable {
                shape,
                values: values.into_boxed_slice(),
            },
        });
        Ok(Val::unknown(DecodedValue::Module(handle)))
    }

    pub(crate) fn seal_module(&mut self, root: Val) -> Result<Val, HeapError> {
        if matches!(root.value(), DecodedValue::Module(_)) {
            return Ok(root);
        }
        let DecodedValue::Dict(handle) = root.value() else {
            return Err(HeapError(
                "module evaluation must produce a Dict of exports",
            ));
        };
        if handle.storage != self.storage {
            return Err(HeapError("module exports Dict belongs to another world"));
        }
        let object = self
            .objects
            .get_mut(handle.slot as usize)
            .ok_or(HeapError("module exports Dict is out of bounds"))?;
        let Object::Dict { shape, values } = std::mem::replace(object, Object::Reserved) else {
            return Err(HeapError("module exports handle has another object kind"));
        };
        *object = Object::Module {
            exports: ExportTable { shape, values },
        };
        Ok(root.with_value(DecodedValue::Module(handle)))
    }

    pub(crate) fn work() -> Self {
        Self::new(Storage::Work, crate::type_store::shared_type_store())
    }

    pub(crate) fn main() -> Self {
        Self::new(Storage::Main, crate::type_store::shared_type_store())
    }

    pub(crate) fn work_for(background: &Self) -> Self {
        Self::new(Storage::Work, Arc::clone(&background.types))
    }

    pub(crate) fn canonical_declared_type_id(
        &self,
        declared: &crate::value::DeclaredTypeId,
    ) -> Result<crate::TypeId, HeapError> {
        let mut types = self
            .types
            .lock()
            .map_err(|_| HeapError("type store poisoned"))?;
        let arguments = declared
            .arguments()
            .iter()
            .map(|argument| types.intern_descriptor(argument))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                HeapError::owned(format!("declared type argument is not canonical: {error}"))
            })?;
        Ok(match types.begin(declared.constructor(), arguments) {
            crate::type_store::InternType::Existing(id)
            | crate::type_store::InternType::Reserved(id) => id,
        })
    }

    pub(crate) fn canonical_descriptor_type_id(
        &self,
        descriptor: &crate::types::TypeDescriptor,
    ) -> Result<crate::TypeId, HeapError> {
        self.types
            .lock()
            .map_err(|_| HeapError("type store poisoned"))?
            .intern_descriptor(descriptor)
            .map_err(HeapError::owned)
    }

    pub(crate) fn canonical_type_name(
        &self,
        type_id: crate::TypeId,
    ) -> Result<Option<String>, HeapError> {
        let types = self
            .types
            .lock()
            .map_err(|_| HeapError("type store poisoned"))?;
        Ok(types.get(type_id).map(|data| data.name.clone()))
    }

    #[cfg(test)]
    pub(crate) fn counts(&self) -> (usize, usize, usize) {
        (
            self.objects.len(),
            self.text.values.len(),
            self.shapes.len(),
        )
    }

    pub(crate) fn allocate(&mut self, object: Object) -> Handle {
        let handle = Handle {
            storage: self.storage,
            slot: self.objects.len() as u32,
        };
        self.objects.push(object);
        handle
    }

    pub(crate) fn allocate_declared_type(&mut self, object: Object) -> Handle {
        let Object::DeclaredType { type_id, .. } = &object else {
            panic!("allocate_declared_type requires declared type metadata")
        };
        let type_id = *type_id;
        let handle = self.allocate(object);
        self.declared_types
            .entry(type_id)
            .or_insert_with(|| Val::unknown(DecodedValue::DeclaredType(handle)));
        handle
    }

    fn intern_native_type(&mut self, value: crate::NativeType) -> crate::value::NativeTypeId {
        let id = value.id();
        self.native_types.entry(id).or_insert(value);
        id
    }

    fn native_type(&self, id: crate::value::NativeTypeId) -> Result<&crate::NativeType, HeapError> {
        self.native_types
            .get(&id)
            .ok_or(HeapError("native type ID is not registered in this world"))
    }

    pub(crate) fn reserve(&mut self) -> Handle {
        self.allocate(Object::Reserved)
    }

    pub(crate) fn initialize(&mut self, handle: Handle, object: Object) -> Result<(), HeapError> {
        let slot = self.object_mut(handle)?;
        if !matches!(slot, Object::Reserved) {
            return Err(HeapError("heap slot is already initialized"));
        }
        *slot = object;
        Ok(())
    }

    pub(crate) fn initialize_type_slot(
        &mut self,
        handle: Handle,
        value: Val,
    ) -> Result<(), HeapError> {
        if handle.storage != Storage::Work {
            return Err(HeapError("Main up-links are read-only"));
        }
        let Object::TypeSlot { value: slot } = self.object_mut(handle)? else {
            return Err(HeapError("handle is not an up-link"));
        };
        if slot.is_some() {
            return Err(HeapError("up-link is already initialized"));
        }
        *slot = Some(value);
        Ok(())
    }

    pub(crate) fn seal_local_func(
        &mut self,
        target: Handle,
        source: Handle,
    ) -> Result<(), HeapError> {
        if target.storage != Storage::Work || source.storage != Storage::Work {
            return Err(HeapError(
                "function refs can only be sealed in their Work world",
            ));
        }
        let closure = match self.object(source)? {
            Object::Closure { .. } => self.object(source)?.clone(),
            _ => return Err(HeapError("function ref source is not a sealed function")),
        };
        let slot = self.object_mut(target)?;
        if !matches!(slot, Object::OpenFunc) {
            return Err(HeapError("function ref is already sealed"));
        }
        *slot = closure;
        Ok(())
    }

    pub(crate) fn object(&self, handle: Handle) -> Result<&Object, HeapError> {
        if handle.storage != self.storage {
            return Err(HeapError("object handle belongs to another heap"));
        }
        self.objects
            .get(handle.slot as usize)
            .ok_or(HeapError("object handle is out of bounds"))
    }

    fn object_mut(&mut self, handle: Handle) -> Result<&mut Object, HeapError> {
        if handle.storage != self.storage {
            return Err(HeapError("object handle belongs to another heap"));
        }
        self.objects
            .get_mut(handle.slot as usize)
            .ok_or(HeapError("object handle is out of bounds"))
    }

    pub(crate) fn intern(&mut self, text: &str) -> InternId {
        InternId {
            storage: self.storage,
            slot: self.text.insert(text),
        }
    }

    pub(crate) fn find_text(&self, text: &str) -> Option<InternId> {
        self.text.find(text).map(|slot| InternId {
            storage: self.storage,
            slot,
        })
    }

    pub(crate) fn resolve_text(&self, id: InternId) -> Result<&str, HeapError> {
        if id.storage != self.storage {
            return Err(HeapError("intern ID belongs to another heap"));
        }
        self.text
            .resolve(id.slot)
            .ok_or(HeapError("intern ID is out of bounds"))
    }

    pub(crate) fn string(&mut self, background: Option<&Heap>, text: &str) -> DecodedValue {
        if let Some(text) = InlineText::new(text) {
            DecodedValue::InlineString(text)
        } else {
            if let Some(id) = background.and_then(|heap| heap.find_text(text)) {
                DecodedValue::ShortString(id)
            } else {
                DecodedValue::ShortString(self.intern(text))
            }
        }
    }

    pub(crate) fn atom(&mut self, background: Option<&Heap>, text: &str) -> DecodedValue {
        if let Some(builtin) = builtin_atom(text) {
            DecodedValue::BuiltinAtom(builtin)
        } else if let Some(text) = InlineText::new(text) {
            DecodedValue::InlineAtom(text)
        } else if let Some(id) = background.and_then(|heap| heap.find_text(text)) {
            DecodedValue::Atom(id)
        } else {
            DecodedValue::Atom(self.intern(text))
        }
    }

    pub(crate) fn intern_shape(&mut self, fields: Vec<InternId>) -> ShapeId {
        if let Some(slot) = self.shape_slots.get(&fields) {
            return ShapeId {
                storage: self.storage,
                slot: *slot,
            };
        }
        let slot = self.shapes.len() as u32;
        self.shapes.push(fields.clone().into());
        self.shape_slots.insert(fields, slot);
        ShapeId {
            storage: self.storage,
            slot,
        }
    }

    fn shape(&self, id: ShapeId) -> Result<&[InternId], HeapError> {
        if id.storage != self.storage {
            return Err(HeapError("shape ID belongs to another heap"));
        }
        self.shapes
            .get(id.slot as usize)
            .map(AsRef::as_ref)
            .ok_or(HeapError("shape ID is out of bounds"))
    }

    pub(crate) fn link_bytecode_resolved(
        &mut self,
        background: Option<&Heap>,
        function: &BytecodeFunction,
        externals: &HashMap<String, Val>,
    ) -> Result<Handle, HeapError> {
        self.link_bytecode_with(background, function, externals, &mut HashMap::new())
    }

    fn link_bytecode_with(
        &mut self,
        background: Option<&Heap>,
        function: &BytecodeFunction,
        externals: &HashMap<String, Val>,
        forwarded: &mut HashMap<*const BytecodeFunction, Handle>,
    ) -> Result<Handle, HeapError> {
        let identity = std::ptr::from_ref(function);
        if let Some(handle) = forwarded.get(&identity) {
            return Ok(*handle);
        }
        let handle = self.reserve();
        forwarded.insert(identity, handle);
        let values = function
            .links()
            .values()
            .iter()
            .enumerate()
            .map(|(index, value)| {
                if let Some(key) = function.links().external_value(index) {
                    let resolved = externals
                        .get(key)
                        .copied()
                        .ok_or(HeapError("external value link is unresolved"))?;
                    if key.starts_with("\0declared-owner:")
                        && !matches!(resolved.value(), DecodedValue::DeclaredType(_))
                    {
                        return Err(HeapError(
                            "declared owner external link did not resolve to a TypeRef",
                        ));
                    }
                    return Ok(resolved);
                }
                Ok(match value {
                    Constant::Placeholder => {
                        return Err(HeapError("unresolved bytecode constant placeholder"));
                    }
                    Constant::Int(value) => Val::unknown(DecodedValue::Int(*value)),
                    Constant::Float(value) if value.is_finite() => {
                        Val::unknown(DecodedValue::Float(*value))
                    }
                    Constant::Float(_) => return Err(HeapError("Telora Float must be finite")),
                    Constant::String(value) => Val::unknown(self.string(background, value)),
                    Constant::Bytes(value) => Val::unknown(DecodedValue::Bytes(
                        self.allocate(Object::Bytes(value.as_ref().into())),
                    )),
                    Constant::Atom(value) => Val::unknown(self.atom(background, value.name())),
                    Constant::Native(function) => self.native_closure(*function, []),
                })
            })
            .collect::<Result<Box<[_]>, _>>()?;
        let text = function
            .links()
            .text()
            .iter()
            .map(|text| {
                background
                    .and_then(|heap| heap.find_text(text))
                    .unwrap_or_else(|| self.intern(text))
            })
            .collect::<Box<[_]>>();
        let prototypes = function
            .links()
            .prototypes()
            .iter()
            .map(|prototype| {
                self.link_bytecode_with(background, prototype, externals, forwarded)
                    .map(RuntimePrototype::Bytecode)
            })
            .collect::<Result<Box<[_]>, _>>()?;
        self.initialize(
            handle,
            Object::ByteCodeProto {
                code: Arc::clone(function.code()),
                values,
                text,
                prototypes,
            },
        )?;
        Ok(handle)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct HeapView<'a> {
    pub(crate) current: &'a Heap,
    pub(crate) background: Option<&'a Heap>,
}

type BytecodeLinks<'a> = (
    &'a Arc<FuncByteCode>,
    &'a [Val],
    &'a [InternId],
    &'a [RuntimePrototype],
);

impl<'a> HeapView<'a> {
    pub(crate) fn static_func(&self, id: crate::FuncId) -> Option<Val> {
        self.current
            .static_func(id)
            .or_else(|| self.background.and_then(|heap| heap.static_func(id)))
    }

    pub(crate) fn resolve_func(&self, mut value: Val) -> Result<Option<Handle>, HeapError> {
        let mut visited = HashSet::new();
        loop {
            match value.value() {
                DecodedValue::Func(handle) => return Ok(Some(handle)),
                DecodedValue::FuncRef(id) => {
                    if !visited.insert(id) {
                        return Err(HeapError("cyclic static function alias"));
                    }
                    value = self
                        .static_func(id)
                        .ok_or(HeapError("static function slot is not sealed"))?;
                }
                _ => return Ok(None),
            }
        }
    }

    pub(crate) fn resolved_function_arity(&self, value: Val) -> Result<Option<usize>, HeapError> {
        self.resolve_func(value)?
            .map(|handle| self.function_arity(handle))
            .transpose()
    }

    fn heap(&self, storage: Storage) -> Result<&'a Heap, HeapError> {
        match storage {
            Storage::Work if self.current.storage == Storage::Work => Ok(self.current),
            Storage::Main if self.current.storage == Storage::Main => Ok(self.current),
            Storage::Main => self
                .background
                .filter(|heap| heap.storage == Storage::Main)
                .ok_or(HeapError("Main value has no Main world")),
            _ => Err(HeapError("value refers to a heap outside its view")),
        }
    }

    pub(crate) fn object(&self, handle: Handle) -> Result<&'a Object, HeapError> {
        self.heap(handle.storage)?.object(handle)
    }

    pub(crate) fn unwrap_declared(&self, mut value: Val) -> Result<Val, HeapError> {
        if value.type_id().is_some() {
            self.type_witness(value)?;
            value = value.without_type_id();
        }
        Ok(value)
    }

    pub(crate) fn type_witness(&self, value: Val) -> Result<Option<Val>, HeapError> {
        let Some(type_id) = value.type_id() else {
            return Ok(None);
        };
        let owner = self
            .current
            .declared_types
            .get(&type_id)
            .or_else(|| self.background?.declared_types.get(&type_id))
            .copied();
        let owner = owner.ok_or(HeapError("canonical type ID has no metadata in this world"))?;
        let DecodedValue::DeclaredType(handle) = owner.value() else {
            return Err(HeapError("type metadata has another value kind"));
        };
        if !matches!(self.object(handle)?, Object::DeclaredType { .. }) {
            return Err(HeapError("type metadata refers to another object kind"));
        }
        Ok(Some(owner))
    }

    pub(crate) fn declared_type_id(&self, owner: Val) -> Result<crate::TypeId, HeapError> {
        let DecodedValue::DeclaredType(handle) = owner.value() else {
            return Err(HeapError("declared value owner is not a declared Type"));
        };
        let Object::DeclaredType {
            type_id, sealed, ..
        } = self.object(handle)?
        else {
            return Err(HeapError("declared value owner has another object kind"));
        };
        if !sealed {
            return Err(HeapError("declared value owner is not sealed"));
        }
        Ok(*type_id)
    }

    pub(crate) fn canonical_type_name(
        &self,
        type_id: crate::TypeId,
    ) -> Result<Option<String>, HeapError> {
        self.current.canonical_type_name(type_id)
    }

    pub(crate) fn text(&self, id: InternId) -> Result<&'a str, HeapError> {
        self.heap(id.storage)?.resolve_text(id)
    }

    pub(crate) fn native_type(
        &self,
        id: crate::value::NativeTypeId,
    ) -> Result<&'a crate::NativeType, HeapError> {
        self.current.native_type(id).or_else(|_| {
            self.background
                .ok_or(HeapError("native type ID is not registered in this view"))?
                .native_type(id)
        })
    }

    fn shape(&self, id: ShapeId) -> Result<&'a [InternId], HeapError> {
        self.heap(id.storage)?.shape(id)
    }

    pub(crate) fn bytecode(&self, handle: Handle) -> Result<BytecodeLinks<'a>, HeapError> {
        let Object::ByteCodeProto {
            code,
            values,
            text,
            prototypes,
        } = self.object(handle)?
        else {
            return Err(HeapError("handle is not a bytecode prototype"));
        };
        Ok((code, values, text, prototypes))
    }

    pub(crate) fn closure(
        &self,
        handle: Handle,
    ) -> Result<(RuntimePrototype, &'a [Val]), HeapError> {
        let Object::Closure {
            prototype,
            upvalues,
            ..
        } = self.object(handle)?
        else {
            return Err(HeapError("handle is not a closure"));
        };
        Ok((*prototype, upvalues))
    }

    pub(crate) fn function_arity(&self, handle: Handle) -> Result<usize, HeapError> {
        let (prototype, _) = self.closure(handle)?;
        match prototype {
            RuntimePrototype::Native(function) => Ok(function.arity()),
            RuntimePrototype::Bytecode(handle) => {
                let Object::ByteCodeProto { code, .. } = self.object(handle)? else {
                    return Err(HeapError("prototype handle refers to another object kind"));
                };
                Ok(code.parameter_count())
            }
        }
    }

    pub(crate) fn type_slot(&self, handle: Handle) -> Result<Option<Val>, HeapError> {
        let Object::TypeSlot { value } = self.object(handle)? else {
            return Err(HeapError("handle is not an up-link"));
        };
        Ok(*value)
    }

    pub(crate) fn dyn_parts(&self, handle: Handle) -> Result<(&'a Arc<()>, Val, Val), HeapError> {
        let Object::Dyn {
            identity,
            descriptor,
            value,
            ..
        } = self.object(handle)?
        else {
            return Err(HeapError("handle is not a Dyn"));
        };
        Ok((identity, *descriptor, *value))
    }

    pub(crate) fn sequence(&self, handle: Handle, tuple: bool) -> Result<&'a [Val], HeapError> {
        match self.object(handle)? {
            Object::Array(values) if !tuple => Ok(values),
            Object::Tuple(values) if tuple => Ok(values),
            _ => Err(HeapError("handle is not the requested sequence kind")),
        }
    }

    pub(crate) fn tagged(&self, handle: Handle) -> Result<(Val, Val), HeapError> {
        let Object::Tagged { tag, payload } = self.object(handle)? else {
            return Err(HeapError("handle is not a Tagged value"));
        };
        Ok((*tag, *payload))
    }

    pub(crate) fn dict_get(
        &self,
        handle: Handle,
        field: InternId,
    ) -> Result<Option<Val>, HeapError> {
        let Object::Dict { shape, values } = self.object(handle)? else {
            return Err(HeapError("handle is not a Dict"));
        };
        let wanted = self.text(field)?;
        let fields = self.shape(*shape)?;
        let index = fields
            .binary_search_by(|candidate| {
                if *candidate == field {
                    Ordering::Equal
                } else {
                    self.text(*candidate).unwrap_or("").cmp(wanted)
                }
            })
            .ok();
        Ok(index.and_then(|index| values.get(index).copied()))
    }

    pub(crate) fn exports_get(
        &self,
        handle: Handle,
        field: InternId,
    ) -> Result<Option<Val>, HeapError> {
        let Object::Module { exports, .. } = self.object(handle)? else {
            return Err(HeapError("handle is not a Module"));
        };
        let wanted = self.text(field)?;
        let fields = self.shape(exports.shape)?;
        let index = fields
            .binary_search_by(|candidate| {
                if *candidate == field {
                    Ordering::Equal
                } else {
                    self.text(*candidate).unwrap_or("").cmp(wanted)
                }
            })
            .ok();
        Ok(index.and_then(|index| exports.values.get(index).copied()))
    }

    pub(crate) fn exports_fields(&self, handle: Handle) -> Result<Vec<&'a str>, HeapError> {
        let Object::Module { exports, .. } = self.object(handle)? else {
            return Err(HeapError("handle is not a Module"));
        };
        self.shape(exports.shape)?
            .iter()
            .map(|field| self.text(*field))
            .collect()
    }

    pub(crate) fn dict_fields(&self, handle: Handle) -> Result<Vec<&'a str>, HeapError> {
        let Object::Dict { shape, .. } = self.object(handle)? else {
            return Err(HeapError("handle is not a Dict"));
        };
        self.shape(*shape)?
            .iter()
            .map(|field| self.text(*field))
            .collect()
    }

    pub(crate) fn module_fields(&self, handle: Handle) -> Result<Vec<&'a str>, HeapError> {
        let Object::Module { exports } = self.object(handle)? else {
            return Err(HeapError("handle is not a Module"));
        };
        self.shape(exports.shape)?
            .iter()
            .map(|field| self.text(*field))
            .collect()
    }

    pub(crate) fn module_get_text(
        &self,
        handle: Handle,
        field: &str,
    ) -> Result<Option<Val>, HeapError> {
        let Object::Module { exports } = self.object(handle)? else {
            return Err(HeapError("handle is not a Module"));
        };
        let fields = self.shape(exports.shape)?;
        let index = fields
            .binary_search_by(|candidate| self.text(*candidate).unwrap_or("").cmp(field))
            .ok();
        Ok(index.and_then(|index| exports.values.get(index).copied()))
    }

    pub(crate) fn dict_parts(
        &self,
        handle: Handle,
    ) -> Result<(&'a [InternId], &'a [Val]), HeapError> {
        let Object::Dict { shape, values } = self.object(handle)? else {
            return Err(HeapError("handle is not a Dict"));
        };
        Ok((self.shape(*shape)?, values))
    }

    pub(crate) fn dict_get_text(
        &self,
        handle: Handle,
        field: &str,
    ) -> Result<Option<Val>, HeapError> {
        let Object::Dict { shape, values } = self.object(handle)? else {
            return Err(HeapError("handle is not a Dict"));
        };
        let fields = self.shape(*shape)?;
        let index = fields
            .binary_search_by(|candidate| self.text(*candidate).unwrap_or("").cmp(field))
            .ok();
        Ok(index.and_then(|index| values.get(index).copied()))
    }

    pub(crate) fn string_text(&self, value: Val) -> Result<Option<TextRef<'a>>, HeapError> {
        match value.value() {
            DecodedValue::InlineString(text) => Ok(Some(TextRef::inline(text))),
            DecodedValue::ShortString(id) => Ok(Some(TextRef::borrowed(self.text(id)?))),
            _ => Ok(None),
        }
    }

    pub(crate) fn atom_text(&self, value: Val) -> Result<Option<TextRef<'a>>, HeapError> {
        match value.value() {
            DecodedValue::BuiltinAtom(atom) => Ok(Some(TextRef::borrowed(atom.name()))),
            DecodedValue::InlineAtom(text) => Ok(Some(TextRef::inline(text))),
            DecodedValue::Atom(id) => Ok(Some(TextRef::borrowed(self.text(id)?))),
            _ => Ok(None),
        }
    }

    pub(crate) fn values_equal(&self, left: Val, right: Val) -> Result<bool, HeapError> {
        self.values_equal_with(left, right, &mut HashSet::new())
    }

    /// Returns the first failed node reachable through data containers.
    ///
    /// Closures and opaque/native values are intentionally atomic here: an
    /// operation only depends on their identity, not on captured internals.
    pub(crate) fn first_data_failure(&self, root: Val) -> Result<Option<u32>, HeapError> {
        let mut pending = vec![root];
        let mut visited = HashSet::new();
        while let Some(value) = pending.pop() {
            let handle = match value.value() {
                DecodedValue::Failed(failure) => return Ok(Some(failure)),
                DecodedValue::Array(handle)
                | DecodedValue::Tuple(handle)
                | DecodedValue::Tagged(handle)
                | DecodedValue::Dict(handle)
                | DecodedValue::Dyn(handle)
                | DecodedValue::Module(handle) => handle,
                DecodedValue::Int(_)
                | DecodedValue::Float(_)
                | DecodedValue::BuiltinAtom(_)
                | DecodedValue::InlineAtom(_)
                | DecodedValue::Atom(_)
                | DecodedValue::InlineString(_)
                | DecodedValue::ShortString(_)
                | DecodedValue::Bytes(_)
                | DecodedValue::Opaque(_)
                | DecodedValue::NativeType(_)
                | DecodedValue::DeclaredType(_)
                | DecodedValue::SymbolicType(_)
                | DecodedValue::Func(_)
                | DecodedValue::TypeSlot(_)
                | DecodedValue::FuncRef(_) => continue,
            };
            if !visited.insert(handle) {
                continue;
            }
            match self.object(handle)? {
                Object::Array(values) | Object::Tuple(values) => {
                    pending.extend(values.iter().rev().copied());
                }
                Object::Tagged { tag, payload } => {
                    pending.push(*payload);
                    pending.push(*tag);
                }
                Object::Dict { values, .. } => {
                    pending.extend(values.iter().rev().copied());
                }
                Object::Module { exports, .. } => {
                    pending.extend(exports.values.iter().rev().copied());
                }
                Object::Dyn {
                    descriptor, value, ..
                } => {
                    pending.push(*value);
                    pending.push(*descriptor);
                }
                Object::Bytes(_)
                | Object::Opaque(_)
                | Object::DeclaredType { .. }
                | Object::SymbolicType { .. }
                | Object::Closure { .. }
                | Object::TypeSlot { .. }
                | Object::ByteCodeProto { .. }
                | Object::OpenFunc
                | Object::Reserved => {}
            }
        }
        Ok(None)
    }

    fn values_equal_with(
        &self,
        left: Val,
        right: Val,
        visited: &mut HashSet<(Handle, Handle)>,
    ) -> Result<bool, HeapError> {
        match (left.type_id(), right.type_id()) {
            (Some(left_id), Some(right_id)) => {
                if left_id != right_id {
                    return Ok(false);
                }
            }
            (Some(_), None) | (None, Some(_)) => {
                return Ok(false);
            }
            (None, None) => {}
        }
        let left = left.without_type_id();
        let right = right.without_type_id();
        if matches!(left.value(), DecodedValue::FuncRef(_))
            || matches!(right.value(), DecodedValue::FuncRef(_))
        {
            let Some(left) = self.resolve_func(left)? else {
                return Ok(false);
            };
            let Some(right) = self.resolve_func(right)? else {
                return Ok(false);
            };
            let Object::Closure { identity: left, .. } = self.object(left)? else {
                return Err(HeapError("Func handle refers to another object kind"));
            };
            let Object::Closure {
                identity: right, ..
            } = self.object(right)?
            else {
                return Err(HeapError("Func handle refers to another object kind"));
            };
            return Ok(Arc::ptr_eq(left, right));
        }
        match (left.value(), right.value()) {
            (DecodedValue::Func(left), DecodedValue::Func(right)) => {
                let Object::Closure { identity: left, .. } = self.object(left)? else {
                    return Err(HeapError("Func handle refers to another object kind"));
                };
                let Object::Closure {
                    identity: right, ..
                } = self.object(right)?
                else {
                    return Err(HeapError("Func handle refers to another object kind"));
                };
                Ok(Arc::ptr_eq(left, right))
            }
            (DecodedValue::Dyn(left), DecodedValue::Dyn(right)) => {
                let (left, _, _) = self.dyn_parts(left)?;
                let (right, _, _) = self.dyn_parts(right)?;
                Ok(Arc::ptr_eq(left, right))
            }
            (DecodedValue::TypeSlot(_), _) | (_, DecodedValue::TypeSlot(_)) => {
                Err(HeapError("up-link escaped into equality"))
            }
            (DecodedValue::Int(left), DecodedValue::Int(right)) => Ok(left == right),
            (DecodedValue::Float(left), DecodedValue::Float(right)) => Ok(left == right),
            (
                left @ (DecodedValue::BuiltinAtom(_)
                | DecodedValue::InlineAtom(_)
                | DecodedValue::Atom(_)),
                right @ (DecodedValue::BuiltinAtom(_)
                | DecodedValue::InlineAtom(_)
                | DecodedValue::Atom(_)),
            ) => Ok(self.atom_text(left.into())? == self.atom_text(right.into())?),
            (
                left @ (DecodedValue::InlineString(_) | DecodedValue::ShortString(_)),
                right @ (DecodedValue::InlineString(_) | DecodedValue::ShortString(_)),
            ) => Ok(self.string_text(left.into())? == self.string_text(right.into())?),
            (DecodedValue::Bytes(left), DecodedValue::Bytes(right)) => {
                if left == right {
                    return Ok(true);
                }
                let Object::Bytes(left) = self.object(left)? else {
                    return Err(HeapError("Bytes handle refers to another object kind"));
                };
                let Object::Bytes(right) = self.object(right)? else {
                    return Err(HeapError("Bytes handle refers to another object kind"));
                };
                Ok(left == right)
            }
            (DecodedValue::Opaque(left), DecodedValue::Opaque(right)) => {
                if left == right {
                    return Ok(true);
                }
                let Object::Opaque(left) = self.object(left)? else {
                    return Err(HeapError("Opaque handle refers to another object kind"));
                };
                let Object::Opaque(right) = self.object(right)? else {
                    return Err(HeapError("Opaque handle refers to another object kind"));
                };
                Ok(left.logical_eq(right))
            }
            (DecodedValue::NativeType(left), DecodedValue::NativeType(right)) => Ok(left == right),
            (DecodedValue::DeclaredType(left), DecodedValue::DeclaredType(right)) => {
                let left_handle = left;
                let right_handle = right;
                let Object::DeclaredType { type_id: left, .. } = self.object(left_handle)? else {
                    return Err(HeapError(
                        "DeclaredType handle refers to another object kind",
                    ));
                };
                let Object::DeclaredType { type_id: right, .. } = self.object(right_handle)? else {
                    return Err(HeapError(
                        "DeclaredType handle refers to another object kind",
                    ));
                };
                Ok(left == right)
            }
            (DecodedValue::Array(left), DecodedValue::Array(right))
            | (DecodedValue::Tuple(left), DecodedValue::Tuple(right)) => {
                self.sequence_handles_equal(left, right, visited)
            }
            (DecodedValue::Tagged(left), DecodedValue::Tagged(right)) => {
                if left == right || !visited.insert((left, right)) {
                    return Ok(true);
                }
                let (left_tag, left_payload) = self.tagged(left)?;
                let (right_tag, right_payload) = self.tagged(right)?;
                Ok(self.values_equal_with(left_tag, right_tag, visited)?
                    && self.values_equal_with(left_payload, right_payload, visited)?)
            }
            (DecodedValue::Dict(left), DecodedValue::Dict(right)) => {
                self.dict_handles_equal(left, right, visited)
            }
            _ => Ok(false),
        }
    }

    fn sequence_handles_equal(
        &self,
        left: Handle,
        right: Handle,
        visited: &mut HashSet<(Handle, Handle)>,
    ) -> Result<bool, HeapError> {
        if left == right {
            return Ok(true);
        }
        if !visited.insert((left, right)) {
            return Ok(true);
        }
        let left_values = match self.object(left)? {
            Object::Array(values) | Object::Tuple(values) => values,
            _ => return Err(HeapError("sequence handle refers to another object kind")),
        };
        let right_values = match self.object(right)? {
            Object::Array(values) | Object::Tuple(values) => values,
            _ => return Err(HeapError("sequence handle refers to another object kind")),
        };
        self.value_slices_equal(left_values, right_values, visited)
    }

    fn dict_handles_equal(
        &self,
        left: Handle,
        right: Handle,
        visited: &mut HashSet<(Handle, Handle)>,
    ) -> Result<bool, HeapError> {
        if left == right {
            return Ok(true);
        }
        if !visited.insert((left, right)) {
            return Ok(true);
        }
        let Object::Dict {
            shape: left_shape,
            values: left_values,
        } = self.object(left)?
        else {
            return Err(HeapError("Dict handle refers to another object kind"));
        };
        let Object::Dict {
            shape: right_shape,
            values: right_values,
        } = self.object(right)?
        else {
            return Err(HeapError("Dict handle refers to another object kind"));
        };
        let left_fields = self.shape(*left_shape)?;
        let right_fields = self.shape(*right_shape)?;
        if left_fields.len() != right_fields.len() {
            return Ok(false);
        }
        for (left, right) in left_fields.iter().zip(right_fields) {
            if self.text(*left)? != self.text(*right)? {
                return Ok(false);
            }
        }
        self.value_slices_equal(left_values, right_values, visited)
    }

    fn value_slices_equal(
        &self,
        left: &[Val],
        right: &[Val],
        visited: &mut HashSet<(Handle, Handle)>,
    ) -> Result<bool, HeapError> {
        if left.len() != right.len() {
            return Ok(false);
        }
        for (left, right) in left.iter().zip(right) {
            if !self.values_equal_with(*left, *right, visited)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn copy_roots(
    target: &mut Heap,
    source: HeapView<'_>,
    roots: &[Val],
) -> Result<Vec<Val>, HeapError> {
    let mut pending = PendingCopy::new(target, &source);
    let roots = roots
        .iter()
        .map(|root| pending.copy_value(target, &source, *root))
        .collect::<Result<Vec<_>, _>>()?;
    pending.validate()?;
    pending.commit(target);
    Ok(roots)
}

pub(crate) fn instantiate_type_family(
    target: &mut Heap,
    background: Option<&Heap>,
    template: Val,
    arguments: &[Val],
    argument_descriptors: &[crate::types::TypeDescriptor],
) -> Result<(Val, usize), HeapError> {
    let (root, pending) = {
        let source = HeapView {
            current: target,
            background,
        };
        let (replacements, forced_objects) = bound_type_replacements(&source, template, arguments)?;
        let mut pending = PendingCopy::new_type_application(
            target,
            &source,
            replacements,
            forced_objects,
            arguments,
            argument_descriptors,
        );
        let root = pending.copy_value(target, &source, template)?;
        pending.validate()?;
        (root, pending)
    };
    let allocation_count = pending.objects.len();
    pending.commit(target);
    Ok((root, allocation_count))
}

fn bound_type_replacements(
    source: &HeapView<'_>,
    root: Val,
    arguments: &[Val],
) -> Result<(HashMap<Handle, Val>, HashSet<Handle>), HeapError> {
    let mut replacements = HashMap::new();
    let mut pending = vec![root];
    let mut visited = HashSet::new();
    let mut parents = HashMap::<Handle, Vec<Handle>>::new();
    let mut forced_objects = HashSet::new();
    while let Some(value) = pending.pop() {
        let Some(handle) = runtime_object_handle(value.value()) else {
            continue;
        };
        if !visited.insert(handle) {
            continue;
        }
        let object = source.object(handle)?;
        if let Object::DeclaredType { id, .. } | Object::SymbolicType { id, .. } = object
            && id
                .arguments()
                .iter()
                .any(crate::types::type_identity_contains_bound_parameter)
        {
            // Nominal identity retains phantom arguments even when the
            // structural body contains no corresponding Bound metadata.
            forced_objects.insert(handle);
        }
        if let Object::Dict { shape, values } = object {
            let fields = source.shape(*shape)?;
            let mut kind = None;
            let mut parameter = None;
            for (field, value) in fields.iter().zip(values.iter()) {
                match source.text(*field)? {
                    "kind" => kind = source.atom_text(*value)?,
                    "parameter" => {
                        if let DecodedValue::Int(index) = value.value() {
                            parameter = usize::try_from(index).ok();
                        }
                    }
                    _ => {}
                }
            }
            if kind.is_some_and(|kind| kind == "Bound") {
                let index = parameter.ok_or(HeapError("Bound metadata has no parameter index"))?;
                let argument = arguments
                    .get(index)
                    .copied()
                    .ok_or(HeapError("Bound metadata parameter is out of range"))?;
                replacements.insert(handle, argument);
                forced_objects.insert(handle);
                continue;
            }
        }
        let children = match object {
            Object::DeclaredType {
                body, sealed: true, ..
            }
            | Object::SymbolicType {
                body, sealed: true, ..
            } => vec![*body],
            Object::DeclaredType { sealed: false, .. }
            | Object::SymbolicType { sealed: false, .. } => {
                return Err(HeapError("type ref is not sealed"));
            }
            Object::Array(values) | Object::Tuple(values) => values.to_vec(),
            Object::Tagged { tag, payload } => vec![*tag, *payload],
            Object::Dict { values, .. } => values.to_vec(),
            Object::Module { exports } => exports.values.to_vec(),
            Object::Closure { upvalues, .. } => upvalues.to_vec(),
            Object::Dyn {
                descriptor, value, ..
            } => vec![*descriptor, *value],
            Object::TypeSlot { value } => {
                vec![value.ok_or(HeapError("uninitialized type metadata up-link"))?]
            }
            Object::ByteCodeProto { values, .. } => values.to_vec(),
            Object::OpenFunc => return Err(HeapError("function ref is not sealed")),
            Object::Reserved | Object::Bytes(_) | Object::Opaque(_) => Vec::new(),
        };
        for child in children {
            if let Some(child_handle) = runtime_object_handle(child.value()) {
                parents.entry(child_handle).or_default().push(handle);
            }
            pending.push(child);
        }
    }
    let mut affected = forced_objects.iter().copied().collect::<Vec<_>>();
    while let Some(child) = affected.pop() {
        for parent in parents.get(&child).into_iter().flatten() {
            if forced_objects.insert(*parent) {
                affected.push(*parent);
            }
        }
    }
    Ok((replacements, forced_objects))
}

fn runtime_object_handle(value: DecodedValue) -> Option<Handle> {
    match value {
        DecodedValue::NativeType(_) => None,
        DecodedValue::Bytes(handle)
        | DecodedValue::DeclaredType(handle)
        | DecodedValue::SymbolicType(handle)
        | DecodedValue::Opaque(handle)
        | DecodedValue::Array(handle)
        | DecodedValue::Tuple(handle)
        | DecodedValue::Tagged(handle)
        | DecodedValue::Dict(handle)
        | DecodedValue::Module(handle)
        | DecodedValue::Func(handle)
        | DecodedValue::Dyn(handle)
        | DecodedValue::TypeSlot(handle) => Some(handle),
        DecodedValue::Failed(_)
        | DecodedValue::Int(_)
        | DecodedValue::Float(_)
        | DecodedValue::BuiltinAtom(_)
        | DecodedValue::InlineAtom(_)
        | DecodedValue::Atom(_)
        | DecodedValue::InlineString(_)
        | DecodedValue::ShortString(_)
        | DecodedValue::FuncRef(_) => None,
    }
}

pub(crate) fn relocate_work_roots(
    target: &mut Heap,
    main: &Heap,
    source: &Heap,
    roots: &[Val],
) -> Result<Vec<Val>, HeapError> {
    if target.storage != Storage::Work
        || source.storage != Storage::Work
        || main.storage != Storage::Main
    {
        return Err(HeapError(
            "work relocation requires two Work worlds and one Main world",
        ));
    }
    copy_roots(
        target,
        HeapView {
            current: source,
            background: Some(main),
        },
        roots,
    )
}

pub(crate) fn publish_root(
    target: &mut Heap,
    current: &Heap,
    root: Val,
) -> Result<PersistentValue, HeapError> {
    if target.storage != Storage::Main || current.storage != Storage::Work {
        return Err(HeapError(
            "publication requires a Work world and Main world",
        ));
    }
    if (HeapView {
        current,
        background: Some(target),
    })
    .first_data_failure(root)?
    .is_some()
    {
        return Err(HeapError(
            "failed evaluation node cannot cross a Host publication boundary",
        ));
    }
    let roots = copy_roots(
        target,
        HeapView {
            current,
            background: None,
        },
        &[root],
    )?;
    Ok(PersistentValue(roots[0]))
}

pub(crate) fn publish_module_root(
    target: &mut Heap,
    current: &Heap,
    root: Val,
) -> Result<PersistentValue, HeapError> {
    if target.storage != Storage::Main || current.storage != Storage::Work {
        return Err(HeapError(
            "module publication requires a Work world and Main world",
        ));
    }
    let mut functions = current
        .functions
        .iter()
        .filter_map(|(id, value)| value.map(|value| (*id, value)))
        .collect::<Vec<_>>();
    functions.sort_by_key(|(id, _)| *id);
    for (id, _) in &functions {
        match target.functions.get(id) {
            Some(None) => {}
            Some(Some(_)) => return Err(HeapError("static function slot is already sealed")),
            None => return Err(HeapError("unknown static function slot")),
        }
    }
    let mut roots = Vec::with_capacity(functions.len() + 1);
    roots.push(root);
    roots.extend(functions.iter().map(|(_, value)| *value));
    let copied = copy_roots(
        target,
        HeapView {
            current,
            background: None,
        },
        &roots,
    )?;
    for ((id, _), value) in functions.into_iter().zip(copied.iter().skip(1).copied()) {
        let slot = target
            .functions
            .get_mut(&id)
            .expect("static function slots were validated before copying");
        debug_assert!(slot.is_none());
        *slot = Some(value);
    }
    Ok(PersistentValue(copied[0]))
}

/// Converts the private raw graph produced by a data frontend into the public
/// recursively tagged `std/value.Value` graph. Variant wrappers retain the raw
/// node location; provenance paths therefore continue to address semantic
/// array indices and object keys rather than implementation wrappers.
pub(crate) fn wrap_semantic_value(
    current: &mut Heap,
    background: Option<&Heap>,
    raw: Val,
    owner: Val,
) -> Result<Val, HeapError> {
    enum RawNode {
        Unit(BuiltinAtom),
        Scalar(&'static str, Val),
        Array(Vec<Val>),
        Object(Vec<(String, Val)>),
        Temporal(String, Val),
    }

    let view = HeapView {
        current,
        background,
    };
    let type_id = view.declared_type_id(owner)?;
    let node = match raw.value() {
        DecodedValue::BuiltinAtom(
            atom @ (BuiltinAtom::None | BuiltinAtom::True | BuiltinAtom::False),
        ) => RawNode::Unit(atom),
        DecodedValue::Int(_) => RawNode::Scalar("Int", raw.without_type_id()),
        DecodedValue::Float(value) if value.is_finite() => {
            RawNode::Scalar("Float", raw.without_type_id())
        }
        DecodedValue::Float(_) => {
            return Err(HeapError(
                "semantic Value cannot contain a non-finite Float",
            ));
        }
        DecodedValue::InlineString(_) | DecodedValue::ShortString(_) => {
            RawNode::Scalar("String", raw.without_type_id())
        }
        DecodedValue::Bytes(_) => RawNode::Scalar("Bytes", raw.without_type_id()),
        DecodedValue::Array(handle) => RawNode::Array(view.sequence(handle, false)?.to_vec()),
        DecodedValue::Dict(handle) => {
            let (fields, values) = view.dict_parts(handle)?;
            let fields = fields
                .iter()
                .map(|field| view.text(*field).map(str::to_owned))
                .collect::<Result<Vec<_>, _>>()?;
            RawNode::Object(fields.into_iter().zip(values.iter().copied()).collect())
        }
        DecodedValue::Tagged(handle) => {
            let (tag, payload) = view.tagged(handle)?;
            let tag = view
                .atom_text(tag)?
                .ok_or(HeapError("semantic temporal tag is not an Atom"))?
                .as_str()
                .to_owned();
            if !matches!(
                tag.as_str(),
                "LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime"
            ) {
                return Err(HeapError::owned(format!(
                    "raw data graph contains unsupported tag {tag:?}"
                )));
            }
            if view.string_text(payload)?.is_none() {
                return Err(HeapError("semantic temporal payload is not a String"));
            }
            RawNode::Temporal(tag, payload.without_type_id())
        }
        DecodedValue::NativeType(_)
        | DecodedValue::DeclaredType(_)
        | DecodedValue::SymbolicType(_)
        | DecodedValue::TypeSlot(_) => {
            return Err(HeapError("semantic Value cannot encode Type"));
        }
        _ => {
            return Err(HeapError::owned(format!(
                "raw data graph contains unsupported {:?}",
                raw.value()
            )));
        }
    };

    let loc = raw.loc();
    let value = match node {
        RawNode::Unit(atom) => Val::new(DecodedValue::BuiltinAtom(atom), loc),
        RawNode::Scalar(tag, payload) => {
            let tag = Val::new(current.atom(background, tag), loc);
            Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged { tag, payload })),
                loc,
            )
        }
        RawNode::Array(items) => {
            let items = items
                .into_iter()
                .map(|item| wrap_semantic_value(current, background, item, owner))
                .collect::<Result<Box<[_]>, _>>()?;
            let payload = Val::new(
                DecodedValue::Array(current.allocate(Object::Array(items))),
                loc,
            );
            let tag = Val::new(current.atom(background, "Array"), loc);
            Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged { tag, payload })),
                loc,
            )
        }
        RawNode::Object(fields) => {
            let mut fields = fields
                .into_iter()
                .map(|(name, value)| {
                    wrap_semantic_value(current, background, value, owner)
                        .map(|value| (name, value))
                })
                .collect::<Result<Vec<_>, _>>()?;
            fields.sort_by(|left, right| left.0.cmp(&right.0));
            let (names, values): (Vec<_>, Vec<_>) = fields
                .into_iter()
                .map(|(name, value)| (current.intern(&name), value))
                .unzip();
            let shape = current.intern_shape(names);
            let payload = Val::new(
                DecodedValue::Dict(current.allocate(Object::Dict {
                    shape,
                    values: values.into(),
                })),
                loc,
            );
            let tag = Val::new(current.atom(background, "Object"), loc);
            Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged { tag, payload })),
                loc,
            )
        }
        RawNode::Temporal(tag, payload) => {
            let tag = Val::new(current.atom(background, &tag), loc);
            Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged { tag, payload })),
                loc,
            )
        }
    };
    Ok(value.with_type_id(type_id))
}

pub(crate) fn semantic_value_wrapper_bytes(
    current: &Heap,
    background: Option<&Heap>,
    raw: Val,
) -> Result<u64, HeapError> {
    fn add(left: u64, right: u64) -> Result<u64, HeapError> {
        left.checked_add(right)
            .ok_or(HeapError("semantic Value size overflowed"))
    }

    fn visit(
        view: &HeapView<'_>,
        raw: Val,
        active: &mut HashSet<Handle>,
    ) -> Result<u64, HeapError> {
        let tagged_bytes = (std::mem::size_of::<Val>() as u64)
            .checked_mul(2)
            .ok_or(HeapError("semantic Value size overflowed"))?;
        match raw.value() {
            DecodedValue::BuiltinAtom(
                BuiltinAtom::None | BuiltinAtom::True | BuiltinAtom::False,
            ) => Ok(0),
            DecodedValue::Int(_)
            | DecodedValue::InlineString(_)
            | DecodedValue::ShortString(_)
            | DecodedValue::Bytes(_) => Ok(tagged_bytes),
            DecodedValue::Float(value) if value.is_finite() => Ok(tagged_bytes),
            DecodedValue::Float(_) => Err(HeapError(
                "semantic Value cannot contain a non-finite Float",
            )),
            DecodedValue::Array(handle) => {
                if !active.insert(handle) {
                    return Err(HeapError("semantic Value cannot contain a cycle"));
                }
                let items = view.sequence(handle, false)?;
                let own = (std::mem::size_of::<Val>() as u64)
                    .checked_mul(items.len() as u64)
                    .ok_or(HeapError("semantic Value size overflowed"))?;
                let mut bytes = add(own, tagged_bytes)?;
                for item in items {
                    bytes = add(bytes, visit(view, *item, active)?)?;
                }
                active.remove(&handle);
                Ok(bytes)
            }
            DecodedValue::Dict(handle) => {
                if !active.insert(handle) {
                    return Err(HeapError("semantic Value cannot contain a cycle"));
                }
                let (_, values) = view.dict_parts(handle)?;
                let own = (std::mem::size_of::<Val>() as u64)
                    .checked_mul(values.len() as u64)
                    .ok_or(HeapError("semantic Value size overflowed"))?;
                let mut bytes = add(own, tagged_bytes)?;
                for value in values {
                    bytes = add(bytes, visit(view, *value, active)?)?;
                }
                active.remove(&handle);
                Ok(bytes)
            }
            DecodedValue::Tagged(handle) => {
                if !active.insert(handle) {
                    return Err(HeapError("semantic Value cannot contain a cycle"));
                }
                let (tag, payload) = view.tagged(handle)?;
                let tag = view
                    .atom_text(tag)?
                    .ok_or(HeapError("semantic temporal tag is not an Atom"))?;
                if !matches!(
                    tag.as_str(),
                    "LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime"
                ) || view.string_text(payload)?.is_none()
                {
                    return Err(HeapError(
                        "raw data graph contains unsupported tagged value",
                    ));
                }
                active.remove(&handle);
                Ok(tagged_bytes)
            }
            DecodedValue::NativeType(_)
            | DecodedValue::DeclaredType(_)
            | DecodedValue::SymbolicType(_)
            | DecodedValue::TypeSlot(_) => Err(HeapError("semantic Value cannot encode Type")),
            _ => Err(HeapError::owned(format!(
                "raw data graph contains unsupported {:?}",
                raw.value()
            ))),
        }
    }

    visit(
        &HeapView {
            current,
            background,
        },
        raw,
        &mut HashSet::new(),
    )
}

pub(crate) fn semantic_value_unwrap_bytes(
    current: &Heap,
    background: Option<&Heap>,
    value: Val,
    owner: Val,
) -> Result<u64, HeapError> {
    fn add(left: u64, right: u64) -> Result<u64, HeapError> {
        left.checked_add(right)
            .ok_or(HeapError("semantic Value size overflowed"))
    }

    fn visit(
        view: &HeapView<'_>,
        value: Val,
        expected: crate::TypeId,
        active: &mut HashSet<Handle>,
    ) -> Result<u64, HeapError> {
        if value.type_id() != Some(expected) {
            return Err(HeapError(
                "data value does not have the canonical std/value.Value identity",
            ));
        }
        match value.value() {
            DecodedValue::BuiltinAtom(
                BuiltinAtom::None | BuiltinAtom::True | BuiltinAtom::False,
            ) => Ok(0),
            DecodedValue::Tagged(handle) => {
                if !active.insert(handle) {
                    return Err(HeapError("std/value.Value cannot contain a cycle"));
                }
                let (tag, payload) = view.tagged(handle)?;
                let tag = view
                    .atom_text(tag)?
                    .ok_or(HeapError("Value variant tag is not an Atom"))?;
                let bytes = match tag.as_str() {
                    "Int" if matches!(payload.value(), DecodedValue::Int(_)) => 0,
                    "Float" if matches!(payload.value(), DecodedValue::Float(value) if value.is_finite()) => {
                        0
                    }
                    "String" if view.string_text(payload)?.is_some() => 0,
                    "Bytes" if matches!(payload.value(), DecodedValue::Bytes(_)) => 0,
                    "Array" => {
                        let DecodedValue::Array(payload_handle) = payload.value() else {
                            return Err(HeapError("Value.Array payload is not an Array"));
                        };
                        let items = view.sequence(payload_handle, false)?;
                        let mut bytes = (std::mem::size_of::<Val>() as u64)
                            .checked_mul(items.len() as u64)
                            .ok_or(HeapError("semantic Value size overflowed"))?;
                        for item in items {
                            bytes = add(bytes, visit(view, *item, expected, active)?)?;
                        }
                        bytes
                    }
                    "Object" => {
                        let DecodedValue::Dict(payload_handle) = payload.value() else {
                            return Err(HeapError("Value.Object payload is not a Dict"));
                        };
                        let (_, values) = view.dict_parts(payload_handle)?;
                        let mut bytes = (std::mem::size_of::<Val>() as u64)
                            .checked_mul(values.len() as u64)
                            .ok_or(HeapError("semantic Value size overflowed"))?;
                        for value in values {
                            bytes = add(bytes, visit(view, *value, expected, active)?)?;
                        }
                        bytes
                    }
                    "LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime"
                        if view.string_text(payload)?.is_some() =>
                    {
                        std::mem::size_of::<Val>() as u64
                    }
                    _ => {
                        return Err(HeapError::owned(format!(
                            "invalid std/value.Value variant {:?}",
                            tag.as_str()
                        )));
                    }
                };
                active.remove(&handle);
                Ok(bytes)
            }
            _ => Err(HeapError(
                "std/value.Value has an invalid runtime representation",
            )),
        }
    }

    let view = HeapView {
        current,
        background,
    };
    let expected = view.declared_type_id(owner)?;
    visit(&view, value, expected, &mut HashSet::new())
}

/// Removes the public `Value` variants into the private raw graph consumed by
/// the existing schema transformer and format writers.
pub(crate) fn unwrap_semantic_value(
    current: &mut Heap,
    background: Option<&Heap>,
    value: Val,
    owner: Val,
) -> Result<Val, HeapError> {
    enum ValueNode {
        Unit(BuiltinAtom),
        Scalar(Val),
        Array(Vec<Val>),
        Object(Vec<(String, Val)>),
        Temporal(String, Val),
    }

    let view = HeapView {
        current,
        background,
    };
    let expected = view.declared_type_id(owner)?;
    if value.type_id() != Some(expected) {
        return Err(HeapError(
            "data value does not have the canonical std/value.Value identity",
        ));
    }
    let node = match value.value() {
        DecodedValue::BuiltinAtom(
            atom @ (BuiltinAtom::None | BuiltinAtom::True | BuiltinAtom::False),
        ) => ValueNode::Unit(atom),
        DecodedValue::Tagged(handle) => {
            let (tag, payload) = view.tagged(handle)?;
            let tag = view
                .atom_text(tag)?
                .ok_or(HeapError("Value variant tag is not an Atom"))?
                .as_str()
                .to_owned();
            match tag.as_str() {
                "Int" if matches!(payload.value(), DecodedValue::Int(_)) => {
                    ValueNode::Scalar(payload)
                }
                "Float" if matches!(payload.value(), DecodedValue::Float(value) if value.is_finite()) => {
                    ValueNode::Scalar(payload)
                }
                "String" if view.string_text(payload)?.is_some() => ValueNode::Scalar(payload),
                "Bytes" if matches!(payload.value(), DecodedValue::Bytes(_)) => {
                    ValueNode::Scalar(payload)
                }
                "Array" => {
                    let DecodedValue::Array(handle) = payload.value() else {
                        return Err(HeapError("Value.Array payload is not an Array"));
                    };
                    ValueNode::Array(view.sequence(handle, false)?.to_vec())
                }
                "Object" => {
                    let DecodedValue::Dict(handle) = payload.value() else {
                        return Err(HeapError("Value.Object payload is not a Dict"));
                    };
                    let (fields, values) = view.dict_parts(handle)?;
                    let fields = fields
                        .iter()
                        .map(|field| view.text(*field).map(str::to_owned))
                        .collect::<Result<Vec<_>, _>>()?;
                    ValueNode::Object(fields.into_iter().zip(values.iter().copied()).collect())
                }
                "LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime"
                    if view.string_text(payload)?.is_some() =>
                {
                    ValueNode::Temporal(tag, payload)
                }
                _ => {
                    return Err(HeapError::owned(format!(
                        "invalid std/value.Value variant {tag:?}"
                    )));
                }
            }
        }
        _ => {
            return Err(HeapError(
                "std/value.Value has an invalid runtime representation",
            ));
        }
    };

    let loc = value.loc();
    match node {
        ValueNode::Unit(atom) => Ok(Val::new(DecodedValue::BuiltinAtom(atom), loc)),
        ValueNode::Scalar(payload) => Ok(payload.without_type_id().with_loc(payload.loc().or(loc))),
        ValueNode::Array(items) => {
            let items = items
                .into_iter()
                .map(|item| unwrap_semantic_value(current, background, item, owner))
                .collect::<Result<Box<[_]>, _>>()?;
            Ok(Val::new(
                DecodedValue::Array(current.allocate(Object::Array(items))),
                loc,
            ))
        }
        ValueNode::Object(fields) => {
            let mut fields = fields
                .into_iter()
                .map(|(name, value)| {
                    unwrap_semantic_value(current, background, value, owner)
                        .map(|value| (name, value))
                })
                .collect::<Result<Vec<_>, _>>()?;
            fields.sort_by(|left, right| left.0.cmp(&right.0));
            let (names, values): (Vec<_>, Vec<_>) = fields
                .into_iter()
                .map(|(name, value)| (current.intern(&name), value))
                .unzip();
            let shape = current.intern_shape(names);
            Ok(Val::new(
                DecodedValue::Dict(current.allocate(Object::Dict {
                    shape,
                    values: values.into(),
                })),
                loc,
            ))
        }
        ValueNode::Temporal(tag, payload) => {
            let field = current.intern(&tag);
            let shape = current.intern_shape(vec![field]);
            Ok(Val::new(
                DecodedValue::Dict(current.allocate(Object::Dict {
                    shape,
                    values: Box::new([payload]),
                })),
                loc,
            ))
        }
    }
}

struct PendingCopy {
    target_storage: Storage,
    source_storage: Storage,
    background_storage: Option<Storage>,
    object_base: u32,
    objects: Vec<Object>,
    text_base: u32,
    text: TextTable,
    shape_base: u32,
    shapes: Vec<Box<[InternId]>>,
    objects_forwarded: HashMap<u32, u32>,
    text_forwarded: HashMap<InternId, InternId>,
    shapes_forwarded: HashMap<ShapeId, ShapeId>,
    native_types: HashMap<crate::value::NativeTypeId, crate::NativeType>,
    value_replacements: HashMap<Handle, Val>,
    forced_objects: HashSet<Handle>,
    type_argument_values: Option<Arc<[Val]>>,
    type_arguments: Option<Arc<[crate::types::TypeDescriptor]>>,
}

impl PendingCopy {
    fn new(target: &Heap, source: &HeapView<'_>) -> Self {
        Self {
            target_storage: target.storage,
            source_storage: source.current.storage,
            background_storage: source.background.map(|heap| heap.storage),
            object_base: target.objects.len() as u32,
            objects: Vec::new(),
            text_base: target.text.values.len() as u32,
            text: TextTable::default(),
            shape_base: target.shapes.len() as u32,
            shapes: Vec::new(),
            objects_forwarded: HashMap::new(),
            text_forwarded: HashMap::new(),
            shapes_forwarded: HashMap::new(),
            native_types: HashMap::new(),
            value_replacements: HashMap::new(),
            forced_objects: HashSet::new(),
            type_argument_values: None,
            type_arguments: None,
        }
    }

    fn new_type_application(
        target: &Heap,
        source: &HeapView<'_>,
        value_replacements: HashMap<Handle, Val>,
        forced_objects: HashSet<Handle>,
        type_argument_values: &[Val],
        type_arguments: &[crate::types::TypeDescriptor],
    ) -> Self {
        Self {
            value_replacements,
            forced_objects,
            type_argument_values: Some(type_argument_values.into()),
            type_arguments: Some(type_arguments.into()),
            ..Self::new(target, source)
        }
    }

    fn copy_value(
        &mut self,
        target: &Heap,
        source: &HeapView<'_>,
        value: Val,
    ) -> Result<Val, HeapError> {
        if let Some(handle) = runtime_object_handle(value.value())
            && let Some(replacement) = self.value_replacements.get(&handle)
        {
            return Ok(if replacement.loc().is_some() {
                *replacement
            } else {
                replacement.with_loc(value.loc())
            });
        }
        let copied = match value.value() {
            // Failure ids belong to the Main world's stable failure arena.
            // Work executions inherit that arena as a prefix and append new
            // roots, so the identity does not need relocation during copy.
            DecodedValue::Failed(id) => DecodedValue::Failed(id),
            DecodedValue::Int(_)
            | DecodedValue::BuiltinAtom(_)
            | DecodedValue::InlineAtom(_)
            | DecodedValue::InlineString(_)
            | DecodedValue::FuncRef(_) => value.value(),
            DecodedValue::Float(float) if float.is_finite() => value.value(),
            DecodedValue::Float(_) => return Err(HeapError("Telora Float must be finite")),
            DecodedValue::Atom(id) => DecodedValue::Atom(self.copy_text(target, source, id)?),
            DecodedValue::ShortString(id) => {
                DecodedValue::ShortString(self.copy_text(target, source, id)?)
            }
            DecodedValue::Bytes(handle) => {
                DecodedValue::Bytes(self.copy_object(target, source, handle)?)
            }
            DecodedValue::Opaque(handle) => {
                DecodedValue::Opaque(self.copy_object(target, source, handle)?)
            }
            DecodedValue::NativeType(id) => {
                self.copy_native_type(target, source, id)?;
                DecodedValue::NativeType(id)
            }
            DecodedValue::DeclaredType(handle) => {
                DecodedValue::DeclaredType(self.copy_object(target, source, handle)?)
            }
            DecodedValue::SymbolicType(handle) => {
                let copied = self.copy_object(target, source, handle)?;
                if copied == handle {
                    return Ok(value.with_value(DecodedValue::SymbolicType(copied)));
                }
                let Object::SymbolicType { id, .. } = source.object(handle)? else {
                    return Err(HeapError(
                        "SymbolicType handle refers to another object kind",
                    ));
                };
                let id = self.type_arguments.as_ref().map_or_else(
                    || id.clone(),
                    |arguments| crate::types::apply_declared_type_arguments(id, arguments),
                );
                let remains_symbolic = id
                    .arguments()
                    .iter()
                    .any(crate::types::type_identity_is_symbolic);
                if remains_symbolic {
                    DecodedValue::SymbolicType(copied)
                } else {
                    DecodedValue::DeclaredType(copied)
                }
            }
            DecodedValue::Array(handle) => {
                DecodedValue::Array(self.copy_object(target, source, handle)?)
            }
            DecodedValue::Tuple(handle) => {
                DecodedValue::Tuple(self.copy_object(target, source, handle)?)
            }
            DecodedValue::Tagged(handle) => {
                DecodedValue::Tagged(self.copy_object(target, source, handle)?)
            }
            DecodedValue::Dict(handle) => {
                DecodedValue::Dict(self.copy_object(target, source, handle)?)
            }
            DecodedValue::Module(handle) => {
                DecodedValue::Module(self.copy_object(target, source, handle)?)
            }
            DecodedValue::Func(handle) => {
                DecodedValue::Func(self.copy_object(target, source, handle)?)
            }
            DecodedValue::Dyn(handle) => {
                DecodedValue::Dyn(self.copy_object(target, source, handle)?)
            }
            DecodedValue::TypeSlot(handle) => {
                DecodedValue::TypeSlot(self.copy_object(target, source, handle)?)
            }
        };
        let mut copied = value.with_value(copied).without_type_id();
        if let Some(type_id) = value.type_id() {
            if self.type_arguments.is_none() && target.declared_types.contains_key(&type_id) {
                return Ok(copied.with_type_id(type_id));
            }
            let owner = source
                .type_witness(value)?
                .expect("value with a TypeId has registered metadata");
            let DecodedValue::DeclaredType(owner_handle) = owner.value() else {
                unreachable!("type metadata is a declared Type")
            };
            let Object::DeclaredType { id, .. } = source.object(owner_handle)? else {
                unreachable!("type metadata is a declared Type")
            };
            let copied_id = self.canonical_declared_type_id(target, id)?;
            self.copy_object(target, source, owner_handle)?;
            copied = copied.with_type_id(copied_id);
        }
        Ok(copied)
    }

    fn canonical_declared_type_id(
        &self,
        target: &Heap,
        id: &crate::value::DeclaredTypeId,
    ) -> Result<crate::TypeId, HeapError> {
        let id = self.type_arguments.as_ref().map_or_else(
            || id.clone(),
            |arguments| crate::types::apply_declared_type_arguments(id, arguments),
        );
        target.canonical_declared_type_id(&id)
    }

    fn copy_object(
        &mut self,
        target: &Heap,
        source: &HeapView<'_>,
        handle: Handle,
    ) -> Result<Handle, HeapError> {
        if handle.storage != self.source_storage && !self.forced_objects.contains(&handle) {
            if handle.storage == self.target_storage {
                target.object(handle)?;
            } else {
                source.object(handle)?;
            }
            return Ok(handle);
        }
        if let Some(forwarded) = self.objects_forwarded.get(&handle.slot) {
            return Ok(Handle {
                storage: self.target_storage,
                slot: *forwarded,
            });
        }
        let object = source.object(handle)?;
        if matches!(object, Object::Reserved) {
            return Err(HeapError("cannot copy an uninitialized object"));
        }
        let copied = Handle {
            storage: self.target_storage,
            slot: self.object_base + self.objects.len() as u32,
        };
        self.objects_forwarded.insert(handle.slot, copied.slot);
        self.objects.push(Object::Reserved);
        let object = self.copy_object_data(target, source, object)?;
        self.objects[(copied.slot - self.object_base) as usize] = object;
        Ok(copied)
    }

    fn copy_object_data(
        &mut self,
        target: &Heap,
        source: &HeapView<'_>,
        object: &Object,
    ) -> Result<Object, HeapError> {
        let copy_values = |this: &mut Self, values: &[Val]| {
            values
                .iter()
                .map(|value| this.copy_value(target, source, *value))
                .collect::<Result<Box<[_]>, _>>()
        };
        Ok(match object {
            Object::Reserved | Object::OpenFunc => {
                return Err(HeapError("cannot copy an uninitialized object"));
            }
            Object::Bytes(value) => Object::Bytes(value.clone()),
            Object::Opaque(value) => Object::Opaque(value.clone()),
            Object::DeclaredType {
                id,
                name,
                body,
                sealed,
                application_arguments,
                ..
            } => {
                if !sealed {
                    return Err(HeapError("cannot copy an unsealed type ref"));
                }
                let type_argument_values = self.type_argument_values.clone();
                let application_arguments = if let Some(arguments) = type_argument_values {
                    Some(arguments.as_ref().into())
                } else if let Some(arguments) = application_arguments {
                    Some(
                        arguments
                            .iter()
                            .map(|argument| self.copy_value(target, source, *argument))
                            .collect::<Result<Box<[_]>, _>>()?,
                    )
                } else {
                    None
                };
                let id = self.type_arguments.as_ref().map_or_else(
                    || id.clone(),
                    |arguments| crate::types::apply_declared_type_arguments(id, arguments),
                );
                let type_id = target.canonical_declared_type_id(&id)?;
                Object::DeclaredType {
                    type_id,
                    id,
                    name: Arc::clone(name),
                    body: self.copy_value(target, source, *body)?,
                    sealed: true,
                    application_arguments,
                }
            }
            Object::SymbolicType {
                id,
                name,
                body,
                sealed,
                application_arguments,
            } => {
                if !sealed {
                    return Err(HeapError("cannot copy an unsealed symbolic type ref"));
                }
                let type_argument_values = self.type_argument_values.clone();
                let application_arguments = if let Some(arguments) = type_argument_values {
                    Some(arguments.as_ref().into())
                } else if let Some(arguments) = application_arguments {
                    Some(
                        arguments
                            .iter()
                            .map(|argument| self.copy_value(target, source, *argument))
                            .collect::<Result<Box<[_]>, _>>()?,
                    )
                } else {
                    None
                };
                let id = self.type_arguments.as_ref().map_or_else(
                    || id.clone(),
                    |arguments| crate::types::apply_declared_type_arguments(id, arguments),
                );
                let body = self.copy_value(target, source, *body)?;
                if id
                    .arguments()
                    .iter()
                    .any(crate::types::type_identity_is_symbolic)
                {
                    Object::SymbolicType {
                        id,
                        name: Arc::clone(name),
                        body,
                        sealed: true,
                        application_arguments,
                    }
                } else {
                    Object::DeclaredType {
                        type_id: target.canonical_declared_type_id(&id)?,
                        id,
                        name: Arc::clone(name),
                        body,
                        sealed: true,
                        application_arguments,
                    }
                }
            }
            Object::Array(values) => Object::Array(copy_values(self, values)?),
            Object::Tuple(values) => Object::Tuple(copy_values(self, values)?),
            Object::Tagged { tag, payload } => Object::Tagged {
                tag: self.copy_value(target, source, *tag)?,
                payload: self.copy_value(target, source, *payload)?,
            },
            Object::Dict { shape, values } => Object::Dict {
                shape: self.copy_shape(target, source, *shape)?,
                values: copy_values(self, values)?,
            },
            Object::Module { exports } => Object::Module {
                exports: ExportTable {
                    shape: self.copy_shape(target, source, exports.shape)?,
                    values: copy_values(self, &exports.values)?,
                },
            },
            Object::Closure {
                identity,
                prototype,
                upvalues,
            } => Object::Closure {
                identity: Arc::clone(identity),
                prototype: self.copy_prototype(target, source, prototype)?,
                upvalues: copy_values(self, upvalues)?,
            },
            Object::Dyn {
                identity,
                descriptor,
                value,
                scheme,
                origin,
            } => Object::Dyn {
                identity: Arc::clone(identity),
                descriptor: self.copy_value(target, source, *descriptor)?,
                value: self.copy_value(target, source, *value)?,
                scheme: scheme.clone(),
                origin: origin.clone(),
            },
            Object::TypeSlot { value } => Object::TypeSlot {
                value: Some(self.copy_value(
                    target,
                    source,
                    value.ok_or(HeapError("cannot publish an uninitialized up-link"))?,
                )?),
            },
            Object::ByteCodeProto {
                code,
                values,
                text,
                prototypes,
            } => Object::ByteCodeProto {
                code: Arc::clone(code),
                values: copy_values(self, values)?,
                text: text
                    .iter()
                    .map(|id| self.copy_text(target, source, *id))
                    .collect::<Result<Box<[_]>, _>>()?,
                prototypes: prototypes
                    .iter()
                    .map(|prototype| self.copy_prototype(target, source, prototype))
                    .collect::<Result<Box<[_]>, _>>()?,
            },
        })
    }

    fn copy_prototype(
        &mut self,
        target: &Heap,
        source: &HeapView<'_>,
        prototype: &RuntimePrototype,
    ) -> Result<RuntimePrototype, HeapError> {
        Ok(match prototype {
            RuntimePrototype::Bytecode(handle) => {
                RuntimePrototype::Bytecode(self.copy_object(target, source, *handle)?)
            }
            RuntimePrototype::Native(function) => RuntimePrototype::Native(*function),
        })
    }

    fn copy_text(
        &mut self,
        target: &Heap,
        source: &HeapView<'_>,
        id: InternId,
    ) -> Result<InternId, HeapError> {
        if id.storage != self.source_storage {
            if id.storage == self.target_storage {
                target.resolve_text(id)?;
            } else {
                source.text(id)?;
            }
            return Ok(id);
        }
        if let Some(forwarded) = self.text_forwarded.get(&id) {
            return Ok(*forwarded);
        }
        let text = source.text(id)?;
        let copied = if let Some(id) = target.find_text(text) {
            id
        } else if let Some(slot) = self.text.find(text) {
            InternId {
                storage: self.target_storage,
                slot: self.text_base + slot,
            }
        } else {
            let local_slot = self.text.insert(text);
            InternId {
                storage: self.target_storage,
                slot: self.text_base + local_slot,
            }
        };
        self.text_forwarded.insert(id, copied);
        Ok(copied)
    }

    fn copy_native_type(
        &mut self,
        target: &Heap,
        source: &HeapView<'_>,
        id: crate::value::NativeTypeId,
    ) -> Result<(), HeapError> {
        if target.native_types.contains_key(&id) || self.native_types.contains_key(&id) {
            return Ok(());
        }
        self.native_types
            .insert(id, source.native_type(id)?.clone());
        Ok(())
    }

    fn copy_shape(
        &mut self,
        target: &Heap,
        source: &HeapView<'_>,
        id: ShapeId,
    ) -> Result<ShapeId, HeapError> {
        if id.storage != self.source_storage {
            if id.storage == self.target_storage {
                target.shape(id)?;
            } else {
                source.shape(id)?;
            }
            return Ok(id);
        }
        if let Some(forwarded) = self.shapes_forwarded.get(&id) {
            return Ok(*forwarded);
        }
        let fields = source
            .shape(id)?
            .iter()
            .map(|field| self.copy_text(target, source, *field))
            .collect::<Result<Vec<_>, _>>()?;
        let copied = if let Some(slot) = target.shape_slots.get(&fields) {
            ShapeId {
                storage: self.target_storage,
                slot: *slot,
            }
        } else if let Some(index) = self.shapes.iter().position(|shape| **shape == fields) {
            ShapeId {
                storage: self.target_storage,
                slot: self.shape_base + index as u32,
            }
        } else {
            let copied = ShapeId {
                storage: self.target_storage,
                slot: self.shape_base + self.shapes.len() as u32,
            };
            self.shapes.push(fields.into());
            copied
        };
        self.shapes_forwarded.insert(id, copied);
        Ok(copied)
    }

    fn validate(&self) -> Result<(), HeapError> {
        if self.objects.iter().any(|object| {
            object_contains_disallowed(object, self.target_storage, self.background_storage)
        }) {
            return Err(HeapError(
                "copied object graph is not target-self-contained",
            ));
        }
        Ok(())
    }

    fn commit(self, target: &mut Heap) {
        let declared_types = self
            .objects
            .iter()
            .enumerate()
            .filter_map(|(index, object)| match object {
                Object::DeclaredType { type_id, .. } => Some((
                    *type_id,
                    Val::unknown(DecodedValue::DeclaredType(Handle {
                        storage: self.target_storage,
                        slot: self.object_base + index as u32,
                    })),
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        target.objects.extend(self.objects);
        target.native_types.extend(self.native_types);
        for (type_id, value) in declared_types {
            target.declared_types.entry(type_id).or_insert(value);
        }
        for value in self.text.values {
            target.text.insert(&value);
        }
        for shape in self.shapes {
            target.intern_shape(shape.into_vec());
        }
    }
}

#[cfg(test)]
fn value_contains_foreign(value: DecodedValue, target: Storage) -> bool {
    match value {
        DecodedValue::Atom(id) | DecodedValue::ShortString(id) => id.storage != target,
        DecodedValue::NativeType(_) => false,
        DecodedValue::Bytes(handle)
        | DecodedValue::Opaque(handle)
        | DecodedValue::DeclaredType(handle)
        | DecodedValue::SymbolicType(handle)
        | DecodedValue::Array(handle)
        | DecodedValue::Tuple(handle)
        | DecodedValue::Tagged(handle)
        | DecodedValue::Dict(handle)
        | DecodedValue::Module(handle)
        | DecodedValue::Func(handle)
        | DecodedValue::Dyn(handle)
        | DecodedValue::TypeSlot(handle) => handle.storage != target,
        DecodedValue::Failed(_)
        | DecodedValue::Int(_)
        | DecodedValue::Float(_)
        | DecodedValue::BuiltinAtom(_)
        | DecodedValue::InlineAtom(_)
        | DecodedValue::InlineString(_)
        | DecodedValue::FuncRef(_) => false,
    }
}

#[cfg(test)]
fn val_contains_foreign(value: Val, target: Storage) -> bool {
    value_contains_foreign(value.value(), target)
}

fn object_contains_disallowed(
    object: &Object,
    target: Storage,
    background: Option<Storage>,
) -> bool {
    let foreign = |storage| storage != target && Some(storage) != background;
    let value_foreign = |value: Val| {
        let payload_is_foreign = match value.value() {
            DecodedValue::Atom(id) | DecodedValue::ShortString(id) => foreign(id.storage),
            DecodedValue::NativeType(_) => false,
            DecodedValue::Bytes(handle)
            | DecodedValue::Opaque(handle)
            | DecodedValue::DeclaredType(handle)
            | DecodedValue::SymbolicType(handle)
            | DecodedValue::Array(handle)
            | DecodedValue::Tuple(handle)
            | DecodedValue::Tagged(handle)
            | DecodedValue::Dict(handle)
            | DecodedValue::Module(handle)
            | DecodedValue::Func(handle)
            | DecodedValue::Dyn(handle)
            | DecodedValue::TypeSlot(handle) => foreign(handle.storage),
            DecodedValue::Failed(_)
            | DecodedValue::Int(_)
            | DecodedValue::Float(_)
            | DecodedValue::BuiltinAtom(_)
            | DecodedValue::InlineAtom(_)
            | DecodedValue::InlineString(_)
            | DecodedValue::FuncRef(_) => false,
        };
        payload_is_foreign
    };
    match object {
        Object::Reserved | Object::OpenFunc => true,
        Object::DeclaredType { sealed: false, .. } => true,
        Object::SymbolicType { sealed: false, .. } => true,
        Object::Array(values) | Object::Tuple(values) => {
            values.iter().any(|value| value_foreign(*value))
        }
        Object::Tagged { tag, payload } => value_foreign(*tag) || value_foreign(*payload),
        Object::Dict { shape, values } => {
            foreign(shape.storage) || values.iter().any(|value| value_foreign(*value))
        }
        Object::Module { exports } => {
            foreign(exports.shape.storage)
                || exports.values.iter().any(|value| value_foreign(*value))
        }
        Object::Closure { upvalues, .. } => upvalues.iter().any(|value| value_foreign(*value)),
        Object::Dyn {
            descriptor, value, ..
        } => value_foreign(*descriptor) || value_foreign(*value),
        Object::DeclaredType { body, .. } => value_foreign(*body),
        Object::SymbolicType { body, .. } => value_foreign(*body),
        Object::TypeSlot { value } => value.is_none_or(value_foreign),
        Object::ByteCodeProto {
            values,
            text,
            prototypes,
            ..
        } => {
            values.iter().any(|value| value_foreign(*value))
                || text.iter().any(|id| foreign(id.storage))
                || prototypes.iter().any(|prototype| match prototype {
                    RuntimePrototype::Bytecode(handle) => foreign(handle.storage),
                    RuntimePrototype::Native(_) => false,
                })
        }
        Object::Bytes(_) | Object::Opaque(_) => false,
    }
}

fn builtin_atom(text: &str) -> Option<BuiltinAtom> {
    match text {
        "None" => Some(BuiltinAtom::None),
        "Some" => Some(BuiltinAtom::Some),
        "Ok" => Some(BuiltinAtom::Ok),
        "Err" => Some(BuiltinAtom::Err),
        "True" => Some(BuiltinAtom::True),
        "False" => Some(BuiltinAtom::False),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(name: &str, range: std::ops::Range<usize>) -> Loc {
        let mut sources = crate::SourceDatabase::default();
        let source = sources.add(name, "0123456789");
        Loc::from_usize(source, range).unwrap()
    }

    fn rv(value: DecodedValue) -> Val {
        value.into()
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn val_is_compact_and_copy() {
        fn assert_copy<T: Copy>() {}

        assert_copy::<Val>();
        assert_eq!(std::mem::size_of::<Val>(), 32);
        assert_eq!(std::mem::align_of::<Val>(), 8);
        assert_eq!(std::mem::size_of::<Meta>(), 4);
    }

    #[test]
    fn flat_meta_round_trips_exact_classification_and_traits() {
        for storage in [Storage::Main, Storage::Work] {
            let value = Val::unknown(DecodedValue::Array(Handle { storage, slot: 7 }));
            assert_eq!(
                value.value(),
                DecodedValue::Array(Handle { storage, slot: 7 })
            );
            assert_eq!(value.meta.sub_kind(), HeapKind::Array);
            assert_ne!(value.meta.traits() & TRAIT_REFERENCE, 0);
            assert_ne!(value.meta.traits() & TRAIT_HEAP, 0);
            assert_ne!(value.meta.traits() & TRAIT_TRACE, 0);
            assert_eq!(ScopedId::from_raw(value.raw).storage(), storage);
        }
    }

    #[test]
    fn inline_text_and_native_type_use_no_heap_or_text_slot() {
        let mut heap = Heap::work();
        let short_string = Val::unknown(heap.string(None, "1234567"));
        let short_atom = Val::unknown(heap.atom(None, "1234567"));
        let native = crate::NativeType::bind(
            crate::value::NativeTypeId {
                module: crate::value::NativeModuleId(7),
                local: 11,
            },
            "fixture#Native",
        );
        let native_value = Val::unknown(DecodedValue::NativeType(
            heap.intern_native_type(native.clone()),
        ));

        assert_eq!(heap.counts(), (0, 0, 0));
        let view = HeapView {
            current: &heap,
            background: None,
        };
        assert_eq!(view.string_text(short_string).unwrap().unwrap(), "1234567");
        assert_eq!(view.atom_text(short_atom).unwrap().unwrap(), "1234567");
        let DecodedValue::NativeType(id) = native_value.value() else {
            panic!("expected immediate NativeType")
        };
        assert_eq!(heap.native_type(id).unwrap(), &native);

        let long = Val::unknown(heap.string(None, "12345678"));
        assert!(matches!(long.value(), DecodedValue::ShortString(_)));
        assert_eq!(heap.counts(), (0, 1, 0));
    }

    #[test]
    fn canonical_type_id_is_independent_from_value_storage() {
        let raw = Val::unknown(DecodedValue::Int(1));
        let typed = raw.with_type_id(crate::TypeId::builtin(7));
        assert_eq!(typed.type_id(), Some(crate::TypeId::builtin(7)));
        assert_eq!(typed.value(), DecodedValue::Int(1));
    }

    #[test]
    fn equality_never_guesses_across_a_nominal_witness_boundary() {
        let heap = Heap::work();
        let view = HeapView {
            current: &heap,
            background: None,
        };
        let raw = Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::True));
        let typed = raw.with_type_id(crate::TypeId::builtin(7));
        assert!(!view.values_equal(typed, raw).unwrap());
        assert!(!view.values_equal(raw, typed).unwrap());
        assert!(view.values_equal(typed, typed).unwrap());
    }

    #[test]
    fn val_equality_ignores_location() {
        let left = Val::new(DecodedValue::Int(42), Some(location("left", 1..2)));
        let right = Val::new(DecodedValue::Int(42), Some(location("right", 3..4)));

        assert_eq!(left, right);
    }

    #[test]
    fn call_site_rebasing_preserves_original_values_only() {
        let original_loc = location("data", 1..2);
        let generated_loc = location("function", 3..4);
        let call_loc = location("caller", 5..6);

        let original = Val::original(DecodedValue::Int(1), Some(original_loc));
        let generated = Val::new(DecodedValue::Int(2), Some(generated_loc));

        let preserved = original.rebase_generated(Some(call_loc));
        assert!(preserved.is_original());
        assert_eq!(preserved.loc(), Some(original_loc));
        assert_eq!(
            generated.rebase_generated(Some(call_loc)).loc(),
            Some(call_loc)
        );
        assert_eq!(
            Val::unknown(DecodedValue::Int(3))
                .rebase_generated(Some(call_loc))
                .loc(),
            Some(call_loc)
        );
    }

    #[test]
    fn copy_preserves_root_and_collection_edge_locations() {
        let root_loc = location("root", 0..5);
        let item_loc = location("item", 6..7);
        let mut world = Heap::main();
        let mut current = Heap::work();
        let array = current.allocate(Object::Array(
            vec![Val::original(DecodedValue::Int(42), Some(item_loc))].into(),
        ));
        let root = Val::original(DecodedValue::Array(array), Some(root_loc));

        let copied = copy_roots(
            &mut world,
            HeapView {
                current: &current,
                background: None,
            },
            &[root],
        )
        .unwrap()[0];

        assert_eq!(copied.loc(), Some(root_loc));
        assert!(copied.is_original());
        let DecodedValue::Array(handle) = copied.value() else {
            panic!("expected copied Array")
        };
        let Object::Array(items) = world.object(handle).unwrap() else {
            panic!("expected copied Array object")
        };
        assert_eq!(items[0].loc(), Some(item_loc));
        assert!(items[0].is_original());
    }

    #[test]
    fn copy_is_reachable_reinterning_and_target_self_contained() {
        let mut world = Heap::main();
        let shared = world.allocate(Object::Bytes(vec![9].into()));
        let mut current = Heap::work();
        let atom = current.atom(Some(&world), "Custom");
        let string = current.string(Some(&world), "Custom");
        let root = current.allocate(Object::Tuple(
            vec![rv(atom), rv(string), rv(DecodedValue::Bytes(shared))].into(),
        ));
        current.allocate(Object::Bytes(vec![1, 2, 3].into()));

        let copied = copy_roots(
            &mut world,
            HeapView {
                current: &current,
                background: None,
            },
            &[rv(DecodedValue::Tuple(root))],
        )
        .unwrap();

        assert_eq!(world.counts(), (2, 0, 0));
        let DecodedValue::Tuple(root) = copied[0].value() else {
            panic!("expected tuple root")
        };
        let Object::Tuple(values) = world.object(root).unwrap() else {
            panic!("expected tuple object")
        };
        assert_eq!(values[2], rv(DecodedValue::Bytes(shared)));
        assert!(
            !values
                .iter()
                .any(|value| val_contains_foreign(*value, Storage::Main))
        );
    }

    #[test]
    fn copy_preserves_cycles_and_failure_is_atomic() {
        let mut world = Heap::main();
        let mut current = Heap::work();
        let cycle = current.reserve();
        current
            .initialize(
                cycle,
                Object::Array(vec![rv(DecodedValue::Array(cycle))].into()),
            )
            .unwrap();
        copy_roots(
            &mut world,
            HeapView {
                current: &current,
                background: None,
            },
            &[rv(DecodedValue::Array(cycle))],
        )
        .unwrap();
        assert_eq!(world.counts().0, 1);

        let before = world.counts();
        let invalid = DecodedValue::Array(Handle {
            storage: Storage::Work,
            slot: 99,
        });
        assert!(
            copy_roots(
                &mut world,
                HeapView {
                    current: &current,
                    background: None,
                },
                &[rv(invalid)],
            )
            .is_err()
        );
        assert_eq!(world.counts(), before);
    }

    #[test]
    fn multiple_roots_share_one_forwarding_context() {
        let mut target = Heap::main();
        let mut source = Heap::work();
        let shared = source.allocate(Object::Bytes(vec![1].into()));
        let roots = copy_roots(
            &mut target,
            HeapView {
                current: &source,
                background: None,
            },
            &[
                rv(DecodedValue::Bytes(shared)),
                rv(DecodedValue::Bytes(shared)),
            ],
        )
        .unwrap();
        assert_eq!(roots[0], roots[1]);
        assert_eq!(target.counts().0, 1);
    }

    #[test]
    fn work_relocation_copies_work_edges_and_retains_main_edges() {
        let mut main = Heap::main();
        let stable = main.allocate(Object::Bytes(vec![9].into()));
        let mut source = Heap::work();
        let shared = source.allocate(Object::Bytes(vec![1, 2, 3].into()));
        let cycle = source.reserve();
        source
            .initialize(
                cycle,
                Object::Array(vec![rv(DecodedValue::Array(cycle))].into()),
            )
            .unwrap();
        let root = source.allocate(Object::Tuple(
            vec![
                rv(DecodedValue::Bytes(shared)),
                rv(DecodedValue::Bytes(shared)),
                rv(DecodedValue::Bytes(stable)),
                rv(DecodedValue::Array(cycle)),
            ]
            .into(),
        ));
        source.allocate(Object::Bytes(vec![4, 5, 6].into()));
        let mut target = Heap::work();
        target.allocate(Object::Bytes(Box::new([])));

        let relocated = relocate_work_roots(
            &mut target,
            &main,
            &source,
            &[rv(DecodedValue::Tuple(root))],
        )
        .unwrap();

        assert_eq!(target.counts().0, 4);
        let DecodedValue::Tuple(root) = relocated[0].value() else {
            panic!("expected relocated tuple")
        };
        let Object::Tuple(values) = target.object(root).unwrap() else {
            panic!("expected relocated tuple object")
        };
        assert_eq!(values[0], values[1]);
        assert_eq!(values[2], rv(DecodedValue::Bytes(stable)));
        let DecodedValue::Array(cycle) = values[3].value() else {
            panic!("expected relocated cycle")
        };
        let Object::Array(cycle_values) = target.object(cycle).unwrap() else {
            panic!("expected relocated cycle object")
        };
        assert_eq!(cycle_values[0], rv(DecodedValue::Array(cycle)));
        assert_ne!(root.slot, 0);
    }

    #[test]
    fn failed_nodes_cross_module_publication_but_not_host_publication() {
        let main = Heap::main();
        let mut source = Heap::work();
        let root = Val::unknown(DecodedValue::Array(source.allocate(Object::Array(
            vec![Val::unknown(DecodedValue::Failed(7))].into(),
        ))));

        let mut target = Heap::work();
        let relocated = relocate_work_roots(&mut target, &main, &source, &[root]).unwrap();
        let DecodedValue::Array(handle) = relocated[0].value() else {
            panic!("expected relocated Array")
        };
        let Object::Array(items) = target.object(handle).unwrap() else {
            panic!("expected relocated Array object")
        };
        assert!(matches!(items[0].value(), DecodedValue::Failed(7)));
        let mut destination = Heap::main();
        assert!(publish_root(&mut destination, &source, root).is_err());
        let published = publish_module_root(&mut destination, &source, root).unwrap();
        assert_eq!(
            HeapView {
                current: &destination,
                background: None,
            }
            .first_data_failure(published.runtime())
            .unwrap(),
            Some(7)
        );
    }

    #[test]
    fn publication_preserves_main_edges_and_relocates_work_edges() {
        let mut main = Heap::main();
        let stable = rv(DecodedValue::Bytes(
            main.allocate(Object::Bytes(vec![1, 2, 3].into_boxed_slice())),
        ));
        let mut work = Heap::work();
        let work_root = work.allocate(Object::Array(vec![stable].into()));

        let published = publish_root(&mut main, &work, rv(DecodedValue::Array(work_root)))
            .unwrap()
            .runtime();
        let DecodedValue::Array(main_root) = published.value() else {
            panic!("expected published Array")
        };
        assert_eq!(main_root.storage, Storage::Main);
        let Object::Array(items) = main.object(main_root).unwrap() else {
            panic!("expected Main Array")
        };
        let DecodedValue::Bytes(stable_bytes) = items[0].value() else {
            panic!("expected Main Bytes")
        };
        assert_eq!(stable_bytes.storage, Storage::Main);
        assert_eq!(main.counts(), (2, 0, 0));
    }
    #[test]
    fn structural_equality_terminates_on_internal_cycles() {
        let mut local = Heap::work();
        let left = local.reserve();
        local
            .initialize(
                left,
                Object::Array(vec![rv(DecodedValue::Array(left))].into()),
            )
            .unwrap();
        let right = local.reserve();
        local
            .initialize(
                right,
                Object::Array(vec![rv(DecodedValue::Array(right))].into()),
            )
            .unwrap();
        let world = Heap::main();
        assert!(
            HeapView {
                current: &local,
                background: Some(&world),
            }
            .values_equal(
                rv(DecodedValue::Array(left)),
                rv(DecodedValue::Array(right))
            )
            .unwrap()
        );
    }

    #[test]
    fn promotion_copies_ready_type_slots_and_rejects_uninitialized_links() {
        let mut local = Heap::work();
        let link = local.allocate(Object::TypeSlot { value: None });
        let array = local.allocate(Object::Array(vec![rv(DecodedValue::TypeSlot(link))].into()));
        local
            .initialize_type_slot(link, rv(DecodedValue::Array(array)))
            .unwrap();
        let mut world = Heap::main();
        let DecodedValue::TypeSlot(persistent_link) =
            publish_root(&mut world, &local, rv(DecodedValue::TypeSlot(link)))
                .unwrap()
                .runtime()
                .value()
        else {
            panic!("expected persistent up-link")
        };
        let reader = Heap::work();
        let view = HeapView {
            current: &reader,
            background: Some(&world),
        };
        let DecodedValue::Array(array) = view
            .type_slot(persistent_link)
            .unwrap()
            .expect("published up-link is ready")
            .value()
        else {
            panic!("expected Array")
        };
        assert_eq!(
            view.sequence(array, false).unwrap(),
            &[rv(DecodedValue::TypeSlot(persistent_link))]
        );

        let mut uninitialized = Heap::work();
        let link = uninitialized.allocate(Object::TypeSlot { value: None });
        assert!(
            publish_root(&mut world, &uninitialized, rv(DecodedValue::TypeSlot(link))).is_err()
        );
    }
}
