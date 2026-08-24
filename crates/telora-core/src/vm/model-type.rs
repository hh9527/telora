#[allow(clippy::too_many_arguments)]
fn run_core_union_model(
    variants: Val,
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let DecodedValue::Array(handle) = variants.value() else {
        let view = HeapView {
            current,
            background: Some(background),
        };
        return Err(runtime_type_error(
            "variants Array",
            &variants,
            &view,
            function,
            pc,
        ));
    };
    let view = HeapView {
        current,
        background: Some(background),
    };
    let variants = view
        .sequence(handle, false)
        .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
        .to_vec();
    if variants.is_empty() {
        return Err(error(
            RuntimeErrorKind::TypeMismatch,
            "union requires at least one variant",
            function,
            pc,
        ));
    }
    let mut normalized = Vec::with_capacity(variants.len());
    for (index, variant) in variants.into_iter().enumerate() {
        let path = format!("variants[{index}]");
        let (inner, attributes) =
            flatten_attributes(variant, &path, function, pc, current, background)?;
        if !matches!(inner.value(), DecodedValue::TypeSlot(_)) {
            decode_runtime_type_at(inner, &path, current, background)
                .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
        }
        normalized.push(allocate_attributes_wrapper(
            inner,
            attributes,
            variant.loc().or(instruction_location(function, pc)),
            function,
            pc,
            current,
            account,
        )?);
    }
    charge_allocation(
        account,
        logical_value_bytes(normalized.len())
            .map_err(|native_error| allocation_error(native_error.message, function, pc))?,
        function,
        pc,
    )?;
    let variants = Val::new(
        DecodedValue::Array(current.allocate(Object::Array(normalized.into()))),
        instruction_location(function, pc),
    );
    let metadata = allocate_core_dict(
        vec![
            (
                "kind".into(),
                Val::new(
                    DecodedValue::Atom(current.intern("Union")),
                    instruction_location(function, pc),
                ),
            ),
            ("variants".into(), variants),
        ],
        function,
        pc,
        current,
        account,
    )?;
    let value = allocate_attributes_wrapper(
        metadata,
        BTreeMap::new(),
        instruction_location(function, pc),
        function,
        pc,
        current,
        account,
    )?;
    Ok(VmAction::Return {
        value,
        return_target,
    })
}

#[allow(clippy::too_many_arguments)]
fn run_core_builtin_type(
    operation: CoreBuiltinTypeFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let variants = match operation {
        CoreBuiltinTypeFunction::FoldControl => vec![
            ("Break".to_owned(), Some(arguments[1])),
            ("Continue".to_owned(), Some(arguments[0])),
        ],
        CoreBuiltinTypeFunction::Option => vec![
            ("None".to_owned(), None),
            ("Some".to_owned(), Some(arguments[0])),
        ],
        CoreBuiltinTypeFunction::Result => vec![
            ("Err".to_owned(), Some(arguments[1])),
            ("Ok".to_owned(), Some(arguments[0])),
        ],
    };
    let value = allocate_builtin_enum(variants, function, pc, current, background, account)?;
    Ok(VmAction::Return {
        value,
        return_target,
    })
}

#[allow(clippy::too_many_arguments)]
fn allocate_builtin_enum(
    variants: Vec<(String, Option<Val>)>,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<Val, RuntimeError> {
    let loc = instruction_location(function, pc);
    let mut normalized = Vec::with_capacity(variants.len());
    for (name, payload) in variants {
        let path = format!("variants.{name}");
        let (inner, attributes) = if let Some(payload) = payload {
            let (inner, attributes) =
                flatten_attributes(payload, &path, function, pc, current, background)?;
            if !matches!(inner.value(), DecodedValue::TypeSlot(_)) {
                decode_runtime_type_at(inner, &path, current, background).map_err(|message| {
                    error(RuntimeErrorKind::TypeMismatch, message, function, pc)
                })?;
            }
            (inner, attributes)
        } else {
            (
                Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), loc),
                BTreeMap::new(),
            )
        };
        let variant = allocate_attributes_wrapper(
            inner,
            attributes,
            inner.loc().or(loc),
            function,
            pc,
            current,
            account,
        )?;
        normalized.push((name, variant));
    }
    let variants = allocate_core_dict(normalized, function, pc, current, account)?;
    let metadata = allocate_core_dict(
        vec![
            (
                "kind".into(),
                Val::new(DecodedValue::Atom(current.intern("Enum")), loc),
            ),
            ("variants".into(), variants),
        ],
        function,
        pc,
        current,
        account,
    )?;
    allocate_attributes_wrapper(
        metadata,
        BTreeMap::new(),
        loc,
        function,
        pc,
        current,
        account,
    )
}

fn validate_model_context(
    context: Val,
    function: &BytecodeFunction,
    pc: usize,
    current: &Heap,
    background: &Heap,
) -> Result<(), RuntimeError> {
    let view = HeapView {
        current,
        background: Some(background),
    };
    if view
        .atom_text(context)
        .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
        .is_some_and(|atom| atom == "None")
    {
        return Ok(());
    }
    let DecodedValue::Dict(handle) = context.value() else {
        return Err(error(
            RuntimeErrorKind::TypeMismatch,
            "model context must be 'None or a Type context",
            function,
            pc,
        ));
    };
    let fields = view
        .dict_fields(handle)
        .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?;
    let kind = view
        .dict_get_text(handle, "kind")
        .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
        .and_then(|value| view.atom_text(value).ok().flatten());
    let name = view
        .dict_get_text(handle, "name")
        .map_err(|heap_error| core_dict_heap_error(heap_error, function, pc))?
        .and_then(|value| view.string_text(value).ok().flatten());
    if fields == ["kind", "name"] && kind.is_some_and(|kind| kind == "Type") && name.is_some() {
        Ok(())
    } else {
        Err(error(
            RuntimeErrorKind::TypeMismatch,
            "model context must be 'None or {kind: 'Type, name: String}",
            function,
            pc,
        ))
    }
}

#[derive(Clone, Debug)]
struct CodecType {
    kind: CodecKind,
    rule: Val,
    json_rename_all: Option<Val>,
    json_untagged: Option<Val>,
    declared_owner: Option<Val>,
}

#[derive(Clone, Copy, Debug)]
struct CodecProperties {
    parse_by: crate::TypeId,
    decode_by_parse: crate::TypeId,
    encode_by_display: crate::TypeId,
    display_by: crate::TypeId,
    json_rename_all: Option<crate::TypeId>,
    json_untagged: Option<crate::TypeId>,
}

#[derive(Clone, Debug)]
enum CodecKind {
    TypeSlot(Handle),
    TypeRef(Handle),
    Any,
    Type,
    Dyn,
    Int,
    Float,
    String,
    Bytes,
    Opaque,
    Atom(String),
    Array(Box<CodecType>),
    Dict(Box<CodecType>),
    Tagged {
        tag: String,
        payload: Box<CodecType>,
    },
    Tuple(Vec<CodecType>),
    Struct(BTreeMap<String, CodecType>),
    Enum(BTreeMap<String, CodecEnumVariant>),
    Union(Vec<CodecType>),
    Function,
}

#[derive(Clone, Debug)]
struct CodecEnumVariant {
    payload: Option<Box<CodecType>>,
    rule: Val,
}

#[derive(Clone, Debug)]
enum CodecNode {
    Existing(Val),
    SemanticValue {
        owner: Val,
        raw: Box<Self>,
    },
    Declared {
        owner: Val,
        payload: Box<Self>,
        loc: Option<crate::Loc>,
    },
    Atom(BuiltinAtom, Option<crate::Loc>),
    NamedAtom(String, Option<crate::Loc>),
    Array(Vec<Self>, Option<crate::Loc>),
    Tuple(Vec<Self>, Option<crate::Loc>),
    Tagged {
        tag: Box<Self>,
        payload: Box<Self>,
        loc: Option<crate::Loc>,
    },
    Dict(Vec<(String, Self)>, Option<crate::Loc>),
    String(String, Option<crate::Loc>),
    PreparedDisplay {
        function: Val,
        descriptor: Val,
        value: Val,
        loc: Option<crate::Loc>,
    },
}

#[derive(Clone, Copy)]
enum CodecDirection {
    Decode,
    Encode,
}

#[derive(Clone, Debug)]
struct CodecFailure {
    message: String,
    data: Val,
    rule: Val,
}

impl CodecFailure {
    fn new(message: impl Into<String>, data: Val, rule: Val) -> Self {
        Self {
            message: message.into(),
            data,
            rule,
        }
    }
}

#[derive(Debug)]
enum JsonEncodeInput {
    Typed {
        schema: CodecType,
        properties: CodecProperties,
        value: Val,
    },
    Dynamic {
        properties: CodecProperties,
        value: Val,
    },
}
