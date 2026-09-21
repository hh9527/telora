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
    pub(crate) fn location_words(&self, pointer: u64) -> Result<[u32; 5], String> {
        let bytes: [u8; 8] = self.bytes(pointer, 8)?.try_into().unwrap();
        let range =
            telora_wasm_shared::source_range::SourceRange::unpack(u64::from_le_bytes(bytes))
                .ok_or("Wasm: invalid packed origin")?;
        Ok(self
            .location([range.source, range.start, range.end])?
            .unwrap_or([0; 5]))
    }

    pub(crate) fn location(&self, range: [u32; 3]) -> Result<Option<[u32; 5]>, String> {
        let [id, start, end] = range;
        if id == 0 {
            return if start == 0 && end == 0 {
                Ok(None)
            } else {
                Err("Wasm: invalid empty origin".into())
            };
        }
        if start > end {
            return Err("Wasm: invalid source range".into());
        }
        let registry = self.word(SOURCE_REGISTRY as u64)? as u64;
        let count = self.word(SOURCE_REGISTRY as u64 + 4)?;
        let mut index = None;
        for i in 0..count {
            let record = registry + u64::from(i) * 20;
            if self.raw_word(record)? == id {
                index = Some((
                    self.raw_word(record + 12)? as u64,
                    self.raw_word(record + 16)?,
                ));
                break;
            }
        }
        let (lines, count) = index.ok_or("Wasm: missing source metadata")?;
        if count == 0 {
            return Err("Wasm: missing source index".into());
        }
        self.raw_bytes(lines, u64::from(count) * 8)?;
        let point = |byte: u32| -> Result<(u32, u32), String> {
            if byte > self.raw_word(lines + u64::from(count - 1) * 8 + 4)? {
                return Err("Wasm: source offset out of range".into());
            }
            let (mut lo, mut hi) = (0, count);
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                if self.raw_word(lines + u64::from(mid) * 8)? <= byte {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            let line = lo.checked_sub(1).ok_or("Wasm: invalid source index")?;
            let start = self.raw_word(lines + u64::from(line) * 8)?;
            let end = self.raw_word(lines + u64::from(line) * 8 + 4)?;
            Ok((line, byte.min(end) - start))
        };
        let (sl, so) = point(start)?;
        let (el, eo) = point(end)?;
        Ok(Some([id, sl, so, el, eo]))
    }

    pub(crate) fn field(&self, pointer: u64, ty: u32, name: &str) -> Result<(u32, u32), String> {
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
    pub(crate) fn bytes(&self, address: u64, length: u64) -> Result<&[u8], String> {
        self.raw_bytes(self.address(address, length)?, length)
    }
    pub(crate) fn address(&self, reference: u64, length: u64) -> Result<u64, String> {
        let origin = u64::from(self.raw_word(u64::from(WORDS_ORIGIN))?);
        if reference < origin {
            return Ok(reference);
        }
        let offset = reference - origin;
        let end = offset
            .checked_add(length)
            .ok_or("Wasm: heap range overflow")?;
        let heap_length = u64::from(self.raw_word(u64::from(WORDS_VIEW + 4))?);
        if end > heap_length {
            return Err(format!(
                "Wasm: reference {reference}+{length} exceeds language heap origin {origin} length {heap_length}"
            ));
        }
        u64::from(self.raw_word(u64::from(WORDS_VIEW))?)
            .checked_add(offset)
            .ok_or_else(|| "Wasm: heap address overflow".into())
    }
    pub(crate) fn raw_word(&self, address: u64) -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            self.raw_bytes(address, 4)?.try_into().unwrap(),
        ))
    }
    pub(crate) fn raw_bytes(&self, address: u64, length: u64) -> Result<&[u8], String> {
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
    pub(crate) fn word(&self, address: u64) -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            self.bytes(address, 4)?.try_into().unwrap(),
        ))
    }
    pub(crate) fn payload(&self, table: u32, id: u32) -> Result<(u64, u64), String> {
        if matches!(table, RECORDS | VALUES | NEWTYPES) {
            let pointer = u64::from(id);
            let bytes = u64::from(self.word(pointer + 4)?);
            self.bytes(pointer + 8, bytes)?;
            return Ok((pointer + 8, bytes));
        }
        if table == ARRAYS {
            let pointer = u64::from(id);
            let ty = self.word(pointer)?;
            let data = u64::from(self.word(pointer + 4)?);
            let length = self.word(pointer + 8)?;
            let capacity = self.word(pointer + 12)?;
            if length > capacity {
                return Err("Wasm: array length exceeds capacity".into());
            }
            let width = self
                .manifest
                .types
                .get(ty as usize)
                .ok_or("Wasm: invalid array element TypeId")?
                .bytes;
            let bytes = length
                .checked_mul(width)
                .ok_or("Wasm: array size overflow")?;
            self.bytes(data, bytes.into()).map_err(|error| {
                format!(
                    "{error}; array object={id} element_type={ty} data={data} len={length} cap={capacity} width={width}"
                )
            })?;
            return Ok((data, bytes.into()));
        }
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
                let object = self.word(pointer + DATA)?;
                let (base, bytes) = self.payload(ARRAYS, object).map_err(|error| {
                    format!("{error}; Array value={pointer} type={expected} object={object}")
                })?;
                let start = self.word(pointer + DATA + 4)? as u64;
                let end = self.word(pointer + DATA + 8)? as u64;
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
                let (values, value_bytes) = self.payload(ARRAYS, self.word(pointer + DATA + 8)?)?;
                let length = self.word(pointer + DATA + 4)? as u64;
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
                if length * u64::from(STRING_BYTES) != key_bytes || length * stride != value_bytes {
                    return Err("Wasm: invalid dictionary columns".into());
                }
                let mut fields = serde_json::Map::new();
                let mut previous: Option<String> = None;
                for index in 0..length {
                    let key = self.text(keys + index * u64::from(STRING_BYTES))?;
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
                            self.payload(VALUES, self.word(pointer + DATA + 8)?)?.0
                        } else {
                            pointer + DATA + 8
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
                        "Int" | "Float" | "String" | "Array" | "Object" => {
                            payload.ok_or("Wasm: semantic Value payload missing")?
                        }
                        "LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime" => {
                            return Err(
                                "JSON cannot encode temporal values; use a codec first".into()
                            );
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
    pub(crate) fn text(&self, pointer: u64) -> Result<String, String> {
        self.text_str(pointer).map(str::to_owned)
    }
    pub(crate) fn text_str(&self, pointer: u64) -> Result<&str, String> {
        std::str::from_utf8(self.content_bytes(pointer)?)
            .map_err(|_| "Wasm: invalid UTF-8 output".into())
    }
    pub(crate) fn content_bytes(&self, pointer: u64) -> Result<&[u8], String> {
        let header = self.bytes(pointer + DATA, 16)?;
        match header[15] {
            len @ 0..=15 => Ok(&header[..usize::from(len)]),
            16 => {
                let base = u64::from(self.word(u64::from(CONTENT_VIEW))?);
                let length = u64::from(self.word(u64::from(CONTENT_VIEW + 4))?);
                let start = u64::from(self.word(pointer + DATA)?);
                let end = u64::from(self.word(pointer + DATA + 4)?);
                let raw = u64::from(self.word(pointer + DATA + 8)?);
                if raw > start
                    || start > end
                    || end > length
                    || end - start < 16
                    || header[12..15] != [0; 3]
                {
                    return Err("Wasm: invalid content slice".into());
                }
                self.raw_bytes(base + start, end - start)
            }
            _ => Err("Wasm: invalid content representation".into()),
        }
    }
}
