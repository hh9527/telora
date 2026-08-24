use crate::TypeConstructorId;
use crate::types::TypeDescriptor;
use hashbrown::raw::RawTable;
use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::{Arc, Mutex};

pub(crate) type SharedTypeStore = Arc<Mutex<TypeStore>>;

pub(crate) fn shared_type_store() -> SharedTypeStore {
    Arc::new(Mutex::new(TypeStore::default()))
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TypeId(u32);

impl TypeId {
    pub const FIRST_DYNAMIC: u32 = crate::FIRST_DYNAMIC_MODULE_LOCAL;

    pub const fn builtin(raw: u32) -> Self {
        assert!(
            raw < Self::FIRST_DYNAMIC,
            "builtin TypeId exceeds reserved range"
        );
        Self(raw)
    }

    pub(crate) const fn from_raw(raw: u32) -> Option<Self> {
        if raw == 0 { None } else { Some(Self(raw)) }
    }

    fn from_index(index: usize) -> Self {
        let index = u32::try_from(index).expect("type store exceeds u32 ID space");
        Self(
            Self::FIRST_DYNAMIC
                .checked_add(index)
                .expect("type store exceeds u32 ID space"),
        )
    }

    fn index(self) -> Option<usize> {
        self.0
            .checked_sub(Self::FIRST_DYNAMIC)
            .map(|index| index as usize)
    }

    pub const fn raw(self) -> u32 {
        self.0
    }

    pub(crate) const ANY: Self = Self::builtin(1);
    pub(crate) const NEVER: Self = Self::builtin(2);
    pub(crate) const TYPE: Self = Self::builtin(3);
    pub(crate) const DYN: Self = Self::builtin(4);
    pub(crate) const INT: Self = Self::builtin(5);
    pub(crate) const FLOAT: Self = Self::builtin(6);
    pub(crate) const STRING: Self = Self::builtin(7);
    pub(crate) const BYTES: Self = Self::builtin(8);
    pub(crate) const ATOM: Self = Self::builtin(9);
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum TypeInternKey {
    Nominal {
        constructor: TypeConstructorId,
        arguments: Box<[TypeId]>,
    },
    Structural(TypeShape),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum TypeShape {
    TypeOf(TypeId),
    Opaque(String),
    Atom(String),
    Array(TypeId),
    Dict(TypeId),
    Tagged {
        tag: String,
        payload: TypeId,
    },
    Tuple(Box<[TypeId]>),
    Struct(Box<[(String, TypeId)]>),
    Enum(Box<[(String, Option<TypeId>)]>),
    Union(Box<[TypeId]>),
    Function {
        parameters: Box<[TypeId]>,
        result: TypeId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TypeData {
    pub(crate) name: String,
    pub(crate) shape: TypeShape,
}

#[derive(Clone, Debug)]
enum TypeSlot {
    Pending(TypeInternKey),
    Ready { key: TypeInternKey, data: TypeData },
    Aborted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InternType {
    Existing(TypeId),
    Reserved(TypeId),
}

struct InternEntry {
    hash: u64,
    key: TypeInternKey,
    id: TypeId,
}

pub(crate) struct TypeStore {
    slots: Vec<TypeSlot>,
    intern: RawTable<InternEntry>,
    hasher: RandomState,
}

impl Default for TypeStore {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            intern: RawTable::new(),
            hasher: RandomState::new(),
        }
    }
}

impl TypeStore {
    pub(crate) fn begin(
        &mut self,
        constructor: TypeConstructorId,
        arguments: impl Into<Box<[TypeId]>>,
    ) -> InternType {
        let key = TypeInternKey::Nominal {
            constructor,
            arguments: arguments.into(),
        };
        let hash = self.hash(&key);
        if let Some(entry) = self.intern.get(hash, |entry| entry.key == key) {
            return InternType::Existing(entry.id);
        }
        let id = TypeId::from_index(self.slots.len());
        self.slots.push(TypeSlot::Pending(key.clone()));
        self.intern
            .insert(hash, InternEntry { hash, key, id }, |entry| entry.hash);
        InternType::Reserved(id)
    }

    pub(crate) fn seal(&mut self, id: TypeId, data: TypeData) -> Result<(), &'static str> {
        let slot = self.slot_mut(id)?;
        let TypeSlot::Pending(key) = slot else {
            return Err("TypeId is not pending");
        };
        *slot = TypeSlot::Ready {
            key: key.clone(),
            data,
        };
        Ok(())
    }

    pub(crate) fn abort(&mut self, id: TypeId) -> Result<(), &'static str> {
        let key = match self.slot(id)? {
            TypeSlot::Pending(key) => key.clone(),
            _ => return Err("TypeId is not pending"),
        };
        let hash = self.hash(&key);
        self.intern
            .remove_entry(hash, |entry| entry.id == id)
            .ok_or("pending TypeId has no intern entry")?;
        *self.slot_mut(id)? = TypeSlot::Aborted;
        Ok(())
    }

    pub(crate) fn get(&self, id: TypeId) -> Option<&TypeData> {
        match self.slot(id).ok()? {
            TypeSlot::Ready { key, data } => {
                let _ = key;
                Some(data)
            }
            TypeSlot::Pending(_) | TypeSlot::Aborted => None,
        }
    }

    pub(crate) fn is_pending(&self, id: TypeId) -> bool {
        matches!(self.slot(id), Ok(TypeSlot::Pending(_)))
    }

    pub(crate) fn intern_descriptor(
        &mut self,
        descriptor: &TypeDescriptor,
    ) -> Result<TypeId, String> {
        self.intern_descriptor_with_names(descriptor, &HashMap::new())
    }

    pub(crate) fn intern_descriptor_with_names(
        &mut self,
        descriptor: &TypeDescriptor,
        names: &HashMap<String, TypeId>,
    ) -> Result<TypeId, String> {
        match descriptor {
            TypeDescriptor::Any => Ok(TypeId::ANY),
            TypeDescriptor::Never => Ok(TypeId::NEVER),
            TypeDescriptor::Type => Ok(TypeId::TYPE),
            TypeDescriptor::Dyn => Ok(TypeId::DYN),
            TypeDescriptor::Int => Ok(TypeId::INT),
            TypeDescriptor::Float => Ok(TypeId::FLOAT),
            TypeDescriptor::String => Ok(TypeId::STRING),
            TypeDescriptor::Bytes => Ok(TypeId::BYTES),
            TypeDescriptor::AtomValue => Ok(TypeId::ATOM),
            TypeDescriptor::Bound(_) => Err("cannot canonicalize an unbound type parameter".into()),
            TypeDescriptor::Named(name) => names
                .get(name)
                .copied()
                .ok_or_else(|| format!("cannot canonicalize unresolved named type {name:?}")),
            TypeDescriptor::Inference(_) => {
                Err("cannot canonicalize an unresolved inference variable".into())
            }
            TypeDescriptor::Declared(declared) => {
                let arguments = declared
                    .id
                    .arguments()
                    .iter()
                    .map(|argument| self.intern_descriptor_with_names(argument, names))
                    .collect::<Result<Vec<_>, _>>()?;
                match self.begin(declared.id.constructor(), arguments) {
                    InternType::Existing(id) => Ok(id),
                    InternType::Reserved(id) => {
                        let mut nested_names = names.clone();
                        nested_names.insert(declared.name.clone(), id);
                        self.finish_nominal_descriptor(
                            id,
                            declared.name.clone(),
                            &declared.body,
                            &nested_names,
                        )
                    }
                }
            }
            TypeDescriptor::TypeOf(inner) => {
                let inner = self.intern_descriptor_with_names(inner, names)?;
                Ok(self.intern_structural(TypeShape::TypeOf(inner)))
            }
            TypeDescriptor::Opaque(native) => {
                Ok(self.intern_structural(TypeShape::Opaque(native.qualified_name().to_owned())))
            }
            TypeDescriptor::Atom(atom) => {
                Ok(self.intern_structural(TypeShape::Atom(atom.name().to_owned())))
            }
            TypeDescriptor::Array(item) => {
                let item = self.intern_descriptor_with_names(item, names)?;
                Ok(self.intern_structural(TypeShape::Array(item)))
            }
            TypeDescriptor::Dict(item) => {
                let item = self.intern_descriptor_with_names(item, names)?;
                Ok(self.intern_structural(TypeShape::Dict(item)))
            }
            TypeDescriptor::Tagged { tag, payload } => {
                let payload = self.intern_descriptor_with_names(payload, names)?;
                Ok(self.intern_structural(TypeShape::Tagged {
                    tag: tag.name().to_owned(),
                    payload,
                }))
            }
            TypeDescriptor::Tuple(items) => {
                let items = items
                    .iter()
                    .map(|item| self.intern_descriptor_with_names(item, names))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(self.intern_structural(TypeShape::Tuple(items.into())))
            }
            TypeDescriptor::Struct(fields) => {
                let fields = fields
                    .iter()
                    .map(|(name, field)| {
                        self.intern_descriptor_with_names(field, names)
                            .map(|field| (name.clone(), field))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(self.intern_structural(TypeShape::Struct(fields.into())))
            }
            TypeDescriptor::Enum(variants) => {
                let variants = variants
                    .iter()
                    .map(|(name, payload)| {
                        payload
                            .as_deref()
                            .map(|payload| self.intern_descriptor_with_names(payload, names))
                            .transpose()
                            .map(|payload| (name.clone(), payload))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(self.intern_structural(TypeShape::Enum(variants.into())))
            }
            TypeDescriptor::Union(variants) => {
                let variants = variants
                    .iter()
                    .map(|variant| self.intern_descriptor_with_names(variant, names))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(self.intern_structural(TypeShape::Union(variants.into())))
            }
            TypeDescriptor::Function { parameters, result } => {
                let parameters = parameters
                    .iter()
                    .map(|parameter| self.intern_descriptor_with_names(parameter, names))
                    .collect::<Result<Vec<_>, _>>()?;
                let result = self.intern_descriptor_with_names(result, names)?;
                Ok(self.intern_structural(TypeShape::Function {
                    parameters: parameters.into(),
                    result,
                }))
            }
        }
    }

    pub(crate) fn seal_descriptor(
        &mut self,
        id: TypeId,
        name: impl Into<String>,
        descriptor: &TypeDescriptor,
        names: &HashMap<String, TypeId>,
    ) -> Result<(), String> {
        self.finish_nominal_descriptor(id, name.into(), descriptor, names)
            .map(|_| ())
    }

    pub(crate) fn seal_shape(
        &mut self,
        id: TypeId,
        name: impl Into<String>,
        shape: TypeShape,
    ) -> Result<(), String> {
        self.seal(
            id,
            TypeData {
                name: name.into(),
                shape,
            },
        )
        .map_err(str::to_owned)
    }

    fn finish_nominal_descriptor(
        &mut self,
        id: TypeId,
        name: String,
        descriptor: &TypeDescriptor,
        names: &HashMap<String, TypeId>,
    ) -> Result<TypeId, String> {
        let shape = match self.descriptor_shape(descriptor, names) {
            Ok(shape) => shape,
            Err(error) => {
                self.abort(id).map_err(str::to_owned)?;
                return Err(error);
            }
        };
        if let Err(error) = self.seal(id, TypeData { name, shape }) {
            if self.is_pending(id) {
                self.abort(id).map_err(str::to_owned)?;
            }
            return Err(error.to_owned());
        }
        Ok(id)
    }

    fn descriptor_shape(
        &mut self,
        descriptor: &TypeDescriptor,
        names: &HashMap<String, TypeId>,
    ) -> Result<TypeShape, String> {
        match descriptor {
            TypeDescriptor::Struct(fields) => fields
                .iter()
                .map(|(name, field)| {
                    self.intern_descriptor_with_names(field, names)
                        .map(|field| (name.clone(), field))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|fields| TypeShape::Struct(fields.into())),
            TypeDescriptor::Enum(variants) => variants
                .iter()
                .map(|(name, payload)| {
                    payload
                        .as_deref()
                        .map(|payload| self.intern_descriptor_with_names(payload, names))
                        .transpose()
                        .map(|payload| (name.clone(), payload))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|variants| TypeShape::Enum(variants.into())),
            _ => Err("nominal type body must be a struct or enum".into()),
        }
    }

    pub(crate) fn intern_structural(&mut self, shape: TypeShape) -> TypeId {
        let key = TypeInternKey::Structural(shape.clone());
        let hash = self.hash(&key);
        if let Some(entry) = self.intern.get(hash, |entry| entry.key == key) {
            return entry.id;
        }
        let id = TypeId::from_index(self.slots.len());
        self.slots.push(TypeSlot::Ready {
            key: key.clone(),
            data: TypeData {
                name: String::new(),
                shape,
            },
        });
        self.intern
            .insert(hash, InternEntry { hash, key, id }, |entry| entry.hash);
        id
    }

    fn slot(&self, id: TypeId) -> Result<&TypeSlot, &'static str> {
        id.index()
            .and_then(|index| self.slots.get(index))
            .ok_or("TypeId is not allocated by this store")
    }

    fn slot_mut(&mut self, id: TypeId) -> Result<&mut TypeSlot, &'static str> {
        id.index()
            .and_then(|index| self.slots.get_mut(index))
            .ok_or("TypeId is not allocated by this store")
    }

    fn hash(&self, key: &TypeInternKey) -> u64 {
        let mut hasher = self.hasher.build_hasher();
        key.hash(&mut hasher);
        hasher.finish()
    }
}

#[cfg(test)]
#[path = "type_store/tests/mod.rs"]
mod tests;
