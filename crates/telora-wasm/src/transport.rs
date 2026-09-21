//! Typed host handles into Wasm memory; no host copy of language object graphs.
use crate::{artifact::Kind, output::Output, session::Session};

/// A borrowed language handle. Initialization compacts its owned graph: inject
/// pre-initialization inputs into their demand slots, then acquire fresh handles
/// afterwards. Work collection similarly returns replacements for explicit roots.
#[derive(Clone, Copy, Debug)]
pub struct Value {
    pub(crate) pointer: u32,
    pub(crate) ty: u32,
}

impl Value {
    pub fn type_id(self) -> u32 {
        self.ty
    }
}

impl Session {
    /// Read a closed, initialized global by SymbolId. No name lookup or evaluation.
    pub fn initialized_global(&self, symbol: u32) -> Result<Value, String> {
        let global = self
            .manifest
            .globals
            .iter()
            .find(|global| global.symbol == symbol)
            .ok_or("Wasm: global is outside the sealed executable")?;
        let output = self.output();
        if output.word(global.demand as u64)? != 2 {
            return Err("Wasm: global is not initialized".into());
        }
        let value = Value {
            pointer: output.word(global.demand as u64 + 4)?,
            ty: global.ty,
        };
        self.expect_value(value, global.ty)?;
        Ok(value)
    }

    pub(crate) fn output(&self) -> Output<'_> {
        Output {
            memory: self.memory.data(&self.store),
            manifest: &self.manifest,
        }
    }

    pub fn input_value(&mut self, ty: u32, value: &serde_json::Value) -> Result<Value, String> {
        Ok(Value {
            pointer: self.input(ty, value, 0)?,
            ty,
        })
    }

    pub fn output_value(&self, value: Value) -> Result<serde_json::Value, String> {
        self.output().json(value.pointer as u64, value.ty, 0)
    }

    pub fn value_field(&self, value: Value, name: &str) -> Result<Value, String> {
        let (pointer, ty) = self.output().field(value.pointer as u64, value.ty, name)?;
        Ok(Value { pointer, ty })
    }

    pub(crate) fn expect_value(&self, value: Value, ty: u32) -> Result<(), String> {
        let desc = self
            .manifest
            .types
            .get(ty as usize)
            .ok_or("Wasm: invalid transport type")?;
        if value.ty != ty {
            return Err("Wasm: host value differs from sealed protocol type".into());
        }
        self.output()
            .bytes(value.pointer as u64, desc.bytes as u64)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn invoke_values(
        &mut self,
        closure: Value,
        arguments: &[Value],
    ) -> Result<Value, String> {
        self.invoke_testable(closure, arguments)?
            .ok_or_else(|| self.failure())
    }

    /// A normal null return is a language failure; traps remain outer errors.
    pub(crate) fn invoke_testable(
        &mut self,
        closure: Value,
        arguments: &[Value],
    ) -> Result<Option<Value>, String> {
        self.expect_value(closure, closure.ty)?;
        let desc = &self.manifest.types[closure.ty as usize];
        if desc.kind != Kind::Function || desc.arguments.len() != arguments.len() + 1 {
            return Err("Wasm: invocation differs from sealed signature".into());
        }
        for (&value, &ty) in arguments.iter().zip(&desc.arguments) {
            self.expect_value(value, ty)?;
        }
        let ty = *desc.arguments.last().unwrap();
        let args = self.allocate(
            arguments
                .len()
                .checked_add(1)
                .ok_or("Wasm: argument count overflow")?
                .checked_mul(4)
                .ok_or("Wasm: argument size overflow")?,
        )?;
        for (index, value) in arguments.iter().enumerate() {
            self.write(args as usize + index * 4, &value.pointer.to_le_bytes())?;
        }
        self.write(args as usize + arguments.len() * 4, &0u32.to_le_bytes())?;
        let invoke = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&self.store, "telora_invoke")
            .map_err(|e| e.to_string())?;
        let pointer = invoke
            .call(&mut self.store, (closure.pointer as i32, args as i32))
            .map_err(|e| e.to_string())? as u32;
        if pointer == 0 {
            return Ok(None);
        }
        let value = Value { pointer, ty };
        self.expect_value(value, ty)?;
        Ok(Some(value))
    }
}
