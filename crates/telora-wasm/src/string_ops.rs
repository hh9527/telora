use crate::{abi::*, emit::Emitter};
use telora_core::mir::TypeConstructor as T;
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn text_span_array(
        &mut self,
        ty: telora_core::mir::TypeId,
        base: u32,
        count: u32,
    ) -> Result<u32, String> {
        let shape = &self.mir.types[ty.index()];
        if shape.constructor != T::Array
            || shape.arguments.len() != 1
            || self.mir.types[shape.arguments[0].index()].constructor != T::String
        {
            return Err("Wasm: text span list result is not Array(String)".into());
        }
        let string = shape.arguments[0];
        let data = self.array_storage(count, 32);
        let index = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let span = self.array_item(base, index, 8);
        let value = self.text_span_value(string, span)?;
        let destination = self.array_item(data, index, 32);
        self.copy(destination, 0, value, 32);
        self.extend([
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
        ]);
        self.array_result(ty, data, count, 32)
    }
    pub fn string_native(&mut self, name: &str) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        let output = *args.last().ok_or("Wasm: String native signature missing")?;
        if let Some(operation) = ["length", "starts_with", "ends_with", "contains"]
            .iter()
            .position(|&n| n == name)
        {
            let arity = if operation == 0 { 1 } else { 2 };
            if args.len() != arity + 1
                || args[..arity]
                    .iter()
                    .any(|ty| self.mir.types[ty.index()].constructor != T::String)
                || self.mir.types[output.index()].constructor
                    != if operation == 0 { T::Int } else { T::Bool }
            {
                return Err("Wasm: String query signature mismatch".into());
            }
            let a = self.parameter(0);
            let b = if arity == 2 {
                self.parameter(1)
            } else {
                self.local(ValType::I32)
            };
            let result = self.value_as(node, output, SCALAR_BYTES)?;
            self.extend([
                I::LocalGet(result),
                I::I32Const(operation as i32),
                I::LocalGet(a),
                I::LocalGet(b),
                I::Call(TEXT_QUERY),
                I::I64ExtendI32U,
                I::I64Store(memory(DATA, 3)),
            ]);
            return Ok(result);
        }
        if name == "split" || name == "lines" {
            let arity = if name == "split" { 2 } else { 1 };
            if args.len() != arity + 1
                || args[..arity]
                    .iter()
                    .any(|ty| self.mir.types[ty.index()].constructor != T::String)
                || self.mir.types[output.index()].constructor != T::Array
                || self.mir.types[output.index()].arguments != [args[0]]
            {
                return Err("Wasm: String split signature mismatch".into());
            }
            let source = self.parameter(0);
            let separator = if arity == 2 {
                self.parameter(1)
            } else {
                self.local(ValType::I32)
            };
            let spans = self.local(ValType::I32);
            let base = self.local(ValType::I32);
            let count = self.local(ValType::I32);
            self.extend([
                I::I32Const(i32::from(name == "lines")),
                I::LocalGet(source),
                I::LocalGet(separator),
                I::Call(TEXT_SPLIT),
                I::LocalTee(spans),
                I::I32Load(memory(0, 2)),
                I::LocalSet(base),
                I::LocalGet(spans),
                I::I32Load(memory(4, 2)),
                I::LocalSet(count),
            ]);
            return self.text_span_array(output, base, count);
        }
        let operation = [
            "join",
            "join_lines",
            "replace",
            "indent",
            "ensure_trailing_newline",
            "trim_margin",
        ]
        .iter()
        .position(|&n| n == name)
        .ok_or_else(|| format!("Wasm: String native not implemented: {name}"))?;
        let arity = [2, 1, 3, 2, 1, 2][operation];
        if args.len() != arity + 1 || self.mir.types[output.index()].constructor != T::String {
            return Err("Wasm: String construction signature mismatch".into());
        }
        for (index, ty) in args[..arity].iter().enumerate() {
            let shape = &self.mir.types[ty.index()];
            let valid = if operation <= 1 && index == 0 {
                shape.constructor == T::Array && shape.arguments == [output]
            } else if operation == 3 && index == 1 {
                shape.constructor == T::Int
            } else {
                *ty == output
            };
            if !valid {
                return Err("Wasm: String construction argument mismatch".into());
            }
        }
        let a = self.parameter(0);
        let mut b = if arity >= 2 {
            self.parameter(1)
        } else {
            self.local(ValType::I32)
        };
        let c = if arity >= 3 {
            self.parameter(2)
        } else {
            self.local(ValType::I32)
        };
        if operation == 3 || operation == 5 {
            if operation == 3 {
                self.bits(b);
                self.extend([I::I64Const(0), I::I64LtS]);
            } else {
                self.extend([
                    I::I32Const(0),
                    I::LocalGet(b),
                    I::I32Const(0),
                    I::Call(TEXT_QUERY),
                    I::I32Eqz,
                ]);
            }
            self.emit(I::If(BlockType::Empty));
            let message = self.text_as(
                node,
                output,
                if operation == 3 {
                    b"String indentation width must be non-negative"
                } else {
                    b"String margin marker must not be empty"
                },
            )?;
            let one = self.local(ValType::I32);
            self.extend([I::I32Const(1), I::LocalSet(one)]);
            self.report(node, message, b, one, false);
            self.emit(I::End);
            if operation == 3 {
                self.bits(b);
                self.extend([
                    I::I64Const(u32::MAX as i64),
                    I::I64GtU,
                    I::If(BlockType::Empty),
                    I::Unreachable,
                    I::End,
                ]);
                let width = self.local(ValType::I32);
                self.bits(b);
                self.extend([I::I32WrapI64, I::LocalSet(width)]);
                b = width;
            }
        }
        let span = self.local(ValType::I32);
        self.extend([
            I::I32Const(operation as i32),
            I::LocalGet(a),
            I::LocalGet(b),
            I::LocalGet(c),
            I::Call(TEXT_BUILD),
            I::LocalSet(span),
        ]);
        self.text_span_value(output, span)
    }
}
