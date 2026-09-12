//! Native ABI v1: 64-bit little-endian hosts; words are always eight bytes.
use telora_core::{
    candidate_layout::{self, State},
    mir::{SealedMir, TypeId},
    source::Loc,
};

pub type Result<T> = std::result::Result<T, String>;
pub const HEADER_WORDS: usize = 2;

/// The numeric identity is the sealed MIR index, never a second type allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TypeKey(pub(crate) u32);
impl TypeKey {
    pub fn index(self) -> usize {
        self.0 as usize
    }
    pub fn raw(self) -> u32 {
        self.0
    }
}
impl TryFrom<TypeId> for TypeKey {
    type Error = String;
    fn try_from(id: TypeId) -> Result<Self> {
        Ok(Self(
            u32::try_from(id.index()).map_err(|_| "TypeId overflow")?,
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum World {
    Main,
    Work,
}
/// High bit selects Work, low 31 bits index the type-directed object table.
/// No null sentinel: slot zero is valid. World generation belongs to the context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct HeapRef(u32);
impl HeapRef {
    pub(crate) fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
    pub fn new(world: World, slot: u32) -> Result<Self> {
        if slot >= 1 << 31 {
            return Err("HeapId exceeds 31-bit slot space".into());
        }
        Ok(Self(slot | if world == World::Work { 1 << 31 } else { 0 }))
    }
    pub fn raw(self) -> u32 {
        self.0
    }
    pub fn world(self) -> World {
        if self.0 >> 31 == 0 {
            World::Main
        } else {
            World::Work
        }
    }
    pub fn slot(self) -> u32 {
        self.0 & 0x7fff_ffff
    }
}

/// SourceId zero means absent source, and requires both offsets to be zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Origin {
    source: u32,
    start: u32,
    end: u32,
}
impl Origin {
    pub(crate) fn from_words([source, start, end]: [u32; 3]) -> Result<Self> {
        if start > end || (source == 0 && (start != 0 || end != 0)) {
            return Err("invalid native source range".into());
        }
        Ok(Self { source, start, end })
    }
    pub fn from_loc(loc: Option<Loc>) -> Self {
        loc.map(|l| Self {
            source: l.source.get(),
            start: l.start,
            end: l.end,
        })
        .unwrap_or_default()
    }
    pub fn words(self) -> [u32; 3] {
        [self.source, self.start, self.end]
    }
}

#[derive(Clone, Copy, Debug)]
enum Layout {
    Value(usize),
    Never,
    Static,
}
pub struct Layouts {
    entries: Vec<Layout>,
    pub(crate) field_names: Vec<Vec<String>>,
    pub(crate) variant_payloads: Vec<Vec<Option<TypeKey>>>,
}
impl Layouts {
    pub fn from_mir(mir: &SealedMir<'_>) -> Result<Self> {
        if !cfg!(all(target_pointer_width = "64", target_endian = "little")) {
            return Err("native ABI v1 requires a 64-bit little-endian host".into());
        }
        let mut field_names = Vec::new();
        let mut variant_payloads = Vec::new();
        let entries = candidate_layout::calculate(mir)?
            .into_iter()
            .map(|entry| {
                variant_payloads.push(
                    entry
                        .variants
                        .iter()
                        .map(|v| {
                            v.type_id
                                .map(|i| {
                                    u32::try_from(i)
                                        .map(TypeKey)
                                        .map_err(|_| "variant TypeId overflow".to_owned())
                                })
                                .transpose()
                        })
                        .collect::<Result<Vec<_>>>()?,
                );
                field_names.push(
                    entry
                        .object
                        .as_ref()
                        .map(|o| o.members.iter().map(|m| m.name.clone()).collect())
                        .unwrap_or_default(),
                );
                Ok(match entry.layout {
                    State::Known { shape } => {
                        if shape.value_bytes < 16
                            || shape.value_bytes % 8 != 0
                            || shape.value_alignment > 8
                        {
                            return Err("unsupported native value alignment or width".into());
                        }
                        Layout::Value(
                            usize::try_from(shape.value_bytes / 8)
                                .map_err(|_| "value size overflow")?,
                        )
                    }
                    State::Uninhabited { .. } => Layout::Never,
                    State::Template { .. } | State::CompileTime { .. } => Layout::Static,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            entries,
            field_names,
            variant_payloads,
        })
    }
    pub fn type_at(&self, index: usize) -> Result<TypeKey> {
        self.entries
            .get(index)
            .ok_or("TypeId outside sealed image")?;
        Ok(TypeKey(
            u32::try_from(index).map_err(|_| "TypeId overflow")?,
        ))
    }
    pub fn words(&self, ty: TypeKey) -> Result<usize> {
        match self
            .entries
            .get(ty.0 as usize)
            .ok_or("TypeId outside sealed image")?
        {
            Layout::Value(words) => Ok(*words),
            Layout::Never => Err("uninhabited type has no runtime slot".into()),
            Layout::Static => Err("static or template type has no runtime slot".into()),
        }
    }
    pub fn is_never(&self, ty: TypeKey) -> Result<bool> {
        Ok(matches!(
            self.entries
                .get(ty.0 as usize)
                .ok_or("TypeId outside sealed image")?,
            Layout::Never
        ))
    }
    pub fn value(&self, ty: TypeKey, origin: Origin, data: &[u64]) -> Result<Value> {
        let size = self.words(ty)?;
        if data.len() != size - HEADER_WORDS {
            return Err("value data width mismatch".into());
        }
        let [source, start, end] = origin.words();
        let mut words = Vec::with_capacity(size);
        words.push(u64::from(source) | (u64::from(start) << 32));
        words.push(u64::from(end) | (u64::from(ty.0) << 32));
        words.extend_from_slice(data);
        Ok(Value {
            arena: 0,
            words: words.into_boxed_slice(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Value {
    pub(crate) arena: u64,
    pub(crate) words: Box<[u64]>,
}
impl Value {
    #[cfg(feature = "jit")]
    pub(crate) fn from_result(words: Box<[u64]>, expected: TypeKey, size: usize) -> Result<Self> {
        if words.len() != size || size < HEADER_WORDS {
            return Err("native result width mismatch".into());
        }
        let value = Self { arena: 0, words };
        if value.type_key() != expected {
            return Err("native result TypeId mismatch".into());
        }
        let [source, start, end] = value.origin().words();
        if start > end || (source == 0 && (start != 0 || end != 0)) {
            return Err("native result source range invalid".into());
        }
        Ok(value)
    }
    pub fn words(&self) -> &[u64] {
        &self.words
    }
    pub fn type_key(&self) -> TypeKey {
        TypeKey((self.words[1] >> 32) as u32)
    }
    pub fn origin(&self) -> Origin {
        Origin {
            source: self.words[0] as u32,
            start: (self.words[0] >> 32) as u32,
            end: self.words[1] as u32,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Slot {
    ty: TypeKey,
    offset: usize,
    words: usize,
}
impl Slot {
    pub fn type_key(self) -> TypeKey {
        self.ty
    }
    pub fn offset_words(self) -> usize {
        self.offset
    }
    pub fn len_words(self) -> usize {
        self.words
    }
}
/// Immutable offsets. Each activation owns a non-moving, zeroed buffer.
/// Generated code can use an equivalent native stack allocation.
pub struct FrameLayout {
    slots: Vec<Slot>,
    words: usize,
}
impl FrameLayout {
    pub fn new(layouts: &Layouts, types: &[TypeKey]) -> Result<Self> {
        let mut words: usize = 0;
        let mut slots = Vec::with_capacity(types.len());
        for &ty in types {
            let size = layouts.words(ty)?;
            slots.push(Slot {
                ty,
                offset: words,
                words: size,
            });
            words = words.checked_add(size).ok_or("frame size overflow")?;
        }
        let bytes = words.checked_mul(8).ok_or("frame byte size overflow")?;
        if bytes > i32::MAX as usize {
            return Err("frame exceeds native offset range".into());
        }
        Ok(Self { slots, words })
    }
    pub fn slots(&self) -> &[Slot] {
        &self.slots
    }
    pub fn words(&self) -> usize {
        self.words
    }
    pub fn activate(&self) -> Activation<'_> {
        Activation {
            layout: self,
            words: vec![0; self.words].into_boxed_slice(),
            initialized: vec![false; self.slots.len()],
        }
    }
}
pub struct Activation<'a> {
    layout: &'a FrameLayout,
    words: Box<[u64]>,
    initialized: Vec<bool>,
}
impl Activation<'_> {
    pub fn write(&mut self, index: usize, value: &Value) -> Result<()> {
        let slot = self.layout.slots.get(index).ok_or("invalid frame slot")?;
        if value.type_key() != slot.ty || value.words.len() != slot.words {
            return Err("frame value type/width mismatch".into());
        }
        self.words[slot.offset..slot.offset + slot.words].copy_from_slice(&value.words);
        self.initialized[index] = true;
        Ok(())
    }
    pub fn read(&self, index: usize) -> Result<&[u64]> {
        let slot = self.layout.slots.get(index).ok_or("invalid frame slot")?;
        if !self.initialized[index] {
            return Err("uninitialized frame slot".into());
        }
        Ok(&self.words[slot.offset..slot.offset + slot.words])
    }
}

/// Only Success allows reading a result buffer. Other values never cross the ABI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Status {
    Success = 0,
    Failed = 1,
}
#[derive(Default)]
pub struct CallContext {
    diagnostics: Vec<NativeDiagnostic>,
    runtime: Option<crate::runtime::Runtime>,
}
#[derive(Debug)]
pub struct NativeDiagnostic {
    pub message: String,
    pub origin: Origin,
}
impl CallContext {
    pub fn with_runtime(runtime: crate::runtime::Runtime) -> Self {
        Self {
            runtime: Some(runtime),
            ..Self::default()
        }
    }
    pub fn runtime(&self) -> Result<&crate::runtime::Runtime> {
        self.runtime
            .as_ref()
            .ok_or_else(|| "native call requires a runtime".into())
    }
    pub fn runtime_mut(&mut self) -> Result<&mut crate::runtime::Runtime> {
        self.runtime
            .as_mut()
            .ok_or_else(|| "native call requires a runtime".into())
    }
    pub fn diagnostics(&self) -> &[NativeDiagnostic] {
        &self.diagnostics
    }
    pub fn fail(&mut self, message: impl Into<String>) -> Status {
        self.fail_at(message, Origin::default())
    }
    pub fn fail_at(&mut self, message: impl Into<String>, origin: Origin) -> Status {
        self.diagnostics.push(NativeDiagnostic {
            message: message.into(),
            origin,
        });
        Status::Failed
    }
    /// Host helper panic boundary. Propagated Failed need not add a diagnostic.
    pub fn boundary(&mut self, call: impl FnOnce(&mut Self) -> Status) -> Status {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| call(self))) {
            Ok(status) => status,
            Err(_) => self.fail("native runtime helper panicked"),
        }
    }
}

#[cfg(test)]
mod tests;
