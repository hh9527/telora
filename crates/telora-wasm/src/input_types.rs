//! Host transport for closed algebraic and dictionary inputs.
use crate::{
    abi::*,
    artifact::{Kind, TypeDesc},
    session::Session,
};
use serde_json::Value;

impl Session {
    pub(crate) fn input_dict(
        &mut self,
        pointer: u32,
        desc: &TypeDesc,
        value: &Value,
        depth: usize,
    ) -> Result<(), String> {
        let items = value.as_object().ok_or("Wasm: expected Dict input")?;
        let element = *desc
            .arguments
            .first()
            .ok_or("Wasm: missing Dict value type")?;
        let string = self
            .manifest
            .types
            .iter()
            .position(|ty| ty.kind == Kind::String)
            .ok_or("Wasm: missing String type")? as u32;
        let length = u32::try_from(items.len()).map_err(|_| "Wasm: Dict length overflow")?;
        let stride = self.manifest.types[element as usize].bytes;
        let key_bytes = length
            .checked_mul(32)
            .ok_or("Wasm: Dict key size overflow")?;
        let value_bytes = length
            .checked_mul(stride)
            .ok_or("Wasm: Dict value size overflow")?;
        let keys = self.allocate(key_bytes as usize)?;
        let values = self.allocate(value_bytes as usize)?;
        let mut items = items.iter().collect::<Vec<_>>();
        items.sort_by(|(a, _), (b, _)| a.as_bytes().cmp(b.as_bytes()));
        for (index, (key, value)) in items.into_iter().enumerate() {
            let key = self.input(string, &Value::String(key.clone()), depth + 1)?;
            let value = self.input(element, value, depth + 1)?;
            self.copy_input(keys + index as u32 * 32, key, 32)?;
            self.copy_input(values + index as u32 * stride, value, stride as usize)?;
        }
        let keys = self.push_input(ARRAYS, keys, key_bytes)?;
        let values = self.push_input(ARRAYS, values, value_bytes)?;
        self.write(pointer as usize + 16, &keys.to_le_bytes())?;
        self.write(pointer as usize + 20, &length.to_le_bytes())?;
        self.write(pointer as usize + 24, &values.to_le_bytes())
    }
    pub(crate) fn input_enum(
        &mut self,
        pointer: u32,
        desc: &TypeDesc,
        value: &Value,
        depth: usize,
    ) -> Result<(), String> {
        let (name, payload) = match desc.kind {
            Kind::Value => (
                match value {
                    Value::Null => "None",
                    Value::Bool(true) => "True",
                    Value::Bool(false) => "False",
                    Value::Number(n) if n.is_i64() || n.is_u64() => "Int",
                    Value::Number(_) => "Float",
                    Value::String(_) => "String",
                    Value::Array(_) => "Array",
                    Value::Object(_) => "Object",
                },
                Some(value),
            ),
            Kind::Option => (if value.is_null() { "None" } else { "Some" }, Some(value)),
            _ => match value {
                Value::String(name) => (name.as_str(), None),
                Value::Object(items) if items.len() == 1 => {
                    let (name, payload) = items.iter().next().unwrap();
                    (name.as_str(), Some(payload))
                }
                _ => return Err("Wasm: expected enum name or single tagged payload".into()),
            },
        };
        let index = desc
            .variants
            .iter()
            .position(|v| v.name == name)
            .ok_or("Wasm: input enum variant is absent")?;
        let branch = &desc.variants[index];
        if desc.kind == Kind::Enum && branch.ty.is_some() != payload.is_some() {
            return Err("Wasm: enum input payload arity mismatch".into());
        }
        self.write(pointer as usize + 16, &(index as u64).to_le_bytes())?;
        if let Some(ty) = branch.ty {
            let payload = self.input(
                ty,
                payload.ok_or("Wasm: enum input payload missing")?,
                depth + 1,
            )?;
            let width = self.manifest.types[ty as usize].bytes;
            if branch.boxed {
                let id = self.push_input(VALUES, payload, width)?;
                self.write(pointer as usize + 24, &id.to_le_bytes())?;
            } else {
                self.copy_input(pointer + 24, payload, width as usize)?;
            }
        }
        Ok(())
    }
}
