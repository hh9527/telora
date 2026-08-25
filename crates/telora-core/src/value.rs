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

type OpaquePayload = dyn Any + Send + Sync;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct NativeModuleId(pub(crate) u32);

pub(crate) const RESERVED_NATIVE_MODULE_MAX: u32 = 1023;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct NativeTypeId {
    pub(crate) module: NativeModuleId,
    pub(crate) local: u32,
}

#[derive(Clone, Debug)]
pub struct NativeType {
    id: NativeTypeId,
    qualified_name: Arc<str>,
}

#[derive(Clone, Debug)]
pub struct DeclaredTypeId {
    module: crate::ModuleId,
    declaration: u32,
    arguments: Arc<[crate::types::TypeDescriptor]>,
    argument_ids: Arc<[crate::types::TypeExprId]>,
}

impl DeclaredTypeId {
    pub(crate) fn concrete(module: crate::ModuleId, declaration: u32) -> Self {
        Self {
            module,
            declaration,
            arguments: Arc::new([]),
            argument_ids: Arc::new([]),
        }
    }

    pub(crate) fn applied(
        module: crate::ModuleId,
        declaration: u32,
        arguments: &[crate::types::TypeDescriptor],
    ) -> Self {
        Self {
            module,
            declaration,
            arguments: arguments.into(),
            argument_ids: arguments
                .iter()
                .map(crate::types::TypeExprId::from_descriptor)
                .collect::<Vec<_>>()
                .into(),
        }
    }

    pub(crate) fn reapply(&self, arguments: &[crate::types::TypeDescriptor]) -> Self {
        Self::applied(self.module, self.declaration, arguments)
    }

    pub(crate) fn arguments(&self) -> &[crate::types::TypeDescriptor] {
        &self.arguments
    }

    pub(crate) fn constructor(&self) -> crate::TypeConstructorId {
        crate::TypeConstructorId {
            module: self.module,
            local: self.declaration,
        }
    }

    pub(crate) fn has_same_head(&self, other: &Self) -> bool {
        self.module == other.module && self.declaration == other.declaration
    }
}

impl PartialEq for DeclaredTypeId {
    fn eq(&self, other: &Self) -> bool {
        self.module == other.module
            && self.declaration == other.declaration
            && self.argument_ids == other.argument_ids
    }
}

impl Eq for DeclaredTypeId {}

impl std::hash::Hash for DeclaredTypeId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.module.hash(state);
        self.declaration.hash(state);
        self.argument_ids.hash(state);
    }
}

impl PartialOrd for DeclaredTypeId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DeclaredTypeId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (&self.module, self.declaration, &self.argument_ids).cmp(&(
            &other.module,
            other.declaration,
            &other.argument_ids,
        ))
    }
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

    pub(crate) fn id(&self) -> NativeTypeId {
        self.id
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
            Self::Struct => "\0telora_struct",
            Self::Enum => "\0telora_enum",
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoreRuntimeFunction {
    CallWithDiagnostics,
}

impl CoreRuntimeFunction {
    pub(crate) const fn name(self) -> &'static str {
        "std/_rt.call_with_diagnostics"
    }

    pub(crate) const fn arity(self) -> usize {
        2
    }
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
    Fields,
    Variants,
    OpaqueName,
    Resolve,
    StripAttributes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoreDynFunction {
    Pack,
    ProjectWith,
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
    GetFieldValue,
    GetVariantIndex,
    GetVariantPayload,
}

impl CoreDynFunction {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Pack => "std/dyn.pack",
            Self::ProjectWith => "std/dyn.project_with",
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
            Self::GetFieldValue => "std/dyn.get_field_value",
            Self::GetVariantIndex => "std/dyn.get_variant_index",
            Self::GetVariantPayload => "std/dyn.get_variant_payload",
        }
    }

    pub(crate) const fn arity(self) -> usize {
        match self {
            Self::Pack
            | Self::ProjectWith
            | Self::Field
            | Self::GetFieldValue
            | Self::GetVariantPayload => 2,
            _ => 1,
        }
    }
}

impl CoreTypeDescFunction {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Kind => "std/type-desc.kind",
            Self::Children => "std/type-desc.children",
            Self::Fields => "std/type-desc.fields",
            Self::Variants => "std/type-desc.variants",
            Self::OpaqueName => "std/type-desc.opaque_name",
            Self::Resolve => "std/type-desc.resolve",
            Self::StripAttributes => "std/type-desc.strip_attributes",
        }
    }

    pub(crate) const fn arity(self) -> usize {
        1
    }
}

impl CoreCodecFunction {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Decode => "std/codec.decode_with",
            Self::Encode => "std/codec.encode_with",
        }
    }

    pub(crate) const fn arity(self) -> usize {
        3
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
    ParseYaml,
    ParseToml,
    Decode,
    Stringify,
    StringifyPretty,
    StringifyPrettyValue,
    Schema,
}

impl CoreJsonFunction {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Parse => "std/json.parse",
            Self::ParseYaml => "std/yaml.parse",
            Self::ParseToml => "std/toml.parse",
            Self::Decode => "std/json.decode_with",
            Self::Stringify => "std/json.stringify",
            Self::StringifyPretty => "std/json.stringify_pretty",
            Self::StringifyPrettyValue => "std/json.stringify_pretty.configured",
            Self::Schema => "std/json.schema_with",
        }
    }

    pub(crate) const fn arity(self) -> usize {
        match self {
            Self::Parse | Self::ParseYaml | Self::ParseToml => 2,
            Self::Decode => 3,
            Self::Schema => 2,
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
            Self::Length | Self::JoinLines | Self::Lines | Self::EnsureTrailingNewline => 1,
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
    CoreRuntime(CoreRuntimeFunction),
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

    pub(crate) const fn core_runtime(function: CoreRuntimeFunction) -> Self {
        Self {
            name: function.name(),
            arity: function.arity(),
            callback: unavailable_core_callback,
            kind: NativeKind::CoreRuntime(function),
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
