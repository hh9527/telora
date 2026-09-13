//! External JSON boundary reads the artifact's closed schema and Wasm memory.
use crate::{
    abi::*,
    artifact::{Kind, Manifest},
};
use serde_json::Value;

pub(crate) struct Output<'a> {
    pub memory: &'a [u8],
    pub manifest: &'a Manifest,
}

impl Output<'_> {
    pub(crate) fn field(&self, pointer: u64, ty: u32, name: &str) -> Result<(u32, u32), String> {
        if self.word(pointer + TYPE)? != ty {
            return Err("Wasm: record type differs from sealed contract".into());
        }
        let desc = self
            .manifest
            .types
            .get(ty as usize)
            .ok_or("Wasm: invalid record type")?;
        if desc.kind != Kind::Record {
            return Err("Wasm: contract requires record".into());
        }
        let field = desc
            .fields
            .iter()
            .find(|f| f.name == name)
            .ok_or_else(|| format!("Wasm: contract field {name:?} missing"))?;
        let (base, bytes) = self.payload(RECORDS, self.word(pointer + DATA)?)?;
        if field.offset as u64 + self.manifest.types[field.ty as usize].bytes as u64 > bytes {
            return Err("Wasm: contract field exceeds record".into());
        }
        Ok((
            u32::try_from(base + field.offset as u64)
                .map_err(|_| "Wasm: field address overflow")?,
            field.ty,
        ))
    }
    fn bytes(&self, address: u64, length: u64) -> Result<&[u8], String> {
        let end = address
            .checked_add(length)
            .ok_or("Wasm: output range overflow")?;
        self.memory
            .get(
                usize::try_from(address).map_err(|_| "Wasm: address overflow")?
                    ..usize::try_from(end).map_err(|_| "Wasm: address overflow")?,
            )
            .ok_or_else(|| "Wasm: output exceeds linear memory".into())
    }
    fn word(&self, address: u64) -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            self.bytes(address, 4)?.try_into().unwrap(),
        ))
    }
    fn payload(&self, table: u32, id: u32) -> Result<(u64, u64), String> {
        let descriptor = table_address(table) as u64;
        if id >= self.word(descriptor + 4)? {
            return Err("Wasm: output has invalid HeapId".into());
        }
        let item = self.word(descriptor)? as u64 + id as u64 * 8;
        Ok((self.word(item)? as u64, self.word(item + 4)? as u64))
    }
    pub fn json(&self, pointer: u64, expected: u32, depth: usize) -> Result<Value, String> {
        if depth > 512 {
            return Err("Wasm: JSON output nesting limit".into());
        }
        if self.word(pointer + TYPE)? != expected {
            return Err("Wasm: output differs from its sealed type".into());
        }
        let ty = self
            .manifest
            .types
            .get(expected as usize)
            .ok_or("Wasm: invalid output TypeId")?;
        self.bytes(pointer, ty.bytes as u64)?;
        Ok(match ty.kind {
            Kind::Unit => Value::Null,
            Kind::Int => {
                i64::from_le_bytes(self.bytes(pointer + DATA, 8)?.try_into().unwrap()).into()
            }
            Kind::Bool => (self.word(pointer + DATA)? != 0).into(),
            Kind::Float => {
                let bits = u64::from_le_bytes(self.bytes(pointer + DATA, 8)?.try_into().unwrap());
                serde_json::Number::from_f64(f64::from_bits(bits))
                    .ok_or("Wasm: non-finite Float cannot be serialized")?
                    .into()
            }
            Kind::String => self.text(pointer)?.into(),
            Kind::Array => {
                let (base, bytes) = self.payload(ARRAYS, self.word(pointer + DATA)?)?;
                let start = self.word(pointer + 20)? as u64;
                let end = self.word(pointer + 24)? as u64;
                let element = *ty
                    .arguments
                    .first()
                    .ok_or("Wasm: array element type missing")?;
                let stride = self
                    .manifest
                    .types
                    .get(element as usize)
                    .ok_or("Wasm: invalid element type")?
                    .bytes as u64;
                if start > end || end * stride > bytes || (stride == 0 && end != 0) {
                    return Err("Wasm: invalid array slice".into());
                }
                let mut result = vec![];
                for index in start..end {
                    result.push(self.json(base + index * stride, element, depth + 1)?);
                }
                Value::Array(result)
            }
            Kind::Tuple | Kind::Record => {
                let (base, bytes) = self.payload(RECORDS, self.word(pointer + DATA)?)?;
                let mut items = vec![];
                let mut fields = serde_json::Map::new();
                for field in &ty.fields {
                    let width = self
                        .manifest
                        .types
                        .get(field.ty as usize)
                        .ok_or("Wasm: invalid field type")?
                        .bytes as u64;
                    if field.offset as u64 + width > bytes {
                        return Err("Wasm: field exceeds record".into());
                    }
                    let value = self.json(base + field.offset as u64, field.ty, depth + 1)?;
                    if ty.kind == Kind::Tuple {
                        items.push(value);
                    } else {
                        fields.insert(field.name.clone(), value);
                    }
                }
                if ty.kind == Kind::Tuple {
                    Value::Array(items)
                } else {
                    Value::Object(fields)
                }
            }
            Kind::Dict => {
                let (keys, key_bytes) = self.payload(ARRAYS, self.word(pointer + DATA)?)?;
                let (values, value_bytes) = self.payload(ARRAYS, self.word(pointer + 24)?)?;
                let length = self.word(pointer + 20)? as u64;
                let element = *ty
                    .arguments
                    .first()
                    .ok_or("Wasm: missing dictionary value type")?;
                let stride = self
                    .manifest
                    .types
                    .get(element as usize)
                    .ok_or("Wasm: invalid dictionary value type")?
                    .bytes as u64;
                if length * 32 != key_bytes || length * stride != value_bytes {
                    return Err("Wasm: invalid dictionary columns".into());
                }
                let mut fields = serde_json::Map::new();
                let mut previous: Option<String> = None;
                for index in 0..length {
                    let key = self.text(keys + index * 32)?;
                    if previous.as_ref().is_some_and(|p| p >= &key) {
                        return Err("Wasm: dictionary keys are not strictly ordered".into());
                    }
                    previous = Some(key.clone());
                    fields.insert(key, self.json(values + index * stride, element, depth + 1)?);
                }
                Value::Object(fields)
            }
            Kind::Option | Kind::Enum | Kind::Value => {
                let index = self.word(pointer + DATA)? as usize;
                let branch = ty.variants.get(index).ok_or("Wasm: invalid enum tag")?;
                let payload = match branch.ty {
                    Some(payload_ty) => {
                        let address = if branch.boxed {
                            self.payload(VALUES, self.word(pointer + 24)?)?.0
                        } else {
                            pointer + 24
                        };
                        Some(self.json(address, payload_ty, depth + 1)?)
                    }
                    None => None,
                };
                if ty.kind == Kind::Value {
                    match branch.name.as_str() {
                        "None" => Value::Null,
                        "True" => true.into(),
                        "False" => false.into(),
                        "Int" | "Float" | "String" | "Array" | "Object" | "LocalDate"
                        | "LocalTime" | "LocalDateTime" | "OffsetDateTime" => {
                            payload.ok_or("Wasm: semantic Value payload missing")?
                        }
                        "Bytes" => {
                            return Err("Value.Bytes cannot be emitted as semantic JSON".into());
                        }
                        _ => return Err("Wasm: unknown semantic Value variant".into()),
                    }
                } else if ty.kind == Kind::Option {
                    payload.unwrap_or(Value::Null)
                } else {
                    match payload {
                        Some(value) => {
                            let mut fields = serde_json::Map::new();
                            fields.insert(branch.name.clone(), value);
                            Value::Object(fields)
                        }
                        None => branch.name.clone().into(),
                    }
                }
            }
            Kind::Newtype => {
                let field = ty
                    .fields
                    .first()
                    .ok_or("Wasm: newtype has no payload layout")?;
                let (address, _) = self.payload(NEWTYPES, self.word(pointer + DATA)?)?;
                self.json(address, field.ty, depth + 1)?
            }
            _ => return Err("Wasm: result JSON encoding is not implemented for this type".into()),
        })
    }
    fn text(&self, pointer: u64) -> Result<String, String> {
        let header = self.bytes(pointer + DATA, 16)?;
        let bytes = match header[0] {
            0 => {
                let length = header[1] as usize;
                if length > 14 {
                    return Err("Wasm: invalid inline string".into());
                }
                &header[2..2 + length]
            }
            1 => {
                let (base, length) = self.payload(STRINGS, self.word(pointer + 20)?)?;
                let start = self.word(pointer + 24)? as u64;
                let end = self.word(pointer + 28)? as u64;
                if start > end || end > length {
                    return Err("Wasm: invalid string slice".into());
                }
                self.bytes(base + start, end - start)?
            }
            _ => return Err("Wasm: invalid string representation".into()),
        };
        String::from_utf8(bytes.to_vec()).map_err(|_| "Wasm: invalid UTF-8 output".into())
    }
}
