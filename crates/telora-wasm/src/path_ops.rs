use crate::{abi::*, emit::Emitter};
use telora_core::mir::TypeConstructor as T;
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub fn path_native(&mut self, name: &str) -> Result<u32, String> {
        let operation = ["join", "normalize", "parent", "file_name"]
            .iter()
            .position(|&candidate| candidate == name)
            .ok_or_else(|| format!("Wasm: unknown Path native: {name}"))?;
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        if args.len() != 2 {
            return Err("Wasm: Path signature arity mismatch".into());
        }
        let output = args[1];
        let string = if operation >= 2 {
            let shape = &self.mir.types[output.index()];
            if shape.constructor != T::Option || shape.arguments.len() != 1 {
                return Err("Wasm: Path result must be sealed Option(String)".into());
            }
            shape.arguments[0]
        } else {
            output
        };
        let input = &self.mir.types[args[0].index()];
        if self.mir.types[string.index()].constructor != T::String
            || if operation == 0 {
                input.constructor != T::Array || input.arguments != [string]
            } else {
                args[0] != string
            }
        {
            return Err("Wasm: Path signature types mismatch".into());
        }
        let input = self.parameter(0);
        let span = self.local(ValType::I32);
        self.extend([
            I::I32Const(operation as i32),
            I::LocalGet(input),
            I::Call(PATH),
            I::LocalSet(span),
        ]);
        if operation < 2 {
            return self.text_span_value(string, span);
        }
        let result = self.local(ValType::I32);
        self.extend([I::LocalGet(span), I::I32Eqz, I::If(BlockType::Empty)]);
        let none = self.enum_value(node, output, 0, None)?;
        self.extend([I::LocalGet(none), I::LocalSet(result), I::Else]);
        let text = self.text_span_value(string, span)?;
        let some = self.enum_value(node, output, 1, Some(text))?;
        self.extend([I::LocalGet(some), I::LocalSet(result), I::End]);
        Ok(result)
    }
}
