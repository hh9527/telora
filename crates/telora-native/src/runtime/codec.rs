use super::*;

pub(super) enum DecodeFailure {
    Rejected(String, Vec<crate::abi::Origin>),
    Blame(Value),
    Failed,
    Runtime(String),
}
impl From<String> for DecodeFailure {
    fn from(error: String) -> Self { Self::Runtime(error) }
}
impl From<&str> for DecodeFailure {
    fn from(error: &str) -> Self { Self::Runtime(error.into()) }
}

#[derive(Clone, Copy)]
pub(super) struct PropertyPlan {
    pub owner: TypeId,
    pub property: TypeId,
    pub slot: u64,
    pub initializer: usize,
    pub display_dispatcher: usize,
}

fn lower_camel_case(name: &str) -> String {
    let mut output = String::with_capacity(name.len());
    let mut uppercase = false;
    for (index, character) in name.chars().enumerate() {
        if character == '_' { uppercase = true; }
        else if uppercase { output.extend(character.to_uppercase()); uppercase = false; }
        else if index == 0 { output.extend(character.to_lowercase()); }
        else { output.push(character); }
    }
    output
}

pub(super) fn external_names(names: impl Iterator<Item = String>, rename: bool, message: &str) -> std::result::Result<Vec<String>, DecodeFailure> {
    let names = names.map(|name| if rename { lower_camel_case(&name) } else { name }).collect::<Vec<_>>();
    if names.iter().collect::<std::collections::BTreeSet<_>>().len() != names.len() { return Err(message.into()); }
    Ok(names)
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

pub(super) struct Codec<'a> {
    pub context: &'a mut crate::abi::CallContext,
    pub checks: &'a [CheckPlan],
    pub properties: &'a [PropertyPlan],
}

impl Codec<'_> {
    pub(super) fn runtime(&self) -> Result<&Runtime> { self.context.runtime() }
    pub(super) fn runtime_mut(&mut self) -> Result<&mut Runtime> { self.context.runtime_mut() }

    pub(super) fn property_types(&self, properties: &Value) -> Result<Vec<(String, TypeId)>> {
        let rt = self.runtime()?;
        rt.layout(properties.type_id())?.field_names.iter().enumerate()
            .map(|(index, name)| Ok((name.clone(), rt.represented_type(rt.field(properties, index)?)?)))
            .collect()
    }

    pub(super) fn property(&mut self, owner: TypeId, property: TypeId, origin: crate::abi::Origin) -> std::result::Result<Option<Value>, DecodeFailure> {
        if !self.runtime()?.property_presence.contains(&(owner, property)) { return Ok(None); }
        let plan = self.properties.iter().find(|plan| plan.owner == owner && plan.property == property)
            .copied().ok_or("sealed codec property is missing from the native plan")?;
        let mut words = vec![0; self.runtime()?.layout(property)?.words].into_boxed_slice();
        let status = unsafe { helpers::demand(self.context, property, plan.slot, plan.initializer as *const u64, words.as_mut_ptr(), origin) };
        if status == 1 { return Err(DecodeFailure::Failed); }
        if status != 0 { return Err("invalid codec property initializer status".into()); }
        Ok(Some(Value { arena: self.runtime()?.identity, words }))
    }

    pub(super) fn options(&mut self, owner: TypeId, properties: &[(String, TypeId)], origin: crate::abi::Origin) -> std::result::Result<(bool, bool, bool), DecodeFailure> {
        let property_type = |name: &str| properties.iter().find(|(key, _)| key == name).map(|(_, ty)| *ty).ok_or_else(|| format!("codec property contract lacks {name}"));
        let rt = self.runtime()?;
        let decode = rt.property_presence.contains(&(owner, property_type("decode_by_parse")?));
        let encode = rt.property_presence.contains(&(owner, property_type("encode_by_display")?));
        if decode != encode { return Err("std/string.decode_by_parse and std/string.encode_by_display must be used together".into()); }
        if decode {
            self.property(owner, property_type("decode_by_parse")?, origin)?;
            self.property(owner, property_type("encode_by_display")?, origin)?;
            return Ok((false, false, true));
        }
        let mut rename = false;
        if let Some(value) = self.property(owner, property_type("json_rename_all")?, origin)? {
            let rt = self.runtime()?;
            let index = rt.layout(value.type_id())?.field_names.iter().position(|name| name == "case").ok_or("rename_all property has no case")?;
            let case = rt.field(&value, index)?.to_owned();
            if rt.layout(case.type_id())?.variants[rt.enum_tag(&case)? as usize].name != "CamelCase" { return Err("rename_all requires CamelCase".into()); }
            rename = true;
        }
        let untagged = self.property(owner, property_type("json_untagged")?, origin)?.is_some();
        if rename && untagged && self.runtime()?.layout(owner)?.kind == Kind::Enum {
            return Err("rename_all is not meaningful on an untagged Enum".into());
        }
        Ok((rename, untagged, false))
    }

    pub(super) fn decode(&mut self, result: TypeId, target: TypeId, properties: &Value, input: &Value, loc: Location) -> Result<Option<Value>> {
        let contract = self.runtime()?.data_contract.clone().ok_or("semantic Value contract is not loaded")?;
        self.runtime()?.validate(input.as_ref(), contract.value_type())?;
        let properties = self.property_types(properties)?;
        match self.decode_value(&contract, &properties, target, input, "$", 0) {
            Ok(value) => self.runtime_mut()?.named_variant(result, loc, "Ok", Some(&value)).map(Some),
            Err(DecodeFailure::Failed) => Ok(None),
            Err(DecodeFailure::Blame(blame)) => self.runtime_mut()?.named_variant(result, loc, "Err", Some(&blame)).map(Some),
            Err(DecodeFailure::Runtime(error)) => Err(error),
            Err(DecodeFailure::Rejected(message, subjects)) => {
                let blame_type = self.runtime()?.layout(result)?.variants.iter().find(|v| v.name == "Err")
                    .and_then(|v| v.payload).ok_or("codec result has no error payload")?;
                let message = self.runtime_mut()?.owned_string(contract.payload("String")?, loc, message)?;
                let blame = self.runtime_mut()?.blame(blame_type, loc, &message, subjects)?;
                self.runtime_mut()?.named_variant(result, loc, "Err", Some(&blame)).map(Some)
            }
        }
    }

    pub(super) fn check(&mut self, owner: TypeId, site: u64, value: &Value) -> std::result::Result<(), DecodeFailure> {
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

    fn decode_value(&mut self, contract: &DataContract, properties: &[(String, TypeId)], target: TypeId, input: &Value, path: &str, depth: usize) -> std::result::Result<Value, DecodeFailure> {
        if depth > 512 { return Err(DecodeFailure::Runtime("native codec nesting limit".into())); }
        if target == contract.value_type() { return Ok(input.clone()); }
        let info = self.runtime()?.type_info[target.index()].kind;
        let (rename, untagged, bridged) = if info == Some("Ref") { self.options(target, properties, input.origin())? } else { (false, false, false) };
        let tag = self.runtime()?.layout(input.type_id())?.variants[self.runtime()?.enum_tag(input)? as usize].name.clone();
        let payload = self.runtime()?.enum_payload(input)?.map(ValueRef::to_owned);
        let loc = input.origin().words();
        let reject = |expected: &str| DecodeFailure::Rejected(format!("{path}: expected {expected}"), vec![input.origin()]);
        if bridged {
            if tag != "String" { return Err(reject("String text representation")); }
            let text = payload.ok_or("missing semantic String payload")?;
            let length = self.runtime()?.text(text.as_ref())?.as_str().len();
            let property = properties.iter().find(|(name, _)| name == "parse_by").map(|(_, ty)| *ty).ok_or("codec property contract lacks parse_by")?;
            return self.parse_value(target, property, &text, Some(0..length), path, depth + 1)
                .map_err(|error| match error { DecodeFailure::Rejected(message, _) => DecodeFailure::Rejected(message, vec![input.origin()]), error => error });
        }
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
            if untagged {
                let variants = layout.variants.iter().map(|variant| variant.payload).collect::<Vec<_>>();
                let mut matches = Vec::new();
                let mut failures = Vec::new();
                for (index, child) in variants.into_iter().enumerate() {
                    let candidate = if let Some(child) = child {
                        (|| {
                            let value = self.decode_value(contract, properties, child, input, path, depth + 1)?;
                            self.check(target, index as u64 + 1, &value)?;
                            Ok(self.runtime_mut()?.enum_value(target, loc, index as u32, Some(&value))?)
                        })()
                    } else if tag == "None" {
                        Ok(self.runtime_mut()?.enum_value(target, loc, index as u32, None)?)
                    } else { continue; };
                    match candidate {
                        Ok(value) => matches.push(value),
                        Err(DecodeFailure::Rejected(message, subjects)) => failures.push((message, subjects)),
                        Err(DecodeFailure::Blame(blame)) => failures.push(self.runtime()?.blame_diagnostic(&blame)?),
                        Err(error) => return Err(error),
                    }
                }
                if matches.len() == 1 { return Ok(matches.pop().unwrap()); }
                let (message, subjects) = if matches.is_empty() {
                    let message = format!("{path}: value matches no untagged Enum variant ({})", failures.iter().map(|(message, _)| message.as_str()).collect::<Vec<_>>().join("; "));
                    (message, failures.into_iter().next().map(|(_, subjects)| subjects).unwrap_or_else(|| vec![input.origin()]))
                } else { (format!("{path}: value ambiguously matches multiple untagged Enum variants"), vec![input.origin()]) };
                return Err(DecodeFailure::Rejected(message, subjects));
            }
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
            let names = external_names(layout.variants.iter().map(|variant| variant.name.clone()), rename, "duplicate external variant name")?;
            let Some(index) = names.iter().position(|variant| *variant == name) else {
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
            let names = external_names(layout.field_names.iter().cloned(), rename, "duplicate external member name")?;
            let fields = names.into_iter().zip(layout.fields.iter().map(|(ty, _)| *ty)).collect::<Vec<_>>();
            let names = fields.iter().map(|(name, _)| name.clone()).collect::<std::collections::BTreeSet<_>>();
            let payload = payload.ok_or_else(|| DecodeFailure::Runtime("missing semantic Object payload".into()))?;
            let mut inputs = std::collections::BTreeMap::new();
            for i in 0..self.runtime()?.dict_len(&payload)? {
                let (key, value) = self.runtime()?.dict_entry(&payload, i)?;
                let name = self.runtime()?.text(key)?.as_str().to_owned();
                if !names.contains(&name) {
                    return Err(DecodeFailure::Rejected(format!("{path}.{name}: unknown field"), vec![value.to_owned().origin()]));
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
                    return Err(DecodeFailure::Rejected(format!("{path}.{name}: missing required field"), vec![input.origin()]));
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

impl Codec<'_> {
    pub(super) fn encode(
        &mut self,
        target: TypeId,
        properties: &Value,
        input: &Value,
    ) -> Result<Option<Value>> {
        let contract = self.runtime()?
            .data_contract
            .clone()
            .ok_or("semantic Value contract is not loaded")?;
        if contract.value_type() != target {
            return Err("codec target differs from admitted Value identity".into());
        }
        let properties = self.property_types(properties)?;
        match self.encode_value(&contract, &properties, input, 0) {
            Ok(value) => Ok(Some(value)),
            Err(DecodeFailure::Failed) => Ok(None),
            Err(DecodeFailure::Runtime(error)) => Err(error),
            _ => Err("unexpected codec encoding rejection".into()),
        }
    }

    fn encode_value(
        &mut self,
        contract: &DataContract,
        properties: &[(String, TypeId)],
        input: &Value,
        depth: usize,
    ) -> std::result::Result<Value, DecodeFailure> {
        if depth > 512 {
            return Err("native codec nesting limit".into());
        }
        self.runtime()?.validate(input.as_ref(), input.type_id())?;
        let target = contract.value_type();
        if input.type_id() == target {
            return Ok(input.clone());
        }
        let loc = input.origin().words();
        let kind = self.runtime()?.type_info[input.type_id().index()].kind;
        let (rename, untagged, bridged) = if kind == Some("Ref") { self.options(input.type_id(), properties, input.origin())? } else { (false, false, false) };
        if bridged {
            let property = properties.iter().find(|(name, _)| name == "display_by").map(|(_, ty)| *ty).ok_or("codec property contract lacks display_by")?;
            let text = self.display(input, property)?;
            let text = self.runtime_mut()?.owned_string(contract.payload("String")?, loc, text)?;
            return Ok(self.runtime_mut()?.named_variant(target, loc, "String", Some(&text))?);
        }
        if let Some(tag @ ("Int" | "Float" | "String" | "Bytes")) = kind {
            return Ok(self.runtime_mut()?.named_variant(target, loc, tag, Some(input))?);
        }
        let layout = self.runtime()?.layout(input.type_id())?;
        // Bool is scalar in the materialized ABI and Enum in reflection.
        if kind == Some("Enum") && layout.kind == Kind::Scalar {
            let is_false = self.runtime()?.scalar_bits(input.as_ref())? == 0;
            return Ok(self.runtime_mut()?.named_variant(
                target,
                loc,
                if is_false {
                    "False"
                } else {
                    "True"
                },
                None,
            )?);
        }
        if layout.optional {
            return if let Some(value) = self.runtime()?.enum_payload(input)?.map(ValueRef::to_owned) {
                self.encode_value(contract, properties, &value, depth + 1)
            } else {
                Ok(self.runtime_mut()?.named_variant(target, loc, "None", None)?)
            };
        }
        if kind == Some("Ref") && layout.dynamic_kind == Some("Tuple") {
            let value = self.runtime()?.field(input, 0)?.to_owned();
            return self.encode_value(contract, properties, &value, depth + 1);
        }
        if matches!(layout.kind, Kind::Array | Kind::Tuple) {
            let array = layout.kind == Kind::Array;
            let count = if array {
                self.runtime()?.array_len(input)?
            } else {
                layout.fields.len()
            };
            let mut values = Vec::with_capacity(count);
            for i in 0..count {
                let value = if array {
                    self.runtime()?.array_get(input, i)?
                } else {
                    self.runtime()?.field(input, i)?
                }
                .to_owned();
                values.push(self.encode_value(contract, properties, &value, depth + 1)?);
            }
            let payload = self.runtime_mut()?.array(contract.payload("Array")?, loc, &values)?;
            return Ok(self.runtime_mut()?.named_variant(target, loc, "Array", Some(&payload))?);
        }
        if matches!(layout.kind, Kind::Dict | Kind::Record) {
            let dictionary = layout.kind == Kind::Dict;
            let names = external_names(layout.field_names.iter().cloned(), rename, "duplicate external member name")?;
            let count = if dictionary {
                self.runtime()?.dict_len(input)?
            } else {
                names.len()
            };
            let mut pairs = Vec::with_capacity(count);
            for i in 0..count {
                let (key, value) = if dictionary {
                    let (key, value) = self.runtime()?.dict_entry(input, i)?;
                    (key.to_owned(), value.to_owned())
                } else {
                    let value = self.runtime()?.field(input, i)?.to_owned();
                    (
                        self.runtime_mut()?.string(contract.payload("String")?, loc, &names[i])?,
                        value,
                    )
                };
                pairs.push((
                    key,
                    self.encode_value(contract, properties, &value, depth + 1)?,
                ));
            }
            let payload = self.runtime_mut()?.dict(contract.payload("Object")?, loc, &pairs)?;
            return Ok(self.runtime_mut()?.named_variant(target, loc, "Object", Some(&payload))?);
        }
        if layout.kind == Kind::Enum {
            if untagged {
                if layout.variants.iter().filter(|variant| variant.payload.is_none()).count() > 1 {
                    return Err("untagged Enum may contain at most one unit variant".into());
                }
                return if let Some(value) = self.runtime()?.enum_payload(input)?.map(ValueRef::to_owned) {
                    self.encode_value(contract, properties, &value, depth + 1)
                } else { Ok(self.runtime_mut()?.named_variant(target, loc, "None", None)?) };
            }
            let names = external_names(layout.variants.iter().map(|variant| variant.name.clone()), rename, "duplicate external variant name")?;
            let name = &names[self.runtime()?.enum_tag(input)? as usize];
            let key = self.runtime_mut()?.string(contract.payload("String")?, loc, name)?;
            if let Some(value) = self.runtime()?.enum_payload(input)?.map(ValueRef::to_owned) {
                let value = self.encode_value(contract, properties, &value, depth + 1)?;
                let payload = self.runtime_mut()?.dict(contract.payload("Object")?, loc, &[(key, value)])?;
                return Ok(self.runtime_mut()?.named_variant(target, loc, "Object", Some(&payload))?);
            }
            return Ok(self.runtime_mut()?.named_variant(target, loc, "String", Some(&key))?);
        }
        Err("native codec cannot encode this sealed type".into())
    }
}
