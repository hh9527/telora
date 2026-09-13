//! Value assembly at the external data boundary. Objects remain in Wasm memory.
use crate::{abi::*, artifact::Kind, session::Session};

impl Session {
    fn input_header(&mut self, ty: u32) -> Result<u32, String> {
        let bytes = self
            .manifest
            .types
            .get(ty as usize)
            .ok_or("Wasm: invalid input type")?
            .bytes;
        if bytes < HEADER_BYTES {
            return Err("Wasm: input type has no value layout".into());
        }
        let value = self.allocate(bytes as usize)?;
        self.write(value as usize + 12, &ty.to_le_bytes())?;
        Ok(value)
    }
    pub(crate) fn input_variant(
        &mut self,
        ty: u32,
        index: usize,
        payload: Option<u32>,
    ) -> Result<u32, String> {
        let variant = self.manifest.types[ty as usize]
            .variants
            .get(index)
            .ok_or("Wasm: invalid input variant")?
            .clone();
        if variant.ty.is_some() != payload.is_some() {
            return Err("Wasm: input variant payload mismatch".into());
        }
        let value = self.input_header(ty)?;
        self.write(value as usize + 16, &(index as u64).to_le_bytes())?;
        if let (Some(payload), Some(payload_ty)) = (payload, variant.ty) {
            let bytes = self.manifest.types[payload_ty as usize].bytes;
            if variant.boxed {
                let id = self.push_input(VALUES, payload, bytes)?;
                self.write(value as usize + 24, &id.to_le_bytes())?;
            } else {
                self.copy_input(value + 24, payload, bytes as usize)?;
            }
        }
        Ok(value)
    }
    pub(crate) fn input_array_values(&mut self, ty: u32, items: &[u32]) -> Result<u32, String> {
        let desc = &self.manifest.types[ty as usize];
        if desc.kind != Kind::Array {
            return Err("Wasm: expected array layout".into());
        }
        let stride = self.manifest.types[desc.arguments[0] as usize].bytes;
        let length = u32::try_from(items.len()).map_err(|_| "Wasm: input array length overflow")?;
        let bytes = length
            .checked_mul(stride)
            .ok_or("Wasm: input array size overflow")?;
        let data = self.allocate(bytes as usize)?;
        for (index, &value) in items.iter().enumerate() {
            self.copy_input(data + index as u32 * stride, value, stride as usize)?;
        }
        let id = self.push_input(ARRAYS, data, bytes)?;
        let result = self.input_header(ty)?;
        self.write(result as usize + 16, &id.to_le_bytes())?;
        self.write(result as usize + 24, &length.to_le_bytes())?;
        Ok(result)
    }
    /// Caller provides keys in strict UTF-8 order, with their source locations.
    pub(crate) fn input_dict_values(
        &mut self,
        ty: u32,
        pairs: &[(u32, u32)],
    ) -> Result<u32, String> {
        let desc = &self.manifest.types[ty as usize];
        if desc.kind != Kind::Dict {
            return Err("Wasm: expected dictionary layout".into());
        }
        let stride = self.manifest.types[desc.arguments[0] as usize].bytes;
        let length = u32::try_from(pairs.len()).map_err(|_| "Wasm: dictionary length overflow")?;
        let key_bytes = length
            .checked_mul(32)
            .ok_or("Wasm: dictionary keys size overflow")?;
        let value_bytes = length
            .checked_mul(stride)
            .ok_or("Wasm: dictionary values size overflow")?;
        let keys = self.allocate(key_bytes as usize)?;
        let values = self.allocate(value_bytes as usize)?;
        for (index, &(key, value)) in pairs.iter().enumerate() {
            self.copy_input(keys + index as u32 * 32, key, 32)?;
            self.copy_input(values + index as u32 * stride, value, stride as usize)?;
        }
        let keys = self.push_input(ARRAYS, keys, key_bytes)?;
        let values = self.push_input(ARRAYS, values, value_bytes)?;
        let result = self.input_header(ty)?;
        self.write(result as usize + 16, &keys.to_le_bytes())?;
        self.write(result as usize + 20, &length.to_le_bytes())?;
        self.write(result as usize + 24, &values.to_le_bytes())?;
        Ok(result)
    }
    pub(crate) fn input_record_values(
        &mut self,
        ty: u32,
        fields: &std::collections::BTreeMap<&str, u32>,
    ) -> Result<u32, String> {
        let desc = self.manifest.types[ty as usize].clone();
        if desc.kind != Kind::Record || fields.len() != desc.fields.len() {
            return Err("Wasm: input record shape mismatch".into());
        }
        let bytes = desc
            .fields
            .iter()
            .map(|f| {
                f.offset
                    .checked_add(self.manifest.types[f.ty as usize].bytes)
                    .ok_or("Wasm: record input size overflow")
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .max()
            .unwrap_or(0);
        let data = self.allocate(bytes as usize)?;
        for field in &desc.fields {
            let pointer = *fields
                .get(field.name.as_str())
                .ok_or("Wasm: missing input record field")?;
            self.copy_input(
                data + field.offset,
                pointer,
                self.manifest.types[field.ty as usize].bytes as usize,
            )?;
        }
        let id = self.push_input(RECORDS, data, bytes)?;
        let result = self.input_header(ty)?;
        self.write(result as usize + 16, &id.to_le_bytes())?;
        Ok(result)
    }
    pub(crate) fn input_bytes(&mut self, ty: u32, bytes: &[u8]) -> Result<u32, String> {
        if self.manifest.types[ty as usize].kind != Kind::Bytes {
            return Err("Wasm: expected Bytes layout".into());
        }
        let length = u32::try_from(bytes.len()).map_err(|_| "Wasm: byte input size overflow")?;
        let data = self.allocate(bytes.len())?;
        self.write(data as usize, bytes)?;
        let id = self.push_input(BYTES, data, length)?;
        let result = self.input_header(ty)?;
        self.write(result as usize + 16, &id.to_le_bytes())?;
        self.write(result as usize + 24, &length.to_le_bytes())?;
        Ok(result)
    }
}
