//! Candidate ABI only. No runtime objects, inference, or name-based type rules.
use crate::{
    mir::{SealedMir, TypeConstructor as T, TypeId, TypeOperation},
    type_image::TypeImage,
};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Shape {
    pub data_bytes: u64,
    pub data_alignment: u64,
    pub value_bytes: u64,
    pub value_alignment: u64,
    pub table: Option<&'static str>,
    pub encoding: &'static str,
}
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum State {
    Known { shape: Shape },
    Template { reason: &'static str },
    CompileTime { reason: &'static str },
    Uninhabited { reason: &'static str },
}
#[derive(Debug, Serialize)]
pub struct Member {
    pub name: String,
    pub type_id: Option<usize>,
    pub offset: Option<u64>,
    pub storage: &'static str,
    pub table: Option<&'static str>,
}
#[derive(Debug, Serialize)]
pub struct Object {
    pub status: &'static str,
    pub bytes: Option<u64>,
    pub element_type: Option<usize>,
    pub element_stride: Option<u64>,
    pub members: Vec<Member>,
    pub storage_rule: String,
    pub storage: Storage,
}
/// Executable size formulas, not estimates of allocator overhead or RSS.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Storage {
    Fixed { bytes: u64 },
    Sequence { stride: u64, empty_only: bool },
    Dictionary { value_stride: u64, empty_only: bool },
    Captures,
    FullValue,
}
pub enum Extent<'a> {
    Fixed,
    Sequence { length: u32 },
    Dictionary { length: u32 },
    Captures(&'a [u64]),
    FullValue(u64),
}
impl Storage {
    pub fn allocation_bytes(&self, extent: Extent<'_>) -> Result<u64, String> {
        let mul = |a: u64, b: u64| {
            a.checked_mul(b)
                .ok_or_else(|| "candidate layout size overflow".to_owned())
        };
        match (self, extent) {
            (Self::Fixed { bytes }, Extent::Fixed) => Ok(*bytes),
            (Self::Sequence { stride, empty_only }, Extent::Sequence { length }) => {
                if *empty_only && length != 0 {
                    return Err("uninhabited elements require an empty sequence".into());
                }
                mul(*stride, length as u64)
            }
            (
                Self::Dictionary {
                    value_stride,
                    empty_only,
                },
                Extent::Dictionary { length },
            ) => {
                if *empty_only && length != 0 {
                    return Err("invalid dictionary extent".into());
                }
                mul(add(32, *value_stride)?, length.into())
            }
            (Self::Captures, Extent::Captures(sizes)) => {
                let count = u32::try_from(sizes.len()).map_err(|_| "capture count overflow")?;
                let mut bytes = align(add(8, mul(4, count.into())?)?, 8)?;
                for &size in sizes {
                    if size < 16 || size % 8 != 0 {
                        return Err("invalid full captured value size".into());
                    }
                    bytes = add(bytes, size)?;
                }
                u32::try_from(bytes).map_err(|_| "capture object exceeds u32 offset space")?;
                Ok(bytes)
            }
            (Self::FullValue, Extent::FullValue(bytes)) if bytes >= 16 && bytes % 8 == 0 => {
                Ok(bytes)
            }
            _ => Err("extent does not match object layout".into()),
        }
    }
}
#[derive(Debug, Serialize)]
pub struct Entry {
    pub type_id: usize,
    pub constructor: &'static str,
    pub arguments: Vec<usize>,
    pub native_identity: Option<[u32; 2]>,
    pub layout: State,
    pub object: Option<Object>,
    pub variants: Vec<Member>,
}
impl Entry {
    pub fn id(&self) -> TypeId {
        TypeId(self.type_id as u32)
    }
}
fn constructor_name(c: &T) -> &'static str {
    match c {
        T::Int => "Int",
        T::Float => "Float",
        T::String => "String",
        T::Bytes => "Bytes",
        T::Bool => "Bool",
        T::Never => "Never",
        T::Type => "Type",
        T::TypeOf => "TypeOf",
        T::Dyn => "Dyn",
        T::Option => "Option",
        T::Result => "Result",
        T::FoldControl => "FoldControl",
        T::PropertyTarget => "PropertyTarget",
        T::PropertyBound => "PropertyBound",
        T::Unchecked => "Unchecked",
        T::TypeFunction(_) => "TypeFunction",
        T::Nominal(_) => "Nominal",
        T::Native(_) => "Native",
        T::Tuple => "Tuple",
        T::Array => "Array",
        T::ArrayLiteral => "ArrayLiteral",
        T::TupleLiteral => "TupleLiteral",
        T::TypeList => "TypeList",
        T::Dict => "Dict",
        T::Function => "Function",
        T::Quantified(_) => "Quantified",
        T::Bound(_) => "Bound",
        T::Record(_) => "Record",
        T::Newtype => "Newtype",
        T::Enum(_) => "Enum",
        T::Meta => "Meta",
        T::Namespace(_) => "Namespace",
        T::Parameter(_) => "Parameter",
    }
}
fn compile_time(c: &T) -> bool {
    matches!(
        c,
        T::Meta
            | T::Namespace(_)
            | T::TypeFunction(_)
            | T::TypeList
            | T::PropertyBound
            | T::Bound(_)
    )
}
fn add(a: u64, b: u64) -> Result<u64, String> {
    a.checked_add(b)
        .ok_or_else(|| "candidate layout size overflow".into())
}
fn align(n: u64, a: u64) -> Result<u64, String> {
    Ok(add(n, a - 1)? / a * a)
}
fn shape(
    bytes: u64,
    alignment: u64,
    table: Option<&'static str>,
    encoding: &'static str,
) -> Result<State, String> {
    Ok(State::Known {
        shape: Shape {
            data_bytes: bytes,
            data_alignment: alignment,
            value_bytes: add(16, align(bytes, 8)?)?,
            value_alignment: 8,
            table,
            encoding,
        },
    })
}
struct Builder<'a> {
    image: &'a TypeImage,
    states: Vec<Option<State>>,
    templates: Vec<bool>,
    inhabited: Vec<bool>,
    variants: Vec<Vec<(String, Option<TypeId>)>>,
    fields: Vec<Vec<(String, TypeId)>>,
    indirect: Vec<Vec<bool>>,
    module_records: Vec<bool>,
}
impl<'a> Builder<'a> {
    fn new(image: &'a TypeImage, module_records: Vec<bool>) -> Result<Self, String> {
        let n = image.types.len();
        let mut b = Self {
            image,
            states: vec![None; n],
            templates: vec![false; n],
            inhabited: vec![false; n],
            variants: vec![vec![]; n],
            fields: vec![vec![]; n],
            indirect: vec![vec![]; n],
            module_records,
        };
        for (i, ty) in image.types.iter().enumerate() {
            let id = TypeId(i as u32);
            match &ty.constructor {
                T::Nominal(s) => {
                    let d = image
                        .definition(*s)
                        .ok_or("missing sealed nominal definition")?;
                    if matches!(
                        d.operation,
                        TypeOperation::Struct | TypeOperation::Newtype | TypeOperation::Enum
                    ) {
                        let l = image.layout(id).ok_or("missing sealed nominal layout")?;
                        if d.members.len() != l.members.len() {
                            return Err("nominal member/layout mismatch".into());
                        }
                        for (m, t) in d.members.iter().zip(&l.members) {
                            if d.operation == TypeOperation::Enum {
                                b.variants[i].push((m.name.clone(), *t));
                            } else {
                                b.fields[i].push((m.name.clone(), t.ok_or("missing field type")?));
                            }
                        }
                    }
                }
                T::Record(names) => {
                    b.fields[i] = names
                        .iter()
                        .cloned()
                        .zip(ty.arguments.iter().copied())
                        .collect()
                }
                T::Tuple | T::Newtype => {
                    b.fields[i] = ty
                        .arguments
                        .iter()
                        .enumerate()
                        .map(|(i, t)| (i.to_string(), *t))
                        .collect()
                }
                T::Enum(names) => {
                    let mut args = ty.arguments.iter();
                    for (name, payload) in names {
                        b.variants[i].push((
                            name.clone(),
                            if *payload {
                                Some(*args.next().ok_or("missing enum argument")?)
                            } else {
                                None
                            },
                        ));
                    }
                }
                T::Option | T::Result | T::FoldControl | T::PropertyTarget => {
                    for index in 0..6 {
                        if let Some((name, _)) =
                            crate::type_image::builtin_variant(&ty.constructor, index)
                        {
                            let p =
                                crate::type_image::builtin_variant_argument(&ty.constructor, index)
                                    .map(|a| ty.arguments[a]);
                            b.variants[i].push((name.into(), p));
                        }
                    }
                }
                _ => {}
            }
        }
        // Quantified binds Bound nodes, never free declaration Parameters.
        let mut users = vec![vec![]; n];
        for (i, ty) in image.types.iter().enumerate() {
            for arg in &ty.arguments {
                users[arg.index()].push(i);
            }
            for (_, t) in &b.fields[i] {
                users[t.index()].push(i);
            }
            for (_, t) in &b.variants[i] {
                if let Some(t) = t {
                    users[t.index()].push(i);
                }
            }
        }
        for bound in [false, true] {
            let mut open: Vec<_> = image
                .types
                .iter()
                .map(|t| {
                    if bound {
                        matches!(t.constructor, T::Bound(_))
                    } else {
                        matches!(t.constructor, T::Parameter(_))
                    }
                })
                .collect();
            let mut queue: Vec<_> = open
                .iter()
                .enumerate()
                .filter_map(|(i, t)| t.then_some(i))
                .collect();
            while let Some(i) = queue.pop() {
                for &u in &users[i] {
                    if bound && matches!(image.types[u].constructor, T::Quantified(_)) {
                        continue;
                    }
                    if !open[u] {
                        open[u] = true;
                        queue.push(u);
                    }
                }
            }
            for (i, t) in open.into_iter().enumerate() {
                b.templates[i] |= t;
            }
        }
        for i in 0..n {
            if b.templates[i] || b.module_records[i] {
                continue;
            }
            for t in b.fields[i]
                .iter()
                .map(|(_, t)| *t)
                .chain(b.variants[i].iter().filter_map(|(_, t)| *t))
            {
                if compile_time(&image.types[t.index()].constructor) {
                    return Err(format!(
                        "runtime member of type {i} ({:?}) contains compile-time type {} ({:?})",
                        image.types[i].constructor,
                        t.index(),
                        image.types[t.index()].constructor
                    ));
                }
            }
        }
        // Least fixed point of finite constructors. Empty containers and
        // functions remain inhabited even with a Never element/return type.
        loop {
            let mut changed = false;
            for (i, ty) in image.types.iter().enumerate() {
                if b.inhabited[i]
                    || b.templates[i]
                    || compile_time(&ty.constructor)
                    || b.module_records[i]
                {
                    continue;
                }
                let yes = if b.is_enum(i) {
                    b.variants[i]
                        .iter()
                        .any(|(_, p)| p.is_none_or(|p| b.inhabited[p.index()]))
                } else if b.is_fields(i) {
                    b.fields[i].iter().all(|(_, p)| b.inhabited[p.index()])
                } else if ty.constructor == T::Unchecked {
                    b.inhabited[ty.arguments[0].index()]
                } else {
                    !matches!(ty.constructor, T::Never)
                };
                if yes {
                    b.inhabited[i] = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        // Heap edges stop inline recursion. Box exactly edges belonging to an
        // inline cycle, independently of traversal order or provisional sizes.
        let edges: Vec<Vec<usize>> = image
            .types
            .iter()
            .enumerate()
            .map(|(i, t)| {
                if b.is_enum(i) {
                    b.variants[i]
                        .iter()
                        .filter_map(|(_, p)| p.map(|p| p.index()))
                        .collect()
                } else if t.constructor == T::Unchecked {
                    vec![t.arguments[0].index()]
                } else {
                    vec![]
                }
            })
            .collect();
        for i in 0..n {
            b.indirect[i] = b.variants[i]
                .iter()
                .map(|(_, p)| p.is_some_and(|p| reaches(&edges, p.index(), i)))
                .collect();
        }
        Ok(b)
    }
    fn is_enum(&self, i: usize) -> bool {
        match self.image.types[i].constructor {
            T::Enum(_) | T::Option | T::Result | T::FoldControl | T::PropertyTarget => true,
            T::Nominal(s) => self
                .image
                .definition(s)
                .is_some_and(|d| d.operation == TypeOperation::Enum),
            _ => false,
        }
    }
    fn is_fields(&self, i: usize) -> bool {
        match self.image.types[i].constructor {
            T::Tuple | T::Record(_) | T::Newtype => true,
            T::Nominal(s) => self.image.definition(s).is_some_and(|d| {
                matches!(
                    d.operation,
                    TypeOperation::Struct
                        | TypeOperation::Newtype
                        | TypeOperation::Tuple
                        | TypeOperation::Unit
                )
            }),
            _ => false,
        }
    }
    fn value(&mut self, id: TypeId) -> Result<State, String> {
        let i = id.index();
        if let Some(s) = &self.states[i] {
            return Ok(s.clone());
        }
        let ty = &self.image.types[i];
        let state = if self.module_records[i] {
            State::CompileTime {
                reason: "module export record containing static declarations; identified by MIR module body",
            }
        } else if compile_time(&ty.constructor) {
            State::CompileTime {
                reason: "static type expression, namespace, binder or constraint; not a runtime value",
            }
        } else if self.templates[i] {
            State::Template {
                reason: "contains a free type parameter outside a quantified contract",
            }
        } else if !self.inhabited[i] {
            State::Uninhabited {
                reason: "no finite value constructor (least fixed point)",
            }
        } else {
            match &ty.constructor {
                T::Int | T::Float | T::Bool => shape(8, 8, None, "scalar_bits")?,
                T::Type | T::TypeOf => shape(4, 4, None, "represented_type_id:u32")?,
                T::String => shape(
                    16,
                    8,
                    Some("StringTable"),
                    "tag:u8,length:u8,inline_utf8:[u8;14] OR tag:u8,pad:[u8;3],heap:u32,start:u32,end:u32",
                )?,
                T::Bytes => shape(12, 4, Some("BytesTable"), "heap:u32,start:u32,end:u32")?,
                T::Array => shape(12, 4, Some("ArrayTable"), "heap:u32,start:u32,end:u32")?,
                T::Dict => shape(
                    16,
                    4,
                    Some("ArrayTable"),
                    "keys_heap:u32,length:u32,values_heap:u32,reserved:u32=0",
                )?,
                T::Record(_) => shape(4, 4, Some("RecordTable"), "heap:u32")?,
                T::Tuple if ty.arguments.is_empty() => shape(0, 1, None, "unit")?,
                T::Tuple => shape(4, 4, Some("RecordTable"), "heap:u32")?,
                T::Newtype => shape(4, 4, Some("NewtypeTable"), "heap:u32")?,
                T::Unchecked => self.value(ty.arguments[0])?,
                T::Function | T::Quantified(_) => shape(
                    8,
                    4,
                    Some("ClosureEnvTable"),
                    "function_id:u32,environment_id:u32 (0 = no captures)",
                )?,
                T::Dyn => shape(
                    24,
                    8,
                    Some("ValueTable"),
                    "concrete_type:u32,storage:u32,payload:[u8;16] (0=inline data,1=boxed HeapId)",
                )?,
                T::Native(native) => {
                    if !matches!(
                        (native.module, native.slot),
                        (19, 0) | (20, 1) | (16, 3) | (33, 0) | (34, 0)
                    ) {
                        return Err(format!(
                            "native ABI {}#{} has no candidate resource contract",
                            native.module, native.slot
                        ));
                    }
                    shape(
                        4,
                        4,
                        Some("NativeResourceTable"),
                        "heap:u32 (table partitioned by native ABI identity)",
                    )?
                }
                T::Nominal(s) if self.is_fields(i) => {
                    match self.image.definition(*s).unwrap().operation {
                        TypeOperation::Unit => shape(0, 1, None, "unit")?,
                        TypeOperation::Struct => shape(4, 4, Some("RecordTable"), "heap:u32")?,
                        TypeOperation::Tuple => shape(4, 4, Some("RecordTable"), "heap:u32")?,
                        TypeOperation::Newtype => shape(4, 4, Some("NewtypeTable"), "heap:u32")?,
                        _ => unreachable!(),
                    }
                }
                _ if self.is_enum(i) => {
                    let mut max = 0;
                    for (index, (_, p)) in self.variants[i].clone().into_iter().enumerate() {
                        if let Some(p) = p {
                            if !self.inhabited[p.index()] {
                                continue;
                            }
                            let size = if self.indirect[i][index] {
                                4
                            } else {
                                match self.value(p)? {
                                    State::Known { shape } => shape.value_bytes,
                                    other => {
                                        return Err(format!(
                                            "enum payload {} is not materializable: {other:?}",
                                            p.index()
                                        ));
                                    }
                                }
                            };
                            max = max.max(size);
                        }
                    }
                    shape(
                        add(8, max)?,
                        8,
                        None,
                        "tag:u32,pad:u32,payload (active branch only)",
                    )?
                }
                other => return Err(format!("type {} has no concrete layout rule: {other:?}", i)),
            }
        };
        self.states[i] = Some(state.clone());
        Ok(state)
    }
    fn stride(&self, t: TypeId) -> Result<u64, String> {
        match self.states[t.index()].as_ref().unwrap() {
            State::Known { shape } => Ok(shape.value_bytes),
            State::Uninhabited { .. } => Ok(0),
            other => Err(format!(
                "runtime storage contains non-materializable type {}: {other:?}",
                t.index()
            )),
        }
    }
    fn object(&self, id: TypeId) -> Result<Option<Object>, String> {
        let State::Known { shape } = self.states[id.index()].as_ref().unwrap() else {
            return Ok(None);
        };
        let Some(table) = shape.table else {
            return Ok(None);
        };
        let mut owner = id;
        while self.image.types[owner.index()].constructor == T::Unchecked {
            owner = self.image.types[owner.index()].arguments[0];
        }
        let ty = &self.image.types[owner.index()];
        let mut o = Object {
            status: "known",
            bytes: None,
            element_type: None,
            element_stride: None,
            members: vec![],
            storage_rule: String::new(),
            storage: Storage::Fixed { bytes: 0 },
        };
        match table {
            "StringTable" | "BytesTable" => {
                o.element_stride = Some(1);
                o.storage = Storage::Sequence {
                    stride: 1,
                    empty_only: false,
                };
                o.storage_rule="byte_length bytes; table entry stores byte_length:u32; slice bounds checked against byte_length".into();
            }
            "ArrayTable" if ty.constructor != T::Dict => {
                let t = ty.arguments[0];
                let stride = self.stride(t)?;
                o.element_type = Some(t.index());
                o.element_stride = Some(stride);
                o.storage = Storage::Sequence {
                    stride,
                    empty_only: stride == 0,
                };
                o.storage_rule = if stride == 0 {
                    "length must be 0 for an uninhabited element type".into()
                } else {
                    format!(
                        "length * {stride} bytes; table entry stores length:u32; each element is a full value"
                    )
                };
            }
            "RecordTable" | "NewtypeTable" => {
                let mut offset = 0;
                for (name, t) in &self.fields[owner.index()] {
                    o.members.push(Member {
                        name: name.clone(),
                        type_id: Some(t.index()),
                        offset: Some(offset),
                        storage: "full_value",
                        table: None,
                    });
                    offset = add(offset, self.stride(*t)?)?;
                }
                o.bytes = Some(offset);
                o.storage = Storage::Fixed { bytes: offset };
                o.storage_rule="fixed fields in declaration/canonical MIR order, each with its own full header".into();
            }
            "ArrayTable" => {
                let t = ty.arguments[0];
                let stride = self.stride(t)?;
                o.element_type = Some(t.index());
                o.element_stride = Some(stride);
                o.storage = Storage::Dictionary {
                    value_stride: stride,
                    empty_only: stride == 0,
                };
                o.storage_rule = format!(
                    "two whole ArrayTable slots; keys: length * 32 bytes (full String values), strictly increasing UTF-8 byte order; values: length * {stride} bytes; equal column lengths; binary search; no slice offsets; reserved=0; uninhabited values require length=0"
                );
            }
            "ClosureEnvTable" => {
                o.storage = Storage::Captures;
                o.storage_rule="header 8 bytes: capture_count:u32,total_bytes:u32; offsets[capture_count]:u32 at 8, full captured values start at align8(8+4*capture_count); offsets relative to object start; values packed at 8-byte alignment in stable capture order; total_bytes ends after last full value; empty environment uses id 0".into();
            }
            "ValueTable" => {
                o.storage = Storage::FullValue;
                o.storage_rule="one full concrete value at offset 0; size from its stamped TypeId; Dyn inlines data when data_bytes<=16 and alignment<=8, otherwise payload[0..4] is ValueTable HeapId; outer Dyn retains concrete value origin; inline heap references keep their original table".into();
            }
            "NativeResourceTable" => {
                o.storage = Storage::Fixed { bytes: 8 };
                o.bytes = Some(8);
                o.storage_rule="host_resource_token:u64; partition by (native module,slot); registered host ABI owns resource lifetime and clone/drop/trace, including any retained Telora values; token is not a Telora heap pointer".into();
            }
            _ => return Err(format!("unknown object table {table}")),
        }
        Ok(Some(o))
    }
}
fn reaches(edges: &[Vec<usize>], start: usize, target: usize) -> bool {
    let mut seen = vec![false; edges.len()];
    let mut todo = vec![start];
    while let Some(i) = todo.pop() {
        if i == target {
            return true;
        }
        if !seen[i] {
            seen[i] = true;
            todo.extend(edges[i].iter().copied());
        }
    }
    false
}
pub fn calculate(sealed: &SealedMir<'_>) -> Result<Vec<Entry>, String> {
    let image = sealed.types();
    let mut records = vec![false; image.types.len()];
    for module in &sealed.mir().modules {
        let body = match module.state {
            crate::mir::ModuleState::Source { body, .. }
            | crate::mir::ModuleState::Data { body } => body,
            _ => continue,
        };
        if let crate::mir::TypeState::Known(id) = sealed.mir().ty_slots[body.index()] {
            let ty = &image.types[id.index()];
            if matches!(ty.constructor, T::Record(_))
                && ty
                    .arguments
                    .iter()
                    .any(|t| compile_time(&image.types[t.index()].constructor))
            {
                records[id.index()] = true;
            }
        }
    }
    calculate_image(image, records)
}
fn calculate_image(image: &TypeImage, module_records: Vec<bool>) -> Result<Vec<Entry>, String> {
    let mut b = Builder::new(image, module_records)?;
    for i in 0..image.types.len() {
        b.value(TypeId(i as u32))?;
    }
    (0..image.types.len())
        .map(|i| {
            let known = matches!(b.states[i], Some(State::Known { .. }));
            let variants = b.variants[i]
                .iter()
                .enumerate()
                .map(|(index, (name, p))| {
                    let live = known && p.is_none_or(|p| b.inhabited[p.index()]);
                    let indirect = b.indirect[i][index];
                    Member {
                        name: name.clone(),
                        type_id: p.map(|p| p.index()),
                        offset: p.filter(|_| live).map(|_| 24),
                        storage: if !live {
                            "uninhabited_or_template"
                        } else if p.is_none() {
                            "nullary"
                        } else if indirect {
                            "heap_id"
                        } else {
                            "full_value"
                        },
                        table: (live && indirect).then_some("ValueTable"),
                    }
                })
                .collect();
            Ok(Entry {
                type_id: i,
                constructor: constructor_name(&image.types[i].constructor),
                arguments: image.types[i]
                    .arguments
                    .iter()
                    .map(|id| id.index())
                    .collect(),
                native_identity: match image.types[i].constructor {
                    T::Native(id) => Some([id.module, id.slot]),
                    _ => None,
                },
                layout: b.states[i].clone().unwrap(),
                object: b.object(TypeId(i as u32))?,
                variants,
            })
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checked_sizes() {
        for (data, a, expected) in [(0, 1, 16), (8, 8, 24), (12, 4, 32)] {
            let State::Known { shape } = shape(data, a, None, "test").unwrap() else {
                unreachable!()
            };
            assert_eq!(shape.value_bytes, expected);
        }
        assert!(shape(u64::MAX, 8, None, "test").is_err());
    }
    #[test]
    fn inline_cycle_edges_are_order_independent() {
        let edges = vec![vec![1], vec![0, 2], vec![]];
        assert!(reaches(&edges, 1, 0));
        assert!(reaches(&edges, 0, 1));
        assert!(!reaches(&edges, 2, 1));
    }
    fn image(types: Vec<(T, Vec<u32>)>) -> TypeImage {
        let mut mir = crate::mir::Mir::default();
        mir.types = types
            .into_iter()
            .map(|(constructor, args)| crate::mir::ResolvedType {
                constructor,
                arguments: args.into_iter().map(TypeId).collect(),
            })
            .collect();
        TypeImage::from_mir(&mir).unwrap()
    }
    #[test]
    fn quantified_binds_only_bound_parameters_and_missing_rules_fail() {
        let img = image(vec![
            (T::Bound(0), vec![]),
            (T::Function, vec![0, 0]),
            (T::Quantified(1), vec![1]),
            (T::Parameter(crate::mir::SymbolId(0)), vec![]),
            (T::Function, vec![3, 3]),
            (T::Quantified(1), vec![4]),
        ]);
        let entries = calculate_image(&img, vec![false; img.types.len()]).unwrap();
        assert!(matches!(entries[0].layout, State::CompileTime { .. }));
        assert!(matches!(entries[1].layout, State::Template { .. }));
        assert!(matches!(entries[2].layout, State::Known { .. }));
        assert!(matches!(entries[5].layout, State::Template { .. }));
        let bad = image(vec![(
            T::Native(crate::mir::NativeTypeId {
                module: 999,
                slot: 0,
            }),
            vec![],
        )]);
        assert!(
            calculate_image(&bad, vec![false; bad.types.len()])
                .unwrap_err()
                .contains("no candidate resource contract")
        );
        let invalid = image(vec![
            (T::Meta, vec![]),
            (T::Record(vec!["bad".into()]), vec![0]),
        ]);
        assert!(
            calculate_image(&invalid, vec![false; invalid.types.len()])
                .unwrap_err()
                .contains("compile-time type")
        );
    }
    #[test]
    fn storage_extents_are_checked() {
        let dict = Storage::Dictionary {
            value_stride: 24,
            empty_only: false,
        };
        assert_eq!(
            dict.allocation_bytes(Extent::Dictionary { length: 2 })
                .unwrap(),
            112
        );
        assert!(
            Storage::Dictionary {
                value_stride: 0,
                empty_only: true
            }
            .allocation_bytes(Extent::Dictionary { length: 1 })
            .is_err()
        );
        assert_eq!(
            Storage::Captures
                .allocation_bytes(Extent::Captures(&[24, 32]))
                .unwrap(),
            72
        );
        assert!(
            Storage::Captures
                .allocation_bytes(Extent::Captures(&[u32::MAX as u64 + 1]))
                .is_err()
        );
        assert!(
            Storage::Sequence {
                stride: u64::MAX,
                empty_only: false
            }
            .allocation_bytes(Extent::Sequence { length: 2 })
            .is_err()
        );
        assert!(
            Storage::Sequence {
                stride: 0,
                empty_only: true
            }
            .allocation_bytes(Extent::Sequence { length: 1 })
            .is_err()
        );
    }
}
