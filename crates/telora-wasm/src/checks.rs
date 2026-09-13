//! Construction consumes sealed checker signatures and reuses demand-initialized closures.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{HirId, NativeTypeId, PropertySite, TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I};

impl Emitter<'_> {
    pub fn construction_check(
        &mut self,
        node: HirId,
        owner: TypeId,
        site: PropertySite,
        value: u32,
    ) -> Result<(), String> {
        for (&index, &key) in &self.plan.checks {
            let check = &self.mir.construction_checks[index];
            if check.owner != owner || check.site != site {
                continue;
            }
            let signature = &self.mir.types[check.signature.index()];
            if signature.constructor != T::Function || signature.arguments.len() != 2 {
                return Err("Wasm: checker must have a sealed unary signature".into());
            }
            let argument = signature.arguments[0];
            let output = signature.arguments[1];
            let result = &self.mir.types[output.index()];
            if result.constructor != T::Result
                || result.arguments.len() != 2
                || self.mir.types[result.arguments[0].index()].constructor != T::Tuple
                || !self.mir.types[result.arguments[0].index()]
                    .arguments
                    .is_empty()
                || self.mir.types[result.arguments[1].index()].constructor
                    != T::Native(NativeTypeId::BLAME_ERROR)
            {
                return Err("Wasm: checker result is not Result((), BlameError)".into());
            }
            let mut argument_value = value;
            if self.mir.types[argument.index()].constructor == T::Unchecked {
                if self.mir.types[argument.index()].arguments != [owner]
                    || self.width(argument)? != self.width(owner)?
                {
                    return Err("Wasm: unchecked checker view contradicts sealed owner".into());
                }
                argument_value = self.value_as(node, argument, self.width(argument)?)?;
                self.copy(argument_value, 0, value, self.width(argument)?);
                self.store32(argument_value, TYPE, argument.index() as u32);
            }
            let closure = self.call_key(key)?;
            let value = self.invoke(closure, &[argument_value])?;
            let index = self.plan.layouts[output.index()]
                .variants
                .iter()
                .position(|v| v.name == "Err")
                .ok_or("Wasm: Result.Err layout missing")? as u32;
            self.extend([
                I::LocalGet(value),
                I::I32Load(memory(DATA, 2)),
                I::I32Const(index as i32),
                I::I32Eq,
                I::If(BlockType::Empty),
            ]);
            let blame = self.enum_payload(output, index, value)?;
            let (message, subjects, count) = self.blame_parts(blame);
            self.report(node, message, subjects, count, false);
            self.emit(I::End);
        }
        Ok(())
    }
}
