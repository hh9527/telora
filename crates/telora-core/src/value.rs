use crate::bytecode::BytecodeFunction;
use crate::vm::CallContext;
use std::any::Any;
use std::fmt;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BuiltinAtom {
    None,
    Some,
    Ok,
    Err,
    True,
    False,
}

impl BuiltinAtom {
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Some => "Some",
            Self::Ok => "Ok",
            Self::Err => "Err",
            Self::True => "True",
            Self::False => "False",
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Atom {
    Builtin(BuiltinAtom),
    Named(Arc<str>),
}

impl Atom {
    pub fn named(name: impl Into<Arc<str>>) -> Self {
        Self::Named(name.into())
    }

    pub const fn builtin(atom: BuiltinAtom) -> Self {
        Self::Builtin(atom)
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Builtin(atom) => atom.name(),
            Self::Named(name) => name,
        }
    }
}

#[derive(Debug, Eq, Hash, PartialEq)]
pub struct Shape {
    fields: Arc<[String]>,
}

impl Shape {
    pub(crate) fn from_sorted_fields(fields: Vec<String>) -> Self {
        debug_assert!(fields.windows(2).all(|pair| pair[0] < pair[1]));
        Self {
            fields: fields.into(),
        }
    }

    pub fn fields(&self) -> &[String] {
        &self.fields
    }

    pub fn field_index(&self, field: &str) -> Option<usize> {
        self.fields
            .binary_search_by(|candidate| candidate.as_str().cmp(field))
            .ok()
    }
}

#[derive(Clone, Debug)]
pub struct Dict {
    shape: Arc<Shape>,
    values: Arc<[Value]>,
}

type OpaquePayload = dyn Any + Send + Sync;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct NativeModuleId(pub(crate) u32);

pub(crate) const RESERVED_NATIVE_MODULE_MAX: u32 = 1023;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct NativeTypeId {
    pub(crate) module: NativeModuleId,
    pub(crate) local: u32,
}

#[derive(Clone, Debug)]
pub struct NativeType {
    id: NativeTypeId,
    qualified_name: Arc<str>,
}

impl NativeType {
    pub(crate) fn bind(id: NativeTypeId, qualified_name: impl Into<Arc<str>>) -> Self {
        Self {
            id,
            qualified_name: qualified_name.into(),
        }
    }

    pub fn qualified_name(&self) -> &str {
        &self.qualified_name
    }
}

impl PartialEq for NativeType {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for NativeType {}

impl std::hash::Hash for NativeType {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

#[derive(Clone)]
pub struct OpaqueValue {
    native_type: NativeType,
    payload: Arc<OpaquePayload>,
    equal: fn(&OpaquePayload, &OpaquePayload) -> bool,
}

impl OpaqueValue {
    pub fn new<T>(native_type: NativeType, payload: T) -> Self
    where
        T: Any + Eq + Send + Sync,
    {
        Self {
            native_type,
            payload: Arc::new(payload),
            equal: |left, right| {
                left.downcast_ref::<T>()
                    .zip(right.downcast_ref::<T>())
                    .is_some_and(|(left, right)| left == right)
            },
        }
    }

    pub fn new_identity<T>(native_type: NativeType, payload: T) -> Self
    where
        T: Any + Send + Sync,
    {
        Self {
            native_type,
            payload: Arc::new(payload),
            equal: |left, right| std::ptr::eq(left, right),
        }
    }

    pub fn native_type(&self) -> &NativeType {
        &self.native_type
    }

    pub fn downcast_ref<T: Any>(&self, expected_type: &NativeType) -> Option<&T> {
        (&self.native_type == expected_type)
            .then(|| self.payload.downcast_ref::<T>())
            .flatten()
    }

    pub(crate) fn logical_eq(&self, other: &Self) -> bool {
        self.native_type == other.native_type
            && (self.equal)(self.payload.as_ref(), other.payload.as_ref())
    }
}

impl fmt::Debug for OpaqueValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "<opaque {}>", self.native_type.qualified_name)
    }
}

#[derive(Clone, Debug)]
pub struct Closure {
    identity: Arc<()>,
    prototype: Prototype,
    upvalues: Arc<[Value]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeError {
    pub message: String,
    limit: Option<NativeLimit>,
    non_finite_float: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum NativeLimit {
    Stack,
    Allocation,
}

impl NativeError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            limit: None,
            non_finite_float: false,
        }
    }

    pub(crate) fn stack_limit(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            limit: Some(NativeLimit::Stack),
            non_finite_float: false,
        }
    }

    pub(crate) fn allocation_limit(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            limit: Some(NativeLimit::Allocation),
            non_finite_float: false,
        }
    }

    pub(crate) fn non_finite_float() -> Self {
        Self {
            message: "NonFiniteFloat".into(),
            limit: None,
            non_finite_float: true,
        }
    }

    pub(crate) const fn limit(&self) -> Option<NativeLimit> {
        self.limit
    }

    pub(crate) const fn is_non_finite_float(&self) -> bool {
        self.non_finite_float
    }
}

pub type NativeCallback = fn(&mut CallContext<'_, '_>) -> Result<(), NativeError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoreArrayFunction {
    Length,
    Get,
    Enumerate,
    Push,
    Concat,
    Zip,
    Map,
    Filter,
    FlatMap,
    Fold,
    FoldControl,
    Any,
    All,
    Find,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoreDictFunction {
    Get,
    Keys,
    Values,
    Pairs,
    FromPairs,
    Merge,
    MapValues,
    Filter,
    Fold,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoreStringFunction {
    Length,
    Chars,
    Join,
    JoinLines,
    Split,
    Lines,
    StartsWith,
    EndsWith,
    Contains,
    Replace,
    Indent,
    EnsureTrailingNewline,
    TrimMargin,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CorePathFunction {
    Join,
    Normalize,
    Parent,
    FileName,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoreAttributesFunction {
    Normalize,
    Add,
    Get,
    Has,
    All,
    Strip,
}

impl CoreAttributesFunction {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Normalize => "std/attributes.normalize",
            Self::Add => "std/attributes.add",
            Self::Get => "std/attributes.get",
            Self::Has => "std/attributes.has",
            Self::All => "std/attributes.all",
            Self::Strip => "std/attributes.strip",
        }
    }

    pub(crate) const fn arity(self) -> usize {
        match self {
            Self::Normalize | Self::All | Self::Strip => 1,
            Self::Add | Self::Get | Self::Has => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoreModelFunction {
    Struct,
    Enum,
    Union,
}

impl CoreModelFunction {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Union => "union",
        }
    }

    pub(crate) const fn arity(self) -> usize {
        2
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoreBuiltinTypeFunction {
    FoldControl,
    Option,
    Result,
}

impl CoreBuiltinTypeFunction {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::FoldControl => "FoldControl",
            Self::Option => "Option",
            Self::Result => "Result",
        }
    }

    pub(crate) const fn arity(self) -> usize {
        match self {
            Self::Option => 1,
            Self::FoldControl | Self::Result => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoreDiagnosticFunction {
    Warn,
}

impl CoreDiagnosticFunction {
    pub(crate) const fn name(self) -> &'static str {
        "\0telora_warn"
    }

    pub(crate) const fn arity(self) -> usize {
        2
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoreEqFunction {
    Equal,
}

impl CoreEqFunction {
    pub(crate) const fn name(self) -> &'static str {
        "std/eq.equal"
    }

    pub(crate) const fn arity(self) -> usize {
        2
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoreHashFunction {
    Sha256,
}

impl CoreHashFunction {
    pub(crate) const fn name(self) -> &'static str {
        "std/hash.sha256"
    }

    pub(crate) const fn arity(self) -> usize {
        1
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoreCodecFunction {
    Decode,
    Encode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoreTypeDescFunction {
    Kind,
    Children,
    OpaqueName,
    Resolve,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoreDynFunction {
    Pack,
    Desc,
    Kind,
    CheckInt,
    CheckFloat,
    CheckString,
    CheckBytes,
    Field,
    Fields,
    ArrayItems,
    TupleItems,
    Tag,
    Payload,
}

impl CoreDynFunction {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Pack => "std/dyn.pack",
            Self::Desc => "std/dyn.desc",
            Self::Kind => "std/dyn.kind",
            Self::CheckInt => "std/dyn.check_int",
            Self::CheckFloat => "std/dyn.check_float",
            Self::CheckString => "std/dyn.check_string",
            Self::CheckBytes => "std/dyn.check_bytes",
            Self::Field => "std/dyn.field",
            Self::Fields => "std/dyn.fields",
            Self::ArrayItems => "std/dyn.array_items",
            Self::TupleItems => "std/dyn.tuple_items",
            Self::Tag => "std/dyn.tag",
            Self::Payload => "std/dyn.payload",
        }
    }

    pub(crate) const fn arity(self) -> usize {
        match self {
            Self::Pack | Self::Field => 2,
            _ => 1,
        }
    }
}

impl CoreTypeDescFunction {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Kind => "std/type-desc.kind",
            Self::Children => "std/type-desc.children",
            Self::OpaqueName => "std/type-desc.opaque_name",
            Self::Resolve => "std/type-desc.resolve",
        }
    }

    pub(crate) const fn arity(self) -> usize {
        1
    }
}

impl CoreCodecFunction {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Decode => "std/codec.decode",
            Self::Encode => "std/codec.encode",
        }
    }

    pub(crate) const fn arity(self) -> usize {
        2
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoreResultFunction {
    Unwrap,
}

impl CoreResultFunction {
    pub(crate) const fn name(self) -> &'static str {
        "std/result.unwrap"
    }

    pub(crate) const fn arity(self) -> usize {
        1
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoreJsonFunction {
    Parse,
    Decode,
    Stringify,
    StringifyPretty,
    StringifyPrettyValue,
    Rename,
    RenameDecorator,
    RenameAll,
    RenameAllDecorator,
    Flatten,
    Untagged,
    Schema,
    Default,
    DefaultDecorator,
    SkipSerializingIf,
    SkipSerializingIfDecorator,
}

impl CoreJsonFunction {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Parse => "std/json.parse",
            Self::Decode => "std/json.decode",
            Self::Stringify => "std/json.stringify",
            Self::StringifyPretty => "std/json.stringify_pretty",
            Self::StringifyPrettyValue => "std/json.stringify_pretty.configured",
            Self::Rename => "std/json.rename",
            Self::RenameDecorator => "std/json.rename.configured",
            Self::RenameAll => "std/json.rename_all",
            Self::RenameAllDecorator => "std/json.rename_all.configured",
            Self::Flatten => "std/json.flatten",
            Self::Untagged => "std/json.untagged",
            Self::Schema => "std/json.schema",
            Self::Default => "std/json.default",
            Self::DefaultDecorator => "std/json.default.configured",
            Self::SkipSerializingIf => "std/json.skip_serializing_if",
            Self::SkipSerializingIfDecorator => "std/json.skip_serializing_if.configured",
        }
    }

    pub(crate) const fn arity(self) -> usize {
        match self {
            Self::Decode => 2,
            Self::Flatten
            | Self::Untagged
            | Self::RenameDecorator
            | Self::RenameAllDecorator
            | Self::DefaultDecorator
            | Self::SkipSerializingIfDecorator => 2,
            _ => 1,
        }
    }
}

impl CoreDictFunction {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Get => "std/dict.get",
            Self::Keys => "std/dict.keys",
            Self::Values => "std/dict.values",
            Self::Pairs => "std/dict.pairs",
            Self::FromPairs => "std/dict.from_pairs",
            Self::Merge => "std/dict.merge",
            Self::MapValues => "std/dict.map_values",
            Self::Filter => "std/dict.filter",
            Self::Fold => "std/dict.fold",
        }
    }

    pub(crate) const fn arity(self) -> usize {
        match self {
            Self::Keys | Self::Values | Self::Pairs | Self::FromPairs => 1,
            Self::Get => 2,
            Self::Merge | Self::MapValues | Self::Filter => 2,
            Self::Fold => 3,
        }
    }
}

impl CoreStringFunction {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Length => "std/string.length",
            Self::Chars => "std/string.chars",
            Self::Join => "std/string.join",
            Self::JoinLines => "std/string.join_lines",
            Self::Split => "std/string.split",
            Self::Lines => "std/string.lines",
            Self::StartsWith => "std/string.starts_with",
            Self::EndsWith => "std/string.ends_with",
            Self::Contains => "std/string.contains",
            Self::Replace => "std/string.replace",
            Self::Indent => "std/string.indent",
            Self::EnsureTrailingNewline => "std/string.ensure_trailing_newline",
            Self::TrimMargin => "std/string.trim_margin",
        }
    }

    pub(crate) const fn arity(self) -> usize {
        match self {
            Self::Length
            | Self::Chars
            | Self::JoinLines
            | Self::Lines
            | Self::EnsureTrailingNewline => 1,
            Self::Join
            | Self::Split
            | Self::StartsWith
            | Self::EndsWith
            | Self::Contains
            | Self::Indent
            | Self::TrimMargin => 2,
            Self::Replace => 3,
        }
    }
}

impl CorePathFunction {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Join => "std/path.join",
            Self::Normalize => "std/path.normalize",
            Self::Parent => "std/path.parent",
            Self::FileName => "std/path.file_name",
        }
    }

    pub(crate) const fn arity(self) -> usize {
        1
    }
}

impl CoreArrayFunction {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Length => "std/array.length",
            Self::Get => "std/array.get",
            Self::Enumerate => "std/array.enumerate",
            Self::Push => "std/array.push",
            Self::Concat => "std/array.concat",
            Self::Zip => "std/array.zip",
            Self::Map => "std/array.map",
            Self::Filter => "std/array.filter",
            Self::FlatMap => "std/array.flat_map",
            Self::Fold => "std/array.fold",
            Self::FoldControl => "std/array.fold_control",
            Self::Any => "std/array.any",
            Self::All => "std/array.all",
            Self::Find => "std/array.find",
        }
    }

    pub(crate) const fn arity(self) -> usize {
        match self {
            Self::Length | Self::Enumerate | Self::Concat => 1,
            Self::Get | Self::Push | Self::Zip => 2,
            Self::Map | Self::Filter | Self::FlatMap | Self::Any | Self::All | Self::Find => 2,
            Self::Fold | Self::FoldControl => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NativeKind {
    Synchronous,
    CoreArray(CoreArrayFunction),
    CoreAttributes(CoreAttributesFunction),
    CoreModel(CoreModelFunction),
    CoreBuiltinType(CoreBuiltinTypeFunction),
    CoreDict(CoreDictFunction),
    CoreString(CoreStringFunction),
    CorePath(CorePathFunction),
    CoreDiagnostic(CoreDiagnosticFunction),
    CoreHash(CoreHashFunction),
    CoreCodec(CoreCodecFunction),
    CoreTypeDesc(CoreTypeDescFunction),
    CoreDyn(CoreDynFunction),
    CoreEq(CoreEqFunction),
    CoreResult(CoreResultFunction),
    CoreJson(CoreJsonFunction),
}

#[derive(Clone, Copy)]
pub struct NativeFunction {
    name: &'static str,
    arity: usize,
    callback: NativeCallback,
    kind: NativeKind,
    native_type_local: Option<u32>,
}

impl NativeFunction {
    pub const fn new(name: &'static str, arity: usize, callback: NativeCallback) -> Self {
        Self {
            name,
            arity,
            callback,
            kind: NativeKind::Synchronous,
            native_type_local: None,
        }
    }

    pub const fn new_with_native_type(
        name: &'static str,
        arity: usize,
        native_type_local: u32,
        callback: NativeCallback,
    ) -> Self {
        Self {
            name,
            arity,
            callback,
            kind: NativeKind::Synchronous,
            native_type_local: Some(native_type_local),
        }
    }

    pub(crate) const fn core_array(function: CoreArrayFunction) -> Self {
        Self {
            name: function.name(),
            arity: function.arity(),
            callback: unavailable_core_callback,
            kind: NativeKind::CoreArray(function),
            native_type_local: None,
        }
    }

    pub(crate) const fn core_attributes(function: CoreAttributesFunction) -> Self {
        Self {
            name: function.name(),
            arity: function.arity(),
            callback: unavailable_core_callback,
            kind: NativeKind::CoreAttributes(function),
            native_type_local: None,
        }
    }

    pub(crate) const fn core_model(function: CoreModelFunction) -> Self {
        Self {
            name: function.name(),
            arity: function.arity(),
            callback: unavailable_core_callback,
            kind: NativeKind::CoreModel(function),
            native_type_local: None,
        }
    }

    pub(crate) const fn core_builtin_type(function: CoreBuiltinTypeFunction) -> Self {
        Self {
            name: function.name(),
            arity: function.arity(),
            callback: unavailable_core_callback,
            kind: NativeKind::CoreBuiltinType(function),
            native_type_local: None,
        }
    }

    pub(crate) const fn core_dict(function: CoreDictFunction) -> Self {
        Self {
            name: function.name(),
            arity: function.arity(),
            callback: unavailable_core_callback,
            kind: NativeKind::CoreDict(function),
            native_type_local: None,
        }
    }

    pub(crate) const fn core_string(function: CoreStringFunction) -> Self {
        Self {
            name: function.name(),
            arity: function.arity(),
            callback: unavailable_core_callback,
            kind: NativeKind::CoreString(function),
            native_type_local: None,
        }
    }

    pub(crate) const fn core_path(function: CorePathFunction) -> Self {
        Self {
            name: function.name(),
            arity: function.arity(),
            callback: unavailable_core_callback,
            kind: NativeKind::CorePath(function),
            native_type_local: None,
        }
    }

    pub(crate) const fn core_diagnostic(function: CoreDiagnosticFunction) -> Self {
        Self {
            name: function.name(),
            arity: function.arity(),
            callback: unavailable_core_callback,
            kind: NativeKind::CoreDiagnostic(function),
            native_type_local: None,
        }
    }

    pub(crate) const fn core_hash(function: CoreHashFunction) -> Self {
        Self {
            name: function.name(),
            arity: function.arity(),
            callback: unavailable_core_callback,
            kind: NativeKind::CoreHash(function),
            native_type_local: None,
        }
    }

    pub(crate) const fn core_codec(function: CoreCodecFunction) -> Self {
        Self {
            name: function.name(),
            arity: function.arity(),
            callback: unavailable_core_callback,
            kind: NativeKind::CoreCodec(function),
            native_type_local: None,
        }
    }

    pub(crate) const fn core_type_desc(function: CoreTypeDescFunction) -> Self {
        Self {
            name: function.name(),
            arity: function.arity(),
            callback: unavailable_core_callback,
            kind: NativeKind::CoreTypeDesc(function),
            native_type_local: None,
        }
    }

    pub(crate) const fn core_dyn(function: CoreDynFunction) -> Self {
        Self {
            name: function.name(),
            arity: function.arity(),
            callback: unavailable_core_callback,
            kind: NativeKind::CoreDyn(function),
            native_type_local: None,
        }
    }

    pub(crate) const fn core_eq(function: CoreEqFunction) -> Self {
        Self {
            name: function.name(),
            arity: function.arity(),
            callback: unavailable_core_callback,
            kind: NativeKind::CoreEq(function),
            native_type_local: None,
        }
    }

    pub(crate) const fn core_result(function: CoreResultFunction) -> Self {
        Self {
            name: function.name(),
            arity: function.arity(),
            callback: unavailable_core_callback,
            kind: NativeKind::CoreResult(function),
            native_type_local: None,
        }
    }

    pub(crate) const fn core_json(function: CoreJsonFunction) -> Self {
        Self {
            name: function.name(),
            arity: function.arity(),
            callback: unavailable_core_callback,
            kind: NativeKind::CoreJson(function),
            native_type_local: None,
        }
    }

    pub const fn name(self) -> &'static str {
        self.name
    }

    pub const fn arity(self) -> usize {
        self.arity
    }

    pub const fn callback(self) -> NativeCallback {
        self.callback
    }

    pub(crate) const fn kind(self) -> NativeKind {
        self.kind
    }

    pub(crate) const fn native_type_local(self) -> Option<u32> {
        self.native_type_local
    }
}

fn unavailable_core_callback(_: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    Err(NativeError::new(
        "VM-managed core function cannot use the synchronous native ABI",
    ))
}

impl fmt::Debug for NativeFunction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeFunction")
            .field("name", &self.name)
            .field("arity", &self.arity)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub enum Prototype {
    Bytecode(Arc<BytecodeFunction>),
    Native(NativeFunction),
}

pub type Callable = Prototype;

impl Closure {
    pub(crate) fn from_parts(prototype: Prototype, upvalues: Vec<Value>) -> Self {
        Self {
            identity: Arc::new(()),
            prototype,
            upvalues: upvalues.into(),
        }
    }

    pub(crate) fn from_parts_with_identity(
        identity: Arc<()>,
        prototype: Prototype,
        upvalues: Vec<Value>,
    ) -> Self {
        Self {
            identity,
            prototype,
            upvalues: upvalues.into(),
        }
    }

    pub fn new(function: Arc<BytecodeFunction>, captures: Vec<Value>) -> Self {
        Self::from_parts(Prototype::Bytecode(function), captures)
    }

    pub fn native(function: NativeFunction) -> Self {
        Self::native_with_upvalues(function, Vec::new())
    }

    pub fn native_with_upvalues(function: NativeFunction, upvalues: Vec<Value>) -> Self {
        Self::from_parts(Prototype::Native(function), upvalues)
    }

    pub fn prototype(&self) -> &Prototype {
        &self.prototype
    }

    pub fn upvalues(&self) -> &[Value] {
        &self.upvalues
    }

    pub(crate) fn identity(&self) -> &Arc<()> {
        &self.identity
    }
}

impl Dict {
    pub(crate) fn new(shape: Arc<Shape>, values: Vec<Value>) -> Self {
        debug_assert_eq!(shape.fields().len(), values.len());
        Self {
            shape,
            values: values.into(),
        }
    }

    pub fn shape(&self) -> &Arc<Shape> {
        &self.shape
    }

    pub fn values(&self) -> &[Value] {
        &self.values
    }

    pub fn get(&self, field: &str) -> Option<&Value> {
        self.shape
            .field_index(field)
            .map(|index| &self.values[index])
    }

    pub fn shares_shape_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shape, &other.shape)
    }
}

#[derive(Clone)]
pub struct DynValue {
    identity: Arc<()>,
    descriptor: Box<Value>,
    value: Box<Value>,
    scheme: Option<crate::TypeScheme>,
    origin: Option<Arc<str>>,
}

impl DynValue {
    pub(crate) fn from_module_export(
        descriptor: Value,
        value: Value,
        scheme: crate::TypeScheme,
        origin: impl Into<Arc<str>>,
    ) -> Self {
        Self {
            identity: Arc::new(()),
            descriptor: Box::new(descriptor),
            value: Box::new(value),
            scheme: Some(scheme),
            origin: Some(origin.into()),
        }
    }

    pub(crate) fn from_parts_with_metadata(
        identity: Arc<()>,
        descriptor: Value,
        value: Value,
        scheme: Option<crate::TypeScheme>,
        origin: Option<Arc<str>>,
    ) -> Self {
        Self {
            identity,
            descriptor: Box::new(descriptor),
            value: Box::new(value),
            scheme,
            origin,
        }
    }

    pub(crate) fn identity(&self) -> &Arc<()> {
        &self.identity
    }

    pub(crate) fn descriptor(&self) -> &Value {
        &self.descriptor
    }

    pub(crate) fn value(&self) -> &Value {
        &self.value
    }

    pub(crate) fn scheme(&self) -> Option<&crate::TypeScheme> {
        self.scheme.as_ref()
    }

    pub(crate) fn origin(&self) -> Option<&str> {
        self.origin.as_deref()
    }
}

#[derive(Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    String(Arc<str>),
    Bytes(Arc<[u8]>),
    NativeType(NativeType),
    Opaque(OpaqueValue),
    Dict(Dict),
    Array(Arc<[Value]>),
    Atom(Atom),
    Tagged { tag: Atom, payload: Box<Value> },
    Tuple(Arc<[Value]>),
    Func(Arc<Closure>),
    Dyn(Arc<DynValue>),
}

impl Value {
    pub const fn bool(value: bool) -> Self {
        Self::Atom(Atom::Builtin(if value {
            BuiltinAtom::True
        } else {
            BuiltinAtom::False
        }))
    }

    pub const fn none() -> Self {
        Self::Atom(Atom::Builtin(BuiltinAtom::None))
    }

    pub fn string(value: impl Into<Arc<str>>) -> Self {
        Self::String(value.into())
    }

    pub fn atom(name: impl Into<Arc<str>>) -> Self {
        Self::Atom(Atom::named(name))
    }

    pub fn tagged(tag: Atom, payload: Value) -> Self {
        Self::Tagged {
            tag,
            payload: Box::new(payload),
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Int(_) => "Int",
            Self::Float(_) => "Float",
            Self::String(_) => "String",
            Self::Bytes(_) => "Bytes",
            Self::NativeType(_) => "Type",
            Self::Opaque(_) => "Opaque",
            Self::Dict(_) => "Dict",
            Self::Array(_) => "Array",
            Self::Atom(_) => "Atom",
            Self::Tagged { .. } => "Tagged",
            Self::Tuple(_) => "Tuple",
            Self::Func(_) => "Func",
            Self::Dyn(_) => "Dyn",
        }
    }

    pub fn to_telora_literal(&self) -> Result<String, String> {
        fn render(value: &Value, output: &mut String) -> Result<(), String> {
            match value {
                Value::Int(value) => output.push_str(&value.to_string()),
                Value::Float(value) if value.is_finite() => output.push_str(&format!("{value:?}")),
                Value::Float(_) => return Err("non-finite Float has no Telora literal".into()),
                Value::String(value) => output.push_str(&format!("{value:?}")),
                Value::Bytes(value) => {
                    output.push_str("b\"");
                    for byte in value.iter() {
                        output.push_str(&format!("\\x{byte:02x}"));
                    }
                    output.push('"');
                }
                Value::Dict(dict) => {
                    output.push('{');
                    for (index, (field, value)) in
                        dict.shape().fields().iter().zip(dict.values()).enumerate()
                    {
                        if !is_telora_identifier(field) {
                            return Err(format!(
                                "Dict field {field:?} cannot be represented as a Telora literal"
                            ));
                        }
                        if index > 0 {
                            output.push_str(", ");
                        }
                        output.push_str(field);
                        output.push_str(": ");
                        render(value, output)?;
                    }
                    output.push('}');
                }
                Value::Array(values) => render_values("[", "]", values, output)?,
                Value::Atom(atom) => {
                    if !is_telora_identifier(atom.name()) {
                        return Err(format!(
                            "Atom {:?} cannot be represented as a Telora literal",
                            atom.name()
                        ));
                    }
                    output.push('\'');
                    output.push_str(atom.name());
                }
                Value::Tagged { tag, payload } => {
                    if !is_telora_identifier(tag.name()) {
                        return Err(format!(
                            "Tagged constructor {:?} cannot be represented as a Telora literal",
                            tag.name()
                        ));
                    }
                    output.push('\'');
                    output.push_str(tag.name());
                    output.push('(');
                    render(payload, output)?;
                    output.push(')');
                }
                Value::Tuple(values) => render_values("(", ")", values, output)?,
                Value::NativeType(_) | Value::Opaque(_) | Value::Func(_) | Value::Dyn(_) => {
                    return Err(format!(
                        "{} cannot be represented as a Telora literal",
                        value.type_name()
                    ));
                }
            }
            Ok(())
        }

        fn render_values(
            start: &str,
            end: &str,
            values: &[Value],
            output: &mut String,
        ) -> Result<(), String> {
            output.push_str(start);
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push_str(", ");
                }
                render(value, output)?;
            }
            if start == "(" && values.len() == 1 {
                output.push(',');
            }
            output.push_str(end);
            Ok(())
        }

        fn is_telora_identifier(value: &str) -> bool {
            let mut characters = value.chars();
            characters
                .next()
                .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
                && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
        }

        let mut output = String::new();
        render(self, &mut output)?;
        Ok(output)
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(value) => write!(formatter, "{value}"),
            Self::Float(value) => write!(formatter, "{value:?}"),
            Self::String(value) => write!(formatter, "{value:?}"),
            Self::Bytes(value) => {
                write!(formatter, "b\"")?;
                for byte in value.iter() {
                    write!(formatter, "\\x{byte:02x}")?;
                }
                write!(formatter, "\"")
            }
            Self::Opaque(value) => write!(formatter, "{value:?}"),
            Self::NativeType(value) => write!(formatter, "<type {}>", value.qualified_name()),
            Self::Dict(dict) => {
                write!(formatter, "{{")?;
                for (index, (field, value)) in
                    dict.shape().fields().iter().zip(dict.values()).enumerate()
                {
                    if index > 0 {
                        write!(formatter, ", ")?;
                    }
                    write!(formatter, "{field}: {value}")?;
                }
                write!(formatter, "}}")
            }
            Self::Array(values) => format_sequence(formatter, "[", "]", values),
            Self::Atom(atom) => write!(formatter, "'{}", atom.name()),
            Self::Tagged { tag, payload } => write!(formatter, "'{}({payload})", tag.name()),
            Self::Tuple(values) => format_sequence(formatter, "(", ")", values),
            Self::Func(closure) => match closure.prototype() {
                Prototype::Bytecode(function) => {
                    write!(formatter, "<fn {}>", function.name())
                }
                Prototype::Native(function) => write!(formatter, "<native fn {}>", function.name()),
            },
            Self::Dyn(_) => formatter.write_str("<dyn>"),
        }
    }
}

fn format_sequence(
    formatter: &mut fmt::Formatter<'_>,
    start: &str,
    end: &str,
    values: &[Value],
) -> fmt::Result {
    write!(formatter, "{start}")?;
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            write!(formatter, ", ")?;
        }
        write!(formatter, "{value}")?;
    }
    write!(formatter, "{end}")
}
