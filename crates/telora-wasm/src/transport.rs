//! Typed host handles into Wasm memory; no host copy of language object graphs.
use crate::{abi, artifact::Kind, output::Output, session::Session};

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
        if value.ty != ty || self.output().word(value.pointer as u64 + abi::TYPE)? != ty {
            return Err("Wasm: host value differs from sealed protocol type".into());
        }
        self.output()
            .bytes(value.pointer as u64, desc.bytes as u64)?;
        Ok(())
    }

    pub(crate) fn pair(&self, value: Value) -> Result<(Value, Value), String> {
        self.expect_value(value, value.ty)?;
        let desc = &self.manifest.types[value.ty as usize];
        if desc.kind != Kind::Tuple || desc.fields.len() != 2 {
            return Err("Wasm: service transition requires a sealed pair".into());
        }
        let output = self.output();
        let (base, bytes) =
            output.payload(abi::RECORDS, output.word(value.pointer as u64 + abi::DATA)?)?;
        let field = |index: usize| -> Result<Value, String> {
            let field = &desc.fields[index];
            if field.offset as u64 + self.manifest.types[field.ty as usize].bytes as u64 > bytes {
                return Err("Wasm: transition field exceeds tuple".into());
            }
            let value = Value {
                pointer: u32::try_from(base + field.offset as u64)
                    .map_err(|_| "Wasm: field address overflow")?,
                ty: field.ty,
            };
            self.expect_value(value, field.ty)?;
            Ok(value)
        };
        Ok((field(0)?, field(1)?))
    }

    pub(crate) fn invoke_values(
        &mut self,
        closure: Value,
        arguments: &[Value],
    ) -> Result<Value, String> {
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
                .checked_mul(4)
                .ok_or("Wasm: argument size overflow")?,
        )?;
        for (index, value) in arguments.iter().enumerate() {
            self.write(args as usize + index * 4, &value.pointer.to_le_bytes())?;
        }
        let invoke = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&self.store, "telora_invoke")
            .map_err(|e| e.to_string())?;
        let pointer = invoke
            .call(&mut self.store, (closure.pointer as i32, args as i32))
            .map_err(|e| e.to_string())? as u32;
        if pointer == 0 {
            return Err(self.failure());
        }
        let value = Value { pointer, ty };
        self.expect_value(value, ty)?;
        Ok(value)
    }
}
