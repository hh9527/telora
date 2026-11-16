//! Host-authored structured inputs; no source registration or business text parsing.
use crate::{session::Session, transport::Value};
pub use telora_wasm_shared::located_input::{Field, InputValue, LocatedValue, Span};

impl Session {
    pub fn import_located_value(&mut self, value: &LocatedValue) -> Result<Value, String> {
        let ty = self
            .manifest
            .value_type
            .ok_or("Wasm: missing sealed Value contract")?;
        let bytes = value.encode().map_err(String::from)?;
        let length = bytes.len() as u32;
        let pointer = self.guest_call(self.exports.alloc, (length, 1))?;
        self.memory
            .write(&mut self.store, pointer as usize, &bytes)
            .map_err(|e| e.to_string())?;
        let import = self
            .instance
            .get_typed_func::<(u32, u32), u32>(&self.store, "telora_import_located")
            .map_err(|e| e.to_string())?;
        let packet = self.guest_call(import, (pointer, length))?;
        self.guest_call(self.exports.free, (pointer, length, 1))?;
        if self.output().word(packet as u64 + 12)? != 0 {
            return Err("Wasm: validated located input was rejected".into());
        }
        let materialize = self
            .instance
            .get_typed_func::<(u32, u32), u32>(&self.store, "telora_materialize_data")
            .map_err(|e| e.to_string())?;
        let pointer = self.guest_call(materialize, (packet, 0))?;
        Ok(Value { pointer, ty })
    }
}
