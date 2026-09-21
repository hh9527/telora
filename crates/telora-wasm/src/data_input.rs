//! Source-backed data transport. Materialize once, then inject before initialization.
use crate::{abi::*, artifact::Manifest, plan::Plan, session::Session};
use wasm_encoder::{BlockType, Function, Instruction as I};

pub(crate) fn injector(plan: &Plan, manifest: &Manifest) -> Function {
    let mut function = Function::new([]);
    for instruction in [
        I::GlobalGet(PHASE_GLOBAL),
        I::If(BlockType::Empty),
        I::I32Const(0),
        I::Return,
        I::End,
    ] {
        function.instruction(&instruction);
    }
    for module in &manifest.data_modules {
        let key = plan
            .globals
            .iter()
            .find(|(symbol, _)| symbol.index() == module.symbol as usize)
            .unwrap()
            .1;
        let offset = plan.demands[key];
        for instruction in [
            I::LocalGet(0),
            I::I32Const(module.symbol as i32),
            I::I32Eq,
            I::If(BlockType::Empty),
            I::I32Const(offset as i32),
            I::I32Load(memory(0, 2)),
            I::If(BlockType::Empty),
            I::I32Const(0),
            I::Return,
            I::End,
            I::I32Const(offset as i32),
            I::LocalGet(1),
            I::I32Store(memory(4, 2)),
            I::I32Const(offset as i32),
            I::I32Const(2),
            I::I32Store(memory(0, 2)),
            I::I32Const(1),
            I::Return,
            I::End,
        ] {
            function.instruction(&instruction);
        }
    }
    function.instruction(&I::I32Const(0)).instruction(&I::End);
    function
}

impl Session {
    pub fn inject_data_value(
        &mut self,
        symbol: u32,
        value: crate::transport::Value,
    ) -> Result<(), String> {
        let ty = self
            .manifest
            .data_modules
            .iter()
            .find(|module| module.symbol == symbol)
            .ok_or("Wasm: data module is not in the executable")?
            .ty;
        self.expect_value(value, ty)?;
        let pointer = value.pointer;
        let inject = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&self.store, "telora_inject_data")
            .map_err(|e| e.to_string())?;
        if inject
            .call(&mut self.store, (symbol as i32, pointer as i32))
            .map_err(|e| e.to_string())?
            != 1
        {
            return Err(
                "Wasm: data module must be injected exactly once before initialization".into(),
            );
        }
        Ok(())
    }
}
