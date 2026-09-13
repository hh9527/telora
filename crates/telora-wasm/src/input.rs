//! External input materialization follows the persistent, closed type schema.
use crate::{abi::*, artifact::Kind, session::Session};
use serde_json::Value;

impl Session {
    pub(crate) fn allocate(&mut self, bytes: usize) -> Result<u32, String> {
        let bytes = u32::try_from(bytes).map_err(|_| "Wasm: input size exceeds wasm32")?;
        let allocate = self
            .instance
            .get_typed_func::<i32, i32>(&self.store, "telora_alloc")
            .map_err(|e| e.to_string())?;
        Ok(allocate
            .call(&mut self.store, bytes as i32)
            .map_err(|e| e.to_string())? as u32)
    }
    pub(crate) fn write(&mut self, address: usize, bytes: &[u8]) -> Result<(), String> {
        self.memory
            .write(&mut self.store, address, bytes)
            .map_err(|e| e.to_string())
    }
    fn push_input(&mut self, table: u32, payload: u32, bytes: u32) -> Result<u32, String> {
        let push = self
            .instance
            .get_typed_func::<(i32, i32, i32), i32>(&self.store, "telora_table_push")
            .map_err(|e| e.to_string())?;
        Ok(push
            .call(
                &mut self.store,
                (table_address(table) as i32, payload as i32, bytes as i32),
            )
            .map_err(|e| e.to_string())? as u32)
    }
    fn copy_input(&mut self, to: u32, from: u32, bytes: usize) -> Result<(), String> {
        let memory = self.memory.data_mut(&mut self.store);
        let source = from as usize..from as usize + bytes;
        if source.end > memory.len() || to as usize + bytes > memory.len() {
            return Err("Wasm: input copy out of bounds".into());
        }
        memory.copy_within(source, to as usize);
        Ok(())
    }
    pub(crate) fn input(&mut self, ty: u32, value: &Value, depth: usize) -> Result<u32, String> {
        if depth > 512 {
            return Err("Wasm: input nesting limit".into());
        }
        let descriptor = self
            .manifest
            .types
            .get(ty as usize)
            .ok_or("Wasm: invalid input TypeId")?
            .clone();
        if descriptor.bytes < HEADER_BYTES {
            return Err("Wasm: input has no value layout".into());
        }
        let pointer = self.allocate(descriptor.bytes as usize)?;
        self.write(pointer as usize + TYPE as usize, &ty.to_le_bytes())?;
        match descriptor.kind {
            Kind::Unit => {
                if !value.is_null() {
                    return Err("Wasm: expected unit input".into());
                }
            }
            Kind::Int => self.write(
                pointer as usize + 16,
                &value
                    .as_i64()
                    .ok_or("Wasm: expected Int input")?
                    .to_le_bytes(),
            )?,
            Kind::Float => self.write(
                pointer as usize + 16,
                &value
                    .as_f64()
                    .ok_or("Wasm: expected Float input")?
                    .to_le_bytes(),
            )?,
            Kind::Bool => self.write(
                pointer as usize + 16,
                &u64::from(value.as_bool().ok_or("Wasm: expected Bool input")?).to_le_bytes(),
            )?,
            Kind::String => {
                let bytes = value
                    .as_str()
                    .ok_or("Wasm: expected String input")?
                    .as_bytes();
                if bytes.len() <= 14 {
                    let mut inline = [0u8; 16];
                    inline[1] = bytes.len() as u8;
                    inline[2..2 + bytes.len()].copy_from_slice(bytes);
                    self.write(pointer as usize + 16, &inline)?;
                } else {
                    let data = self.allocate(bytes.len())?;
                    self.write(data as usize, bytes)?;
                    let length = u32::try_from(bytes.len())
                        .map_err(|_| "Wasm: string input size overflow")?;
                    let id = self.push_input(STRINGS, data, length)?;
                    self.write(pointer as usize + 16, &1u32.to_le_bytes())?;
                    self.write(pointer as usize + 20, &id.to_le_bytes())?;
                    self.write(pointer as usize + 28, &length.to_le_bytes())?;
                }
            }
            Kind::Array => {
                let items = value.as_array().ok_or("Wasm: expected Array input")?;
                let element = *descriptor
                    .arguments
                    .first()
                    .ok_or("Wasm: missing array element type")?;
                let stride = self.manifest.types[element as usize].bytes;
                let length =
                    u32::try_from(items.len()).map_err(|_| "Wasm: array input length overflow")?;
                let bytes = length
                    .checked_mul(stride)
                    .ok_or("Wasm: array input size overflow")?;
                let data = self.allocate(bytes as usize)?;
                for (index, item) in items.iter().enumerate() {
                    let value = self.input(element, item, depth + 1)?;
                    self.copy_input(data + index as u32 * stride, value, stride as usize)?;
                }
                let id = self.push_input(ARRAYS, data, bytes)?;
                self.write(pointer as usize + 16, &id.to_le_bytes())?;
                self.write(pointer as usize + 24, &length.to_le_bytes())?;
            }
            Kind::Record | Kind::Tuple => {
                let valid = match descriptor.kind {
                    Kind::Record => value
                        .as_object()
                        .is_some_and(|v| v.len() == descriptor.fields.len()),
                    _ => value
                        .as_array()
                        .is_some_and(|v| v.len() == descriptor.fields.len()),
                };
                if !valid {
                    return Err("Wasm: input does not match aggregate shape".into());
                }
                let bytes = descriptor.fields.iter().try_fold(0u32, |end, field| {
                    field
                        .offset
                        .checked_add(self.manifest.types[field.ty as usize].bytes)
                        .map(|n| end.max(n))
                        .ok_or("Wasm: record input size overflow")
                })?;
                let data = self.allocate(bytes as usize)?;
                for (index, field) in descriptor.fields.iter().enumerate() {
                    let item = match descriptor.kind {
                        Kind::Record => value.get(&field.name),
                        _ => value.get(index),
                    }
                    .ok_or("Wasm: missing input field")?;
                    let value = self.input(field.ty, item, depth + 1)?;
                    let width = self.manifest.types[field.ty as usize].bytes;
                    self.copy_input(data + field.offset, value, width as usize)?;
                }
                let id = self.push_input(RECORDS, data, bytes)?;
                self.write(pointer as usize + 16, &id.to_le_bytes())?;
            }
            _ => return Err("Wasm: input encoding is not implemented for this type".into()),
        }
        Ok(pointer)
    }
}
