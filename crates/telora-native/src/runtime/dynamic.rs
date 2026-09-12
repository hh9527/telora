use super::*;

#[derive(Clone, Copy)]
pub enum DynamicQuery {
    Fields,
    ArrayItems,
    TupleItems,
    Tag,
    Payload,
}

impl Runtime {
    pub(crate) fn dynamic_member(
        &mut self,
        output: TypeId,
        loc: Location,
        value: &Value,
        operation: usize,
        index: Option<u32>,
    ) -> Result<Value> {
        let payload = self.dynamic_value(value)?.to_owned();
        if operation == 0 {
            let layout = self.layout(payload.type_id())?;
            if layout.kind != Kind::Record || layout.dynamic_kind != Some("Dict") {
                return Err("Dyn field access expects Struct".into());
            }
            let index = index.ok_or("Dyn field index missing")? as usize;
            if index >= layout.fields.len() {
                return Err(format!("field index {index} is out of range"));
            }
            let child = self.field(&payload, index)?.to_owned();
            return self.dynamic(output, child.location(), &child);
        }
        let (_, child) = self.dynamic_variant(&payload)?;
        let actual = if self.layout(payload.type_id())?.kind == Kind::Scalar {
            self.scalar_bits(payload.as_ref())? as u32
        } else {
            self.enum_tag(&payload)?
        };
        if operation == 1 {
            return self.scalar(output, payload.location(), u64::from(actual));
        }
        let expected = index.ok_or("Dyn variant index missing")?;
        if actual != expected {
            return Err(format!("Dyn variant index is {actual}, not {expected}"));
        }
        match child {
            Some(child) => {
                let dynamic =
                    self.dynamic(self.layout(output)?.arguments[0], child.location(), &child)?;
                self.named_variant(output, loc, "Some", Some(&dynamic))
            }
            None => self.named_variant(output, loc, "None", None),
        }
    }
    pub(super) fn named_variant(
        &mut self,
        ty: TypeId,
        loc: Location,
        name: &str,
        payload: Option<&Value>,
    ) -> Result<Value> {
        let index = self
            .layout(ty)?
            .variants
            .iter()
            .position(|v| v.name == name)
            .ok_or("native result variant missing")?;
        self.enum_value(ty, loc, index as u32, payload)
    }
    fn dynamic_variant(&self, value: &Value) -> Result<(String, Option<Value>)> {
        let layout = self.layout(value.type_id())?;
        if layout.kind == Kind::Scalar && layout.dynamic_kind == Some("Atom") {
            return match self.scalar_bits(value.as_ref())? {
                0 => Ok(("False".into(), None)),
                1 => Ok(("True".into(), None)),
                _ => Err("invalid Bool variant".into()),
            };
        }
        if layout.kind != Kind::Enum {
            return Err("Dyn variant access expects Enum".into());
        }
        let index = self.enum_tag(value)?;
        Ok((
            self.variant_name(value.type_id(), index)?.to_owned(),
            self.enum_payload(value)?.map(ValueRef::to_owned),
        ))
    }
    /// Build only the collection requested by reflection; child object graphs
    /// remain in their original tables and retain their own origins.
    pub fn dynamic_query(
        &mut self,
        result: TypeId,
        loc: Location,
        value: &Value,
        query: DynamicQuery,
    ) -> Result<Value> {
        let payload = self.dynamic_value(value)?.to_owned();
        let result_args = self.layout(result)?.arguments.clone();
        if result_args.len() != 2 {
            return Err("Dyn query needs Result output".into());
        }
        let output = result_args[0];
        let outcome = (|| -> Result<Value> {
            match query {
                DynamicQuery::Tag | DynamicQuery::Payload => {
                    let (tag, child) = self.dynamic_variant(&payload)?;
                    if matches!(query, DynamicQuery::Tag) {
                        return self.string(output, loc, &tag);
                    }
                    match child {
                        Some(child) => {
                            let dynamic = self.dynamic(
                                self.layout(output)?.arguments[0],
                                child.location(),
                                &child,
                            )?;
                            self.named_variant(output, loc, "Some", Some(&dynamic))
                        }
                        None => self.named_variant(output, loc, "None", None),
                    }
                }
                DynamicQuery::ArrayItems | DynamicQuery::TupleItems => {
                    let layout = self.layout(payload.type_id())?;
                    let array = matches!(query, DynamicQuery::ArrayItems);
                    let count = if array && layout.kind == Kind::Array {
                        self.array_len(&payload)?
                    } else if !array && layout.dynamic_kind == Some("Tuple") {
                        layout.fields.len()
                    } else {
                        return Err("Dyn sequence access has the wrong type".into());
                    };
                    let dynamic = self.layout(output)?.arguments[0];
                    let mut values = Vec::with_capacity(count);
                    for index in 0..count {
                        let child = if array {
                            self.array_get(&payload, index)?
                        } else {
                            self.field(&payload, index)?
                        }
                        .to_owned();
                        values.push(self.dynamic(dynamic, child.location(), &child)?);
                    }
                    self.array(output, loc, &values)
                }
                DynamicQuery::Fields => {
                    let layout = self.layout(payload.type_id())?;
                    let dict = layout.kind == Kind::Dict;
                    if !dict
                        && !(layout.kind == Kind::Record && layout.dynamic_kind == Some("Dict"))
                    {
                        return Err("Dyn field access expects Struct".into());
                    }
                    let count = if dict {
                        self.dict_len(&payload)?
                    } else {
                        layout.fields.len()
                    };
                    let pair = self.layout(output)?.arguments[0];
                    let pair_args = self.layout(pair)?.arguments.clone();
                    let mut values = Vec::with_capacity(count);
                    for index in 0..count {
                        let (name, child) = if dict {
                            let (name, child) = self.dict_entry(&payload, index)?;
                            (name.to_owned(), child.to_owned())
                        } else {
                            let name = self.layout(payload.type_id())?.field_names[index].clone();
                            (
                                self.string(pair_args[0], payload.location(), &name)?,
                                self.field(&payload, index)?.to_owned(),
                            )
                        };
                        let child = self.dynamic(pair_args[1], child.location(), &child)?;
                        values.push(self.aggregate(pair, loc, &[name, child])?);
                    }
                    self.array(output, loc, &values)
                }
            }
        })();
        let (tag, payload) = match outcome {
            Ok(value) => ("Ok", value),
            Err(message) => ("Err", self.string(result_args[1], loc, &message)?),
        };
        self.named_variant(result, loc, tag, Some(&payload))
    }
    pub fn dynamic_field(&self, value: &Value, name: &Value) -> Result<ValueRef<'_>> {
        let payload = self.dynamic_value(value)?.to_owned();
        let layout = self.layout(payload.type_id())?;
        let text = self.text(name.as_ref())?;
        let found = if layout.kind == Kind::Dict {
            self.dict_get(&payload, name)?
        } else if layout.kind == Kind::Record && layout.dynamic_kind == Some("Dict") {
            layout
                .field_names
                .iter()
                .position(|n| n == text.as_str())
                .map(|index| self.field(&payload, index))
                .transpose()?
        } else {
            return Err("Dyn field access expects Struct".into());
        };
        found.ok_or_else(|| format!("Dyn record has no field {:?}", text.as_str()))
    }
    pub fn dynamic_kind(&self, value: &Value) -> Result<&'static str> {
        let payload = self.dynamic_value(value)?;
        let layout = self.layout(payload.type_id())?;
        if let Some(kind) = layout.dynamic_kind {
            return Ok(kind);
        }
        if layout.kind == Kind::Enum {
            return Ok(if self.enum_payload_ref(payload)?.is_some() {
                "Tagged"
            } else {
                "Atom"
            });
        }
        Err("Dyn witness is not an executable value type".into())
    }
    /// Box only the fixed-width descriptor. Its referenced objects stay shared.
    pub fn dynamic(&mut self, ty: TypeId, loc: Location, value: &Value) -> Result<Value> {
        self.expect(ty, Kind::Dyn)?;
        self.validate(value.as_ref(), value.type_id())?;
        let heap = self.push_words(Table::Values, value.words().to_vec())?;
        self.pack(
            ty,
            loc,
            &[
                u64::from(value.type_id().raw()) | (1u64 << 32),
                u64::from(heap),
                0,
            ],
        )
    }

    pub fn dynamic_value<'a>(&'a self, value: &Value) -> Result<ValueRef<'a>> {
        self.validate(value.as_ref(), value.type_id())?;
        self.expect(value.type_id(), Kind::Dyn)?;
        if value.words[2] >> 32 != 1 || value.words[4] != 0 {
            return Err("unsupported native Dyn storage".into());
        }
        let ty = TypeId(value.words[2] as u32);
        let heap = u32::try_from(value.words[3]).map_err(|_| "invalid Dyn HeapId")?;
        let result = ValueRef {
            arena: self.identity,
            words: self.object_words(Table::Values, heap)?,
        };
        self.validate(result, ty)?;
        Ok(result)
    }
}
