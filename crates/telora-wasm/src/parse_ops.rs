//! Parse primitives selected by the sealed target, with std-owned error wrappers.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::TypeConstructor as T;
use wasm_encoder::{BlockType, Instruction as I};

impl Emitter<'_> {
    pub fn parse_native(&mut self) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        if args.len() != 4
            || self.mir.types[args[0].index()].constructor != T::Type
            || self.mir.types[args[1].index()].constructor != T::TypeOf
            || self.mir.types[args[1].index()].arguments.len() != 1
            || self.mir.types[args[2].index()].constructor != T::String
            || self.mir.types[args[3].index()].constructor != T::Result
        {
            return Err("Wasm: parse_with signature mismatch".into());
        }
        let mut target = self.mir.types[args[1].index()].arguments[0];
        if self.mir.types[args[3].index()].arguments != [target, args[2]] {
            return Err("Wasm: parse_with result mismatch".into());
        }
        let mut options = Vec::new();
        while self.mir.types[target.index()].constructor == T::Option {
            options.push(target);
            target = self.mir.types[target.index()].arguments[0];
        }
        let input = self.parameter(2);
        let mut value = match self.mir.types[target.index()].constructor {
            T::String => input,
            T::Int | T::Float => {
                let integer = self.mir.types[target.index()].constructor == T::Int;
                let value = self.value_as(node, target, SCALAR_BYTES)?;
                self.copy(value, 0, input, 12);
                self.extend([
                    I::I32Const(if integer { 4 } else { 5 }),
                    I::LocalGet(input),
                    I::LocalGet(value),
                    I::I32Const(DATA as i32),
                    I::I32Add,
                    I::Call(TEXT_QUERY),
                    I::I32Eqz,
                    I::If(BlockType::Empty),
                ]);
                let message = self.text_as(
                    node,
                    args[2],
                    if integer {
                        b"$: input is not a valid Int"
                    } else {
                        b"$: input is not a finite Float"
                    },
                )?;
                let error = self.enum_value(node, args[3], 0, Some(message))?;
                self.extend([I::LocalGet(error), I::Return, I::End]);
                value
            }
            _ => return Err(format!("Wasm: ParseBy target not implemented: {target:?}")),
        };
        for option in options.into_iter().rev() {
            value = self.enum_value(node, option, 1, Some(value))?;
            self.copy(value, 0, input, 12);
        }
        self.enum_value(node, args[3], 1, Some(value))
    }
}
