//! Protocol containers expose typed handles, preserving Wasm-owned data graphs.
use crate::{abi::*, artifact::Kind, session::Session, transport::Value};
use std::collections::BTreeMap;

impl Session {
    pub fn field_type(&self, ty: u32, name: &str) -> Result<u32, String> {
        self.manifest
            .types
            .get(ty as usize)
            .and_then(|d| d.fields.iter().find(|f| f.name == name))
            .map(|f| f.ty)
            .ok_or_else(|| format!("Wasm: sealed protocol field {name:?} missing"))
    }
    pub fn element_type(&self, ty: u32) -> Result<u32, String> {
        self.manifest
            .types
            .get(ty as usize)
            .and_then(|d| d.arguments.first())
            .copied()
            .ok_or("Wasm: protocol element type missing".into())
    }
    pub fn variant_type(&self, ty: u32, name: &str) -> Result<u32, String> {
        self.manifest
            .types
            .get(ty as usize)
            .and_then(|d| d.variants.iter().find(|v| v.name == name))
            .and_then(|v| v.ty)
            .ok_or_else(|| format!("Wasm: protocol payload {name:?} missing"))
    }
    pub fn array_items(&self, value: Value) -> Result<Vec<Value>, String> {
        self.expect_value(value, value.ty)?;
        let desc = &self.manifest.types[value.ty as usize];
        if desc.kind != Kind::Array {
            return Err("Wasm: protocol requires Array".into());
        }
        let ty = self.element_type(value.ty)?;
        let width = self.manifest.types[ty as usize].bytes;
        let output = self.output();
        let (base, bytes) = output.payload(ARRAYS, output.word(value.pointer as u64 + DATA)?)?;
        let start = output.word(value.pointer as u64 + 20)?;
        let end = output.word(value.pointer as u64 + 24)?;
        if start > end || width == 0 || end as u64 * width as u64 > bytes {
            return Err("Wasm: invalid protocol array".into());
        }
        (start..end)
            .map(|i| {
                Ok(Value {
                    pointer: u32::try_from(base + i as u64 * width as u64)
                        .map_err(|_| "Wasm: array offset overflow")?,
                    ty,
                })
            })
            .collect()
    }
    pub fn dict_items(&self, value: Value) -> Result<Vec<(String, Value)>, String> {
        self.expect_value(value, value.ty)?;
        if self.manifest.types[value.ty as usize].kind != Kind::Dict {
            return Err("Wasm: protocol requires Dict".into());
        }
        let ty = self.element_type(value.ty)?;
        let width = self.manifest.types[ty as usize].bytes;
        let output = self.output();
        let (keys, key_bytes) =
            output.payload(ARRAYS, output.word(value.pointer as u64 + DATA)?)?;
        let (values, value_bytes) =
            output.payload(ARRAYS, output.word(value.pointer as u64 + 24)?)?;
        let count = output.word(value.pointer as u64 + 20)?;
        if count as u64 * 32 != key_bytes || count as u64 * width as u64 != value_bytes {
            return Err("Wasm: invalid protocol dictionary".into());
        }
        (0..count)
            .map(|i| {
                Ok((
                    output.text(keys + i as u64 * 32)?,
                    Value {
                        pointer: u32::try_from(values + i as u64 * width as u64)
                            .map_err(|_| "Wasm: dictionary offset overflow")?,
                        ty,
                    },
                ))
            })
            .collect()
    }
    pub fn variant(&self, value: Value) -> Result<(&str, Option<Value>), String> {
        self.expect_value(value, value.ty)?;
        let output = self.output();
        let desc = &self.manifest.types[value.ty as usize];
        let index = output.word(value.pointer as u64 + DATA)? as usize;
        let variant = desc
            .variants
            .get(index)
            .ok_or("Wasm: invalid protocol variant")?;
        let payload = variant
            .ty
            .map(|ty| -> Result<Value, String> {
                let pointer = if variant.boxed {
                    u32::try_from(
                        output
                            .payload(VALUES, output.word(value.pointer as u64 + 24)?)?
                            .0,
                    )
                    .map_err(|_| "Wasm: variant offset overflow")?
                } else {
                    value
                        .pointer
                        .checked_add(24)
                        .ok_or("Wasm: variant overflow")?
                };
                Ok(Value { pointer, ty })
            })
            .transpose()?;
        Ok((&variant.name, payload))
    }
    pub fn text_value(&self, value: Value) -> Result<String, String> {
        self.expect_value(value, value.ty)?;
        if self.manifest.types[value.ty as usize].kind != Kind::String {
            return Err("Wasm: protocol requires String".into());
        }
        self.output().text(value.pointer as u64)
    }
    pub fn named_variant(
        &mut self,
        ty: u32,
        name: &str,
        payload: Option<Value>,
    ) -> Result<Value, String> {
        let desc = self
            .manifest
            .types
            .get(ty as usize)
            .ok_or("Wasm: invalid variant type")?;
        let index = desc
            .variants
            .iter()
            .position(|v| v.name == name)
            .ok_or("Wasm: protocol variant missing")?;
        if desc.variants[index].ty != payload.map(|v| v.ty) {
            return Err("Wasm: protocol variant payload mismatch".into());
        }
        if let Some(value) = payload {
            self.expect_value(value, value.ty)?;
        }
        Ok(Value {
            pointer: self.input_variant(ty, index, payload.map(|v| v.pointer))?,
            ty,
        })
    }
    pub fn record_value(
        &mut self,
        ty: u32,
        fields: impl IntoIterator<Item = (&'static str, Value)>,
    ) -> Result<Value, String> {
        let mut pointers = BTreeMap::new();
        for (name, value) in fields {
            self.expect_value(value, self.field_type(ty, name)?)?;
            pointers.insert(name, value.pointer);
        }
        Ok(Value {
            pointer: self.input_record_values(ty, &pointers)?,
            ty,
        })
    }
    pub fn dict_value(
        &mut self,
        ty: u32,
        string: u32,
        values: impl IntoIterator<Item = (String, Value)>,
    ) -> Result<Value, String> {
        let values: BTreeMap<_, _> = values.into_iter().collect();
        let element = self.element_type(ty)?;
        let mut pairs = vec![];
        for (name, value) in values {
            self.expect_value(value, element)?;
            pairs.push((self.input_text(string, &name)?, value.pointer));
        }
        Ok(Value {
            pointer: self.input_dict_values(ty, &pairs)?,
            ty,
        })
    }
}
