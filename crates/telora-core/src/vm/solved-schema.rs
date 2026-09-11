// Schema generation walks solved IDs and builds Value directly in the VM.
// The work stack is also the continuation when a property initializer is needed.
#[derive(Debug)]
enum SolvedSchemaTask {
    Visit(crate::mir::TypeId, bool),
    String(String),
    Int(i64),
    Bool(bool),
    Array(usize),
    Object(Vec<String>),
    Definition(usize),
    Finish,
}

#[derive(Debug)]
struct SolvedSchema {
    pending: Vec<SolvedSchemaTask>,
    output: Vec<Val>,
    links: Vec<Option<usize>>,
    definitions: Vec<Option<Val>>,
    properties: Vec<(&'static str, crate::mir::TypeId)>,
    target: crate::mir::TypeId,
    location: Option<crate::Loc>,
    return_target: ReturnTarget,
    trace_frame: RuntimeFrame,
    function: Arc<BytecodeFunction>,
    pc: usize,
}

impl NativeContinuation for SolvedSchema {
    fn return_target(&self) -> &ReturnTarget { &self.return_target }
    fn trace_frame(&self) -> &RuntimeFrame { &self.trace_frame }
    fn resume(self: Box<Self>, _: Val, current: &mut Heap, background: &Heap, account: &mut QuotaAccount) -> Result<VmAction, RuntimeError> {
        continue_solved_schema(*self, current, background, account)
    }
    fn resume_failed(self: Box<Self>, failure: Val, _: &mut Heap, _: &Heap, _: &mut QuotaAccount) -> Result<VmAction, RuntimeError> {
        Ok(VmAction::Return { value: failure, return_target: self.return_target })
    }
}

impl SolvedSchema {
    fn schedule(&mut self, steps: Vec<SolvedSchemaTask>) {
        self.pending.extend(steps.into_iter().rev());
    }
}

fn schema_object(names: &[&str]) -> SolvedSchemaTask {
    SolvedSchemaTask::Object(names.iter().map(|name| (*name).into()).collect())
}

fn solved_schema_kind(kind: &str) -> Vec<SolvedSchemaTask> {
    vec![SolvedSchemaTask::String(kind.into()), schema_object(&["type"])]
}

fn solved_schema_ref(index: usize) -> Vec<SolvedSchemaTask> {
    vec![SolvedSchemaTask::String(format!("#/$defs/Type{index}")), schema_object(&["$ref"])]
}

fn run_solved_json_schema(
    arguments: &[Val], return_target: ReturnTarget, function: &BytecodeFunction, pc: usize,
    current: &mut Heap, background: &Heap, account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let types = background.solved_types.as_ref().expect("schema type image");
    let owner = solved_metadata_id(arguments[1], types, function, pc)?;
    let target = solved_metadata_id(arguments[2], types, function, pc)?;
    let input = ValueRef { value: arguments[0], view: HeapView { current, background: Some(background) } };
    let properties = ["decode_by_parse", "encode_by_display", "json_rename_all", "json_untagged"]
        .into_iter().map(|name| {
            let value = input.dict_get(name).ok_or_else(|| error(RuntimeErrorKind::InvalidBytecode,
                "schema property contract is missing a field", function, pc))?;
            Ok((name, solved_metadata_id(value.value, types, function, pc)?))
        }).collect::<Result<Vec<_>, RuntimeError>>()?;
    continue_solved_schema(SolvedSchema {
        pending: vec![SolvedSchemaTask::Finish, SolvedSchemaTask::Visit(owner, false)],
        output: vec![], links: vec![None; types.types.len()], definitions: vec![], properties,
        target, location: arguments[1].loc().or_else(|| instruction_location(function, pc)),
        return_target,
        trace_frame: RuntimeFrame { function: function.name().to_owned(), instruction: pc, origin: function.origin_at(pc) },
        function: Arc::new(function.clone()), pc,
    }, current, background, account)
}

fn continue_solved_schema(
    mut state: SolvedSchema, current: &mut Heap, background: &Heap, account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    use crate::execution_graph::{EvaluationError, PropertyKey, Request};
    use crate::mir::{PropertySite, TypeConstructor as T};
    use SolvedSchemaTask as S;
    let types = background.solved_types.as_ref().expect("schema type image");
    let graph = background.solved_graph.as_ref().expect("schema execution graph");
    while let Some(task) = state.pending.pop() {
        let function = Arc::clone(&state.function);
        let pc = state.pc;
        let loc = state.location;
        consume_fuel(account, &function, pc)?;
        let ty = match task {
            S::String(text) => {
                charge_allocation(account, text.len() as u64, &function, pc)?;
                let payload = Val::new(current.string(Some(background), &text), loc);
                state.output.push(solved_codec_tag("String", payload, state.target, loc, current, background, account, &function, pc)?);
                continue;
            }
            S::Int(value) => {
                let payload = Val::new(DecodedValue::Int(value), loc);
                state.output.push(solved_codec_tag("Int", payload, state.target, loc, current, background, account, &function, pc)?);
                continue;
            }
            S::Bool(value) => {
                state.output.push(Val::new(DecodedValue::BuiltinAtom(if value { BuiltinAtom::True } else { BuiltinAtom::False }), loc)
                    .with_type_id(crate::TypeId::solved(state.target)));
                continue;
            }
            S::Array(count) => {
                let values = state.output.split_off(state.output.len() - count);
                charge_allocation(account, logical_value_bytes(count).map_err(|e| allocation_error(e.message, &function, pc))?, &function, pc)?;
                let payload = Val::new(DecodedValue::Array(current.allocate(Object::Array(values.into_boxed_slice()))), loc);
                state.output.push(solved_codec_tag("Array", payload, state.target, loc, current, background, account, &function, pc)?);
                continue;
            }
            S::Object(names) => {
                let values = state.output.split_off(state.output.len() - names.len());
                charge_allocation(account, logical_value_bytes(names.len()).map_err(|e| allocation_error(e.message, &function, pc))?, &function, pc)?;
                for name in &names { charge_allocation(account, name.len() as u64, &function, pc)?; }
                let payload = current.record_value(names.into_iter().zip(values))
                    .map_err(|e| error(RuntimeErrorKind::InvalidBytecode, e.to_string(), &function, pc))?;
                state.output.push(solved_codec_tag("Object", payload, state.target, loc, current, background, account, &function, pc)?);
                continue;
            }
            S::Definition(index) => {
                state.definitions[index] = state.output.pop();
                state.schedule(solved_schema_ref(index));
                continue;
            }
            S::Finish => {
                let root = state.output.pop().expect("root schema");
                let view = HeapView { current, background: Some(background) };
                let (_, payload) = (ValueRef { value: root, view }).tagged_parts().expect("schema object");
                let DecodedValue::Dict(handle) = payload.value.value() else { unreachable!("schema object fields") };
                let (fields, values) = view.dict_parts(handle).expect("schema object fields");
                let mut names = vec![];
                for (name, &value) in fields.iter().zip(values) {
                    names.push(view.text(*name).expect("schema field name").to_owned());
                    state.output.push(value);
                }
                if !state.definitions.is_empty() {
                    names.push("$defs".into());
                    for value in &state.definitions { state.output.push(value.expect("completed schema definition")); }
                    let definitions = (0..state.definitions.len()).map(|index| format!("Type{index}")).collect();
                    state.schedule(vec![S::Object(definitions), S::String("https://json-schema.org/draft/2020-12/schema".into()), {
                        names.push("$schema".into()); S::Object(names)
                    }]);
                } else {
                    names.push("$schema".into());
                    state.schedule(vec![S::String("https://json-schema.org/draft/2020-12/schema".into()), S::Object(names)]);
                }
                continue;
            }
            S::Visit(ty, body) => (ty, body),
        };
        let (ty, body) = ty;
        let mut shape = &types.types[ty.index()];
        let mut rename = false;
        let mut untagged = false;
        if matches!(shape.constructor, T::Nominal(_)) {
            let property_node = |name| state.properties.iter().find(|(key, _)| *key == name)
                .and_then(|(_, property)| graph.property(PropertyKey { owner: ty, site: PropertySite::Type, property: *property }));
            let bridged = property_node("encode_by_display").is_some();
            if bridged != property_node("decode_by_parse").is_some() {
                return Err(error(RuntimeErrorKind::TypeMismatch,
                    "std/string.decode_by_parse and std/string.encode_by_display must be used together", &function, pc));
            }
            for &(name, property) in &state.properties {
                if bridged && matches!(name, "json_rename_all" | "json_untagged") { continue; }
                let Some(node) = graph.property(PropertyKey { owner: ty, site: PropertySite::Type, property }) else { continue; };
                let result = request_solved(current, background, node);
                let property = match result {
                    Ok(Request::Ready(value)) => *value,
                    Ok(Request::Start) => {
                        let callee = current.solved_tasks.get(node.index()).copied().flatten()
                            .ok_or_else(|| error(RuntimeErrorKind::InvalidBytecode, "schema property has no compiled initializer", &function, pc))?;
                        state.pending.push(S::Visit(ty, body));
                        let continuation = DemandContinuation { node, trace_frame: state.trace_frame.clone(),
                            call_function: Arc::clone(&function), call_pc: pc, return_target: ReturnTarget::Native(Box::new(state)) };
                        return Ok(VmAction::Call { callee, arguments: vec![], return_target: ReturnTarget::Native(Box::new(continuation)),
                            call_function: function, call_pc: pc, rule_boundary: loc });
                    }
                    Err(EvaluationError::Failed(failure)) => return Err(propagated_failure_error(failure.0, loc, &function, pc)),
                    Err(EvaluationError::Cycle(path)) => return Err(error(RuntimeErrorKind::UninitializedDefinition,
                        format!("cyclic schema property demand: {path:?}"), &function, pc)),
                    Err(e) => return Err(error(RuntimeErrorKind::InvalidBytecode, format!("invalid schema property demand: {e:?}"), &function, pc)),
                };
                match name {
                    "json_rename_all" => {
                        let view = HeapView { current, background: Some(background) };
                        if (ValueRef { value: property, view }).dict_get("case").and_then(|value| value.as_atom())
                            .is_none_or(|case| case.as_str() != "CamelCase") {
                            return Err(error(RuntimeErrorKind::TypeMismatch, "rename_all requires CamelCase", &function, pc));
                        }
                        rename = true;
                    }
                    "json_untagged" => untagged = true,
                    _ => {}
                }
            }
            if bridged { state.schedule(solved_schema_kind("string")); continue; }
            if !body {
                if let Some(index) = state.links[ty.index()] { state.schedule(solved_schema_ref(index)); }
                else {
                    let index = state.definitions.len();
                    state.links[ty.index()] = Some(index);
                    state.definitions.push(None);
                    state.schedule(vec![S::Visit(ty, true), S::Definition(index)]);
                }
                continue;
            }
            let layout = types.layout(ty).ok_or_else(|| error(RuntimeErrorKind::InvalidBytecode, "schema owner has no solved layout", &function, pc))?;
            shape = &types.types[layout.body.index()];
        }
        let steps = match &shape.constructor {
            T::Int => solved_schema_kind("integer"),
            T::Float => solved_schema_kind("number"),
            T::String => solved_schema_kind("string"),
            T::Bool => solved_schema_kind("boolean"),
            T::Unchecked | T::Newtype => vec![S::Visit(shape.arguments[0], false)],
            T::Array | T::Dict => vec![S::String(if shape.constructor == T::Array { "array" } else { "object" }.into()),
                S::Visit(shape.arguments[0], false), schema_object(&["type", if shape.constructor == T::Array { "items" } else { "additionalProperties" }])],
            T::Option => {
                let mut steps = solved_schema_kind("null");
                steps.extend([S::Visit(shape.arguments[0], false), S::Array(2), schema_object(&["anyOf"])]);
                steps
            }
            T::Tuple => {
                let mut steps = vec![S::String("array".into())];
                steps.extend(shape.arguments.iter().map(|&ty| S::Visit(ty, false)));
                steps.extend([S::Array(shape.arguments.len()), S::Int(shape.arguments.len() as i64), S::Int(shape.arguments.len() as i64),
                    schema_object(&["type", "prefixItems", "minItems", "maxItems"])]);
                steps
            }
            T::Record(names) => {
                let external = names.iter().map(|name| if rename { lower_camel_case(name) } else { name.clone() }).collect::<Vec<_>>();
                let mut unique = std::collections::BTreeSet::new();
                for name in &external {
                    if !unique.insert(name) { return Err(error(RuntimeErrorKind::TypeMismatch, format!("$.{name}: duplicate external field name"), &function, pc)); }
                }
                let required = external.iter().zip(&shape.arguments).filter(|(_, ty)| types.types[ty.index()].constructor != T::Option)
                    .map(|(name, _)| name.clone()).collect::<Vec<_>>();
                let mut steps = vec![S::String("object".into())];
                steps.extend(shape.arguments.iter().map(|&ty| S::Visit(ty, false)));
                steps.extend([S::Object(external), S::Bool(false)]);
                let mut keys = vec!["type", "properties", "additionalProperties"];
                if !required.is_empty() {
                    let count = required.len();
                    steps.extend(required.into_iter().map(S::String));
                    steps.push(S::Array(count));
                    keys.push("required");
                }
                steps.push(schema_object(&keys));
                steps
            }
            T::Enum(_) | T::Result | T::FoldControl | T::PropertyTarget => {
                let variants = if let T::Enum(members) = &shape.constructor {
                    let mut payload = shape.arguments.iter();
                    members.iter().map(|(name, has_payload)| (name.clone(), has_payload.then(|| *payload.next().expect("enum payload")))).collect::<Vec<_>>()
                } else {
                    let count = if shape.constructor == T::PropertyTarget { 6 } else { 2 };
                    (0..count).map(|index| {
                        let (name, _) = crate::type_image::builtin_variant(&shape.constructor, index).expect("native enum variant");
                        (name.into(), crate::type_image::builtin_variant_argument(&shape.constructor, index).map(|index| shape.arguments[index]))
                    }).collect()
                };
                if rename && untagged { return Err(error(RuntimeErrorKind::TypeMismatch, "$: rename_all is not meaningful on an untagged Enum", &function, pc)); }
                if untagged && variants.iter().filter(|(_, payload)| payload.is_none()).count() > 1 {
                    return Err(error(RuntimeErrorKind::TypeMismatch, "$: untagged Enum may contain at most one unit variant", &function, pc));
                }
                let mut names = std::collections::BTreeSet::new();
                let count = variants.len();
                let mut steps = vec![];
                for (name, payload) in variants {
                    let name = if rename { lower_camel_case(&name) } else { name };
                    if !untagged && !names.insert(name.clone()) { return Err(error(RuntimeErrorKind::TypeMismatch, format!("$.{name}: duplicate external variant name"), &function, pc)); }
                    match (untagged, payload) {
                        (true, Some(ty)) => steps.push(S::Visit(ty, false)),
                        (true, None) => steps.extend(solved_schema_kind("null")),
                        (false, None) => steps.extend([S::String(name), schema_object(&["const"])]),
                        (false, Some(ty)) => steps.extend([S::String("object".into()), S::Visit(ty, false), S::Object(vec![name.clone()]),
                            S::String(name), S::Array(1), S::Bool(false), schema_object(&["type", "properties", "required", "additionalProperties"])]),
                    }
                }
                steps.extend([S::Array(count), schema_object(&["oneOf"])]);
                steps
            }
            T::Type | T::TypeOf => return Err(error(RuntimeErrorKind::TypeMismatch, "JSON Schema cannot describe Type metadata", &function, pc)),
            T::Dyn => return Err(error(RuntimeErrorKind::TypeMismatch, "JSON Schema cannot describe Dyn", &function, pc)),
            T::Parameter(_) => return Err(error(RuntimeErrorKind::InvalidBytecode, "JSON Schema requires a concrete type", &function, pc)),
            T::Bytes | T::Function | T::Native(_) => {
                let name = match shape.constructor { T::Bytes => "Bytes", T::Function => "Func", _ => "Opaque" };
                return Err(error(RuntimeErrorKind::TypeMismatch, format!("Type {name} has no JSON Schema mapping"), &function, pc));
            }
            other => return Err(error(RuntimeErrorKind::TypeMismatch, format!("Type {other:?} has no JSON Schema mapping"), &function, pc)),
        };
        state.schedule(steps);
    }
    let value = state.output.pop().expect("completed schema");
    Ok(VmAction::Return { value, return_target: state.return_target })
}
