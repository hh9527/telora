use super::*;

enum DecodeFailure {
    Rejected(String, crate::abi::Origin),
    Blame(Value),
    Failed,
    Runtime(String),
}
impl From<String> for DecodeFailure {
    fn from(error: String) -> Self { Self::Runtime(error) }
}

#[derive(Clone, Copy)]
pub(super) struct CheckPlan {
    pub owner: TypeId,
    pub site: u64,
    pub slot: u64,
    pub signature: TypeId,
    pub initializer: usize,
    pub dispatcher: usize,
}

pub(super) struct Decoder<'a> {
    pub context: &'a mut crate::abi::CallContext,
    pub checks: &'a [CheckPlan],
}

impl Decoder<'_> {
    fn runtime(&self) -> Result<&Runtime> { self.context.runtime() }
    fn runtime_mut(&mut self) -> Result<&mut Runtime> { self.context.runtime_mut() }

    pub(super) fn decode(&mut self, result: TypeId, target: TypeId, properties: &Value, input: &Value, loc: Location) -> Result<Option<Value>> {
        let contract = self.runtime()?.data_contract.clone().ok_or("semantic Value contract is not loaded")?;
        self.runtime()?.validate(input.as_ref(), contract.value_type())?;
        let properties = (0..self.runtime()?.layout(properties.type_id())?.fields.len())
            .map(|i| self.runtime()?.represented_type(self.runtime()?.field(properties, i)?))
            .collect::<Result<Vec<_>>>()?;
        match self.decode_value(&contract, &properties, target, input, "$", 0) {
            Ok(value) => self.runtime_mut()?.named_variant(result, loc, "Ok", Some(&value)).map(Some),
            Err(DecodeFailure::Failed) => Ok(None),
            Err(DecodeFailure::Blame(blame)) => self.runtime_mut()?.named_variant(result, loc, "Err", Some(&blame)).map(Some),
            Err(DecodeFailure::Runtime(error)) => Err(error),
            Err(DecodeFailure::Rejected(message, subject)) => {
                let blame_type = self.runtime()?.layout(result)?.variants.iter().find(|v| v.name == "Err")
                    .and_then(|v| v.payload).ok_or("codec result has no error payload")?;
                let message = self.runtime_mut()?.owned_string(contract.payload("String")?, loc, message)?;
                let blame = self.runtime_mut()?.blame(blame_type, loc, &message, vec![subject])?;
                self.runtime_mut()?.named_variant(result, loc, "Err", Some(&blame)).map(Some)
            }
        }
    }

    fn check(&mut self, owner: TypeId, site: u64, value: &Value) -> std::result::Result<(), DecodeFailure> {
        let Some(plan) = self.checks.iter().find(|plan| plan.owner == owner && plan.site == site).copied() else {
            return if self.runtime()?.layout(owner)?.construction_checks.contains(&site) {
                Err(DecodeFailure::Runtime("sealed codec checker is missing from the native plan".into()))
            } else { Ok(()) };
        };
        let origin = value.origin();
        let rt = self.runtime()?;
        let mut closure = vec![0; rt.layout(plan.signature)?.words];
        // Addresses borrow the current generated decode adapter's packet only.
        let status = unsafe { helpers::demand(self.context, plan.signature, plan.slot, plan.initializer as *const u64, closure.as_mut_ptr(), origin) };
        if status == 1 { return Err(DecodeFailure::Failed); }
        if status != 0 { return Err(DecodeFailure::Runtime("invalid checker initializer status".into())); }
        let rt = self.runtime()?;
        let signature = rt.layout(plan.signature)?;
        let argument_type = signature.arguments[0];
        let output = signature.arguments[1];
        let mut argument = value.words().to_vec();
        if argument_type != value.type_id() {
            if rt.layout(argument_type)?.arguments != [owner] || value.type_id() != owner || rt.layout(argument_type)?.words != argument.len() {
                return Err(DecodeFailure::Runtime("checker input contradicts sealed unchecked view".into()));
            }
            argument[1] = (argument[1] & 0xffff_ffff) | (u64::from(argument_type.raw()) << 32);
        }
        let mut words = vec![0; rt.layout(output)?.words].into_boxed_slice();
        type Callback = unsafe extern "C" fn(*mut crate::abi::CallContext, *const u64, *mut u64, *const u64) -> u32;
        let callback = unsafe { std::mem::transmute::<usize, Callback>(plan.dispatcher) };
        let status = unsafe { callback(self.context, argument.as_ptr(), words.as_mut_ptr(), closure.as_ptr()) };
        if status == 1 { return Err(DecodeFailure::Failed); }
        if status != 0 { return Err(DecodeFailure::Runtime("invalid checker callback status".into())); }
        let rt = self.runtime()?;
        let result = Value { arena: rt.identity, words };
        rt.validate(result.as_ref(), output)?;
        match rt.layout(output)?.variants[rt.enum_tag(&result)? as usize].name.as_str() {
            "Ok" => Ok(()),
            "Err" => Err(DecodeFailure::Blame(rt.enum_payload(&result)?.ok_or_else(|| "checker error has no payload".to_string())?.to_owned())),
            _ => Err(DecodeFailure::Runtime("checker did not return Result".into())),
        }
    }

    fn decode_value(&mut self, contract: &DataContract, properties: &[TypeId], target: TypeId, input: &Value, path: &str, depth: usize) -> std::result::Result<Value, DecodeFailure> {
        if depth > 512 { return Err(DecodeFailure::Runtime("native codec nesting limit".into())); }
        if target == contract.value_type() { return Ok(input.clone()); }
        let rt = self.runtime()?;
        let info = rt.type_info[target.index()].kind;
        if info == Some("Ref") && properties.iter().any(|property| rt.property_presence.contains(&(target, *property))) {
            return Err(DecodeFailure::Runtime("native codec property execution is not yet linked".into()));
        }
        let tag = self.runtime()?.layout(input.type_id())?.variants[self.runtime()?.enum_tag(input)? as usize].name.clone();
        let payload = self.runtime()?.enum_payload(input)?.map(ValueRef::to_owned);
        let loc = input.origin().words();
        let reject = |expected: &str| DecodeFailure::Rejected(format!("{path}: expected {expected}"), input.origin());
        if let Some(name @ ("Int" | "Float" | "String" | "Bytes")) = info {
            if tag != name { return Err(reject(name)); }
            let value = payload.ok_or_else(|| DecodeFailure::Runtime("missing semantic scalar payload".into()))?;
            self.runtime()?.validate(value.as_ref(), target)?;
            return Ok(value);
        }
        let layout = self.runtime()?.layout(target)?;
        if info == Some("Ref") && layout.dynamic_kind == Some("Tuple") {
            let child = layout.fields[0].0;
            let value = self.decode_value(contract, properties, child, input, path, depth + 1)?;
            self.check(target, 0, &value)?;
            return Ok(self.runtime_mut()?.aggregate(target, loc, &[value])?);
        }
        if info == Some("Ref") && layout.kind == Kind::Enum {
            let (name, value) = match (tag.as_str(), payload) {
                ("String", Some(text)) => (self.runtime()?.text(text.as_ref())?.as_str().to_owned(), None),
                ("Object", Some(object)) if self.runtime()?.dict_len(&object)? == 1 => {
                    let (key, value) = self.runtime()?.dict_entry(&object, 0)?;
                    (self.runtime()?.text(key)?.as_str().to_owned(), Some(value.to_owned()))
                }
                ("LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime", Some(text)) => {
                    let value = self.runtime_mut()?.named_variant(contract.value_type(), loc, "String", Some(&text))?;
                    (tag, Some(value))
                }
                _ => return Err(reject("a declared enum variant")),
            };
            let layout = self.runtime()?.layout(target)?;
            let Some(index) = layout.variants.iter().position(|variant| variant.name == name) else {
                return Err(reject("a declared enum variant"));
            };
            return match (layout.variants[index].payload, value) {
                (Some(child), Some(value)) => {
                    let value = self.decode_value(contract, properties, child, &value, path, depth + 1)?;
                    self.check(target, index as u64 + 1, &value)?;
                    Ok(self.runtime_mut()?.enum_value(target, loc, index as u32, Some(&value))?)
                }
                (None, None) => Ok(self.runtime_mut()?.enum_value(target, loc, index as u32, None)?),
                _ => Err(reject("a declared enum variant")),
            };
        }
        if info == Some("Enum") && layout.kind == Kind::Scalar {
            return match tag.as_str() {
                "True" => Ok(self.runtime()?.scalar(target, loc, 1)?),
                "False" => Ok(self.runtime()?.scalar(target, loc, 0)?),
                _ => Err(reject("Bool")),
            };
        }
        if layout.optional {
            if tag == "None" { return Ok(self.runtime_mut()?.named_variant(target, loc, "None", None)?); }
            let child = layout.arguments[0];
            let value = self.decode_value(contract, properties, child, input, path, depth + 1)?;
            return Ok(self.runtime_mut()?.named_variant(target, loc, "Some", Some(&value))?);
        }
        if matches!(layout.kind, Kind::Array | Kind::Tuple) {
            if tag != "Array" { return Err(reject("Array")); }
            let array = layout.kind == Kind::Array;
            let children = layout.arguments.clone();
            let payload = payload.ok_or_else(|| DecodeFailure::Runtime("missing semantic Array payload".into()))?;
            let count = self.runtime()?.array_len(&payload)?;
            if !array && count != children.len() { return Err(reject("tuple with the declared arity")); }
            let mut values = Vec::with_capacity(count);
            for i in 0..count {
                let value = self.runtime()?.array_get(&payload, i)?.to_owned();
                values.push(self.decode_value(contract, properties, children[if array { 0 } else { i }], &value, &format!("{path}[{i}]"), depth + 1)?);
            }
            return Ok(if array { self.runtime_mut()?.array(target, loc, &values)? } else { self.runtime_mut()?.aggregate(target, loc, &values)? });
        }
        if layout.kind == Kind::Record {
            if tag != "Object" { return Err(reject("Object")); }
            let fields = layout.field_names.iter().cloned().zip(layout.fields.iter().map(|(ty, _)| *ty)).collect::<Vec<_>>();
            let payload = payload.ok_or_else(|| DecodeFailure::Runtime("missing semantic Object payload".into()))?;
            let mut inputs = std::collections::BTreeMap::new();
            for i in 0..self.runtime()?.dict_len(&payload)? {
                let (key, value) = self.runtime()?.dict_entry(&payload, i)?;
                let name = self.runtime()?.text(key)?.as_str().to_owned();
                if fields.binary_search_by(|(field, _)| field.cmp(&name)).is_err() {
                    return Err(DecodeFailure::Rejected(format!("{path}.{name}: unknown field"), value.to_owned().origin()));
                }
                inputs.insert(name, value.to_owned());
            }
            let mut values = Vec::with_capacity(fields.len());
            for (name, ty) in fields {
                if let Some(value) = inputs.remove(&name) {
                    values.push(self.decode_value(contract, properties, ty, &value, &format!("{path}.{name}"), depth + 1)?);
                } else if self.runtime()?.layout(ty)?.optional {
                    values.push(self.runtime_mut()?.named_variant(ty, loc, "None", None)?);
                } else {
                    return Err(DecodeFailure::Rejected(format!("{path}.{name}: missing required field"), input.origin()));
                }
            }
            let value = self.runtime_mut()?.aggregate(target, loc, &values)?;
            self.check(target, 0, &value)?;
            return Ok(value);
        }
        if layout.kind == Kind::Dict {
            if tag != "Object" { return Err(reject("Object")); }
            let child = layout.arguments[0];
            let payload = payload.ok_or_else(|| DecodeFailure::Runtime("missing semantic Object payload".into()))?;
            let mut pairs = Vec::with_capacity(self.runtime()?.dict_len(&payload)?);
            for i in 0..self.runtime()?.dict_len(&payload)? {
                let (key, value) = self.runtime()?.dict_entry(&payload, i)?;
                let (key, value) = (key.to_owned(), value.to_owned());
                let name = self.runtime()?.text(key.as_ref())?.as_str().to_owned();
                let value = self.decode_value(contract, properties, child, &value, &format!("{path}.{name}"), depth + 1)?;
                pairs.push((key, value));
            }
            return Ok(self.runtime_mut()?.dict(target, loc, &pairs)?);
        }
        Err(DecodeFailure::Runtime("native codec cannot decode this sealed type".into()))
    }
}

impl Runtime {
    pub(crate) fn encode(
        &mut self,
        target: TypeId,
        properties: &Value,
        input: &Value,
    ) -> Result<Value> {
        let contract = self
            .data_contract
            .clone()
            .ok_or("semantic Value contract is not loaded")?;
        if contract.value_type() != target {
            return Err("codec target differs from admitted Value identity".into());
        }
        let property_types = (0..self.layout(properties.type_id())?.fields.len())
            .map(|i| self.represented_type(self.field(properties, i)?))
            .collect::<Result<Vec<_>>>()?;
        self.encode_value(&contract, &property_types, input, 0)
    }

    fn encode_value(
        &mut self,
        contract: &DataContract,
        properties: &[TypeId],
        input: &Value,
        depth: usize,
    ) -> Result<Value> {
        if depth > 512 {
            return Err("native codec nesting limit".into());
        }
        self.validate(input.as_ref(), input.type_id())?;
        let target = contract.value_type();
        if input.type_id() == target {
            return Ok(input.clone());
        }
        let loc = input.origin().words();
        let kind = self.type_info[input.type_id().index()].kind;
        if kind == Some("Ref")
            && properties
                .iter()
                .any(|p| self.property_presence.contains(&(input.type_id(), *p)))
        {
            return Err("native codec property execution is not yet linked".into());
        }
        if let Some(tag @ ("Int" | "Float" | "String" | "Bytes")) = kind {
            return self.named_variant(target, loc, tag, Some(input));
        }
        let layout = self.layout(input.type_id())?;
        // Bool is scalar in the materialized ABI and Enum in reflection.
        if kind == Some("Enum") && layout.kind == Kind::Scalar {
            return self.named_variant(
                target,
                loc,
                if self.scalar_bits(input.as_ref())? == 0 {
                    "False"
                } else {
                    "True"
                },
                None,
            );
        }
        if layout.optional {
            return if let Some(value) = self.enum_payload(input)?.map(ValueRef::to_owned) {
                self.encode_value(contract, properties, &value, depth + 1)
            } else {
                self.named_variant(target, loc, "None", None)
            };
        }
        if kind == Some("Ref") && layout.dynamic_kind == Some("Tuple") {
            let value = self.field(input, 0)?.to_owned();
            return self.encode_value(contract, properties, &value, depth + 1);
        }
        if matches!(layout.kind, Kind::Array | Kind::Tuple) {
            let array = layout.kind == Kind::Array;
            let count = if array {
                self.array_len(input)?
            } else {
                layout.fields.len()
            };
            let mut values = Vec::with_capacity(count);
            for i in 0..count {
                let value = if array {
                    self.array_get(input, i)?
                } else {
                    self.field(input, i)?
                }
                .to_owned();
                values.push(self.encode_value(contract, properties, &value, depth + 1)?);
            }
            let payload = self.array(contract.payload("Array")?, loc, &values)?;
            return self.named_variant(target, loc, "Array", Some(&payload));
        }
        if matches!(layout.kind, Kind::Dict | Kind::Record) {
            let dictionary = layout.kind == Kind::Dict;
            let names = layout.field_names.clone();
            let count = if dictionary {
                self.dict_len(input)?
            } else {
                names.len()
            };
            let mut pairs = Vec::with_capacity(count);
            for i in 0..count {
                let (key, value) = if dictionary {
                    let (key, value) = self.dict_entry(input, i)?;
                    (key.to_owned(), value.to_owned())
                } else {
                    let value = self.field(input, i)?.to_owned();
                    (
                        self.string(contract.payload("String")?, loc, &names[i])?,
                        value,
                    )
                };
                pairs.push((
                    key,
                    self.encode_value(contract, properties, &value, depth + 1)?,
                ));
            }
            let payload = self.dict(contract.payload("Object")?, loc, &pairs)?;
            return self.named_variant(target, loc, "Object", Some(&payload));
        }
        if kind == Some("Ref") && layout.kind == Kind::Enum {
            let name = layout.variants[self.enum_tag(input)? as usize].name.clone();
            let key = self.string(contract.payload("String")?, loc, &name)?;
            if let Some(value) = self.enum_payload(input)?.map(ValueRef::to_owned) {
                let value = self.encode_value(contract, properties, &value, depth + 1)?;
                let payload = self.dict(contract.payload("Object")?, loc, &[(key, value)])?;
                return self.named_variant(target, loc, "Object", Some(&payload));
            }
            return self.named_variant(target, loc, "String", Some(&key));
        }
        Err("native codec cannot encode this sealed type".into())
    }
}
