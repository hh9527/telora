use super::*;

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
