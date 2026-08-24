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
    properties: BTreeMap<PropertyKey, Val>,
    property_attr_type: Option<crate::TypeId>,
    memoized_interpreters: HashMap<usize, HashMap<Vec<crate::TypeId>, Val>>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum PropertyKey {
    Ty {
        ty: crate::TypeId,
        property_ty: crate::TypeId,
    },
    Field {
        ty: crate::TypeId,
        member_index: u32,
        property_ty: crate::TypeId,
    },
    Variant {
        ty: crate::TypeId,
        member_index: u32,
        property_ty: crate::TypeId,
    },
}

impl PropertyKey {
    pub(crate) const fn property_type(self) -> crate::TypeId {
        match self {
            Self::Ty { property_ty, .. }
            | Self::Field { property_ty, .. }
            | Self::Variant { property_ty, .. } => property_ty,
        }
    }
}
