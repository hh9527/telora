//! Explicit ownership for Host values that must outlive another execution.
use crate::{session::Session, transport::Value};

/// An opaque, session-scoped capability. Copies become stale together on unpin.
#[derive(Clone, Copy, Debug)]
pub struct PinnedValue {
    owner: u64,
    token: u64,
    ty: u32,
}

impl Session {
    pub fn pin_value(&mut self, value: Value) -> Result<PinnedValue, String> {
        self.expect_value(value, value.ty)?;
        let pin = self
            .instance
            .get_typed_func::<(u32, u32), u64>(&self.store, "telora_gc_pin")
            .map_err(|e| e.to_string())?;
        let token = self.guest_call(pin, (value.pointer, value.ty))?;
        if token == 0 {
            return Err("Wasm: cannot pin value".into());
        }
        Ok(PinnedValue {
            owner: self.identity,
            token,
            ty: value.ty,
        })
    }

    pub fn pinned_value(&mut self, value: PinnedValue) -> Result<Value, String> {
        self.pin_operation(value, "telora_gc_pinned")
    }

    /// The returned value is borrowed; the capability cannot be used again.
    pub fn unpin_value(&mut self, value: PinnedValue) -> Result<Value, String> {
        self.pin_operation(value, "telora_gc_unpin")
    }

    fn pin_operation(&mut self, value: PinnedValue, export: &str) -> Result<Value, String> {
        if value.owner != self.identity {
            return Err("Wasm: pin belongs to another session".into());
        }
        let operation = self
            .instance
            .get_typed_func::<u64, u32>(&self.store, export)
            .map_err(|e| e.to_string())?;
        let pointer = self.guest_call(operation, value.token)?;
        if pointer == 0 {
            return Err("Wasm: stale pin".into());
        }
        let borrowed = Value {
            pointer,
            ty: value.ty,
        };
        self.expect_value(borrowed, value.ty)?;
        Ok(borrowed)
    }
}
