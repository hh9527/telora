use super::*;

enum DecodeFailure {
    Rejected(String, crate::abi::Origin),
    Runtime(String),
}
impl From<String> for DecodeFailure {
    fn from(error: String) -> Self { Self::Runtime(error) }
}

impl Runtime {
    pub(crate) fn decode(&mut self, result: TypeId, target: TypeId, input: &Value, loc: Location) -> Result<Value> {
        let contract = self.data_contract.clone().ok_or("semantic Value contract is not loaded")?;
        self.validate(input.as_ref(), contract.value_type())?;
        match self.decode_value(&contract, target, input, "$", 0) {
            Ok(value) => self.named_variant(result, loc, "Ok", Some(&value)),
            Err(DecodeFailure::Runtime(error)) => Err(error),
            Err(DecodeFailure::Rejected(message, subject)) => {
                let blame_type = self.layout(result)?.variants.iter().find(|v| v.name == "Err")
                    .and_then(|v| v.payload).ok_or("codec result has no error payload")?;
                let message = self.owned_string(contract.payload("String")?, loc, message)?;
                let blame = self.blame(blame_type, loc, &message, vec![subject])?;
                self.named_variant(result, loc, "Err", Some(&blame))
            }
        }
    }

    fn decode_value(&mut self, contract: &DataContract, target: TypeId, input: &Value, path: &str, depth: usize) -> std::result::Result<Value, DecodeFailure> {
        if depth > 512 { return Err(DecodeFailure::Runtime("native codec nesting limit".into())); }
        if target == contract.value_type() { return Ok(input.clone()); }
        let info = self.type_info[target.index()].kind;
        // Nominal decoding also requires property callbacks and construction checks.
        // Until these are linked, it must not produce an unchecked nominal value.
        if info == Some("Ref") { return Err(DecodeFailure::Runtime("native nominal codec decoding is not yet linked".into())); }
        let tag = self.layout(input.type_id())?.variants[self.enum_tag(input)? as usize].name.clone();
        let payload = self.enum_payload(input)?.map(ValueRef::to_owned);
        let loc = input.origin().words();
        let reject = |expected: &str| DecodeFailure::Rejected(format!("{path}: expected {expected}"), input.origin());
        if let Some(name @ ("Int" | "Float" | "String" | "Bytes")) = info {
            if tag != name { return Err(reject(name)); }
            let value = payload.ok_or_else(|| DecodeFailure::Runtime("missing semantic scalar payload".into()))?;
            self.validate(value.as_ref(), target)?;
            return Ok(value);
        }
        let layout = self.layout(target)?;
        if info == Some("Enum") && layout.kind == Kind::Scalar {
            return match tag.as_str() {
                "True" => Ok(self.scalar(target, loc, 1)?),
                "False" => Ok(self.scalar(target, loc, 0)?),
                _ => Err(reject("Bool")),
            };
        }
        if layout.optional {
            if tag == "None" { return Ok(self.named_variant(target, loc, "None", None)?); }
            let child = layout.arguments[0];
            let value = self.decode_value(contract, child, input, path, depth + 1)?;
            return Ok(self.named_variant(target, loc, "Some", Some(&value))?);
        }
        if matches!(layout.kind, Kind::Array | Kind::Tuple) {
            if tag != "Array" { return Err(reject("Array")); }
            let array = layout.kind == Kind::Array;
            let children = layout.arguments.clone();
            let payload = payload.ok_or_else(|| DecodeFailure::Runtime("missing semantic Array payload".into()))?;
            let count = self.array_len(&payload)?;
            if !array && count != children.len() { return Err(reject("tuple with the declared arity")); }
            let mut values = Vec::with_capacity(count);
            for i in 0..count {
                let value = self.array_get(&payload, i)?.to_owned();
                values.push(self.decode_value(contract, children[if array { 0 } else { i }], &value, &format!("{path}[{i}]"), depth + 1)?);
            }
            return Ok(if array { self.array(target, loc, &values)? } else { self.aggregate(target, loc, &values)? });
        }
        if layout.kind == Kind::Record {
            if tag != "Object" { return Err(reject("Object")); }
            let fields = layout.field_names.iter().cloned().zip(layout.fields.iter().map(|(ty, _)| *ty)).collect::<Vec<_>>();
            let payload = payload.ok_or_else(|| DecodeFailure::Runtime("missing semantic Object payload".into()))?;
            let mut inputs = std::collections::BTreeMap::new();
            for i in 0..self.dict_len(&payload)? {
                let (key, value) = self.dict_entry(&payload, i)?;
                let name = self.text(key)?.as_str().to_owned();
                if fields.binary_search_by(|(field, _)| field.cmp(&name)).is_err() {
                    return Err(DecodeFailure::Rejected(format!("{path}.{name}: unknown field"), value.to_owned().origin()));
                }
                inputs.insert(name, value.to_owned());
            }
            let mut values = Vec::with_capacity(fields.len());
            for (name, ty) in fields {
                if let Some(value) = inputs.remove(&name) {
                    values.push(self.decode_value(contract, ty, &value, &format!("{path}.{name}"), depth + 1)?);
                } else if self.layout(ty)?.optional {
                    values.push(self.named_variant(ty, loc, "None", None)?);
                } else {
                    return Err(DecodeFailure::Rejected(format!("{path}.{name}: missing required field"), input.origin()));
                }
            }
            return Ok(self.aggregate(target, loc, &values)?);
        }
        if layout.kind == Kind::Dict {
            if tag != "Object" { return Err(reject("Object")); }
            let child = layout.arguments[0];
            let payload = payload.ok_or_else(|| DecodeFailure::Runtime("missing semantic Object payload".into()))?;
            let mut pairs = Vec::with_capacity(self.dict_len(&payload)?);
            for i in 0..self.dict_len(&payload)? {
                let (key, value) = self.dict_entry(&payload, i)?;
                let (key, value) = (key.to_owned(), value.to_owned());
                let name = self.text(key.as_ref())?.as_str().to_owned();
                let value = self.decode_value(contract, child, &value, &format!("{path}.{name}"), depth + 1)?;
                pairs.push((key, value));
            }
            return Ok(self.dict(target, loc, &pairs)?);
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
