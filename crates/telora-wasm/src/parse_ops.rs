//! Context: property metadata, path String, shared error cell, depth, original input,
//! optional codec rejection cell (zero for ordinary string.parse).
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};
impl Emitter<'_> {
    pub fn parse_native(&mut self) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        if args.len() != 3
            || self.mir.types[args[0].index()].constructor != T::TypeOf
            || self.mir.types[args[0].index()].arguments.len() != 1
            || self.mir.types[args[1].index()].constructor != T::String
            || self.mir.types[args[2].index()].constructor != T::Result
        {
            return Err("Wasm: parse_with signature mismatch".into());
        }
        let target = self.mir.types[args[0].index()].arguments[0];
        if self.mir.types[args[2].index()].arguments != [target, args[1]] {
            return Err("Wasm: parse result mismatch".into());
        }
        let input = self.parameter(1);
        let path = self.text_as(node, args[1], b"$")?;
        let context = self.alloc(24);
        let error = self.alloc(4);
        self.store32(error, 0, 0);
        for (offset, value) in [(4, path), (8, error), (16, input)] {
            self.extend([
                I::LocalGet(context),
                I::LocalGet(value),
                I::I32Store(memory(offset, 2)),
            ]);
        }
        self.store32(context, 12, 0);
        self.store32(context, 20, 0);
        let value = self.parse_call(target, context, input)?;
        self.extend([I::LocalGet(value), I::I32Eqz, I::If(BlockType::Empty)]);
        let message = self.read32(error, 0);
        self.extend([
            I::LocalGet(message),
            I::I32Eqz,
            I::If(BlockType::Empty),
            I::I32Const(0),
            I::Return,
            I::End,
        ]);
        let rejected = self.enum_value(node, args[2], 0, Some(message))?;
        self.extend([I::LocalGet(rejected), I::Return, I::End]);
        self.enum_value(node, args[2], 1, Some(value))
    }
    pub(crate) fn parse_call(
        &mut self,
        ty: TypeId,
        context: u32,
        input: u32,
    ) -> Result<u32, String> {
        let key = self
            .plan
            .parsers
            .get(&ty)
            .ok_or("Wasm: parser was not planned")?;
        let value = self.local(ValType::I32);
        self.extend([
            I::LocalGet(context),
            I::LocalGet(input),
            I::Call(self.plan.functions[key]),
            I::LocalSet(value),
        ]);
        Ok(value)
    }
    pub(crate) fn parse_reject(&mut self, message: &str) -> Result<(), String> {
        let message = self.text_as(self.key.node, self.string_type()?, message.as_bytes())?;
        self.parse_reject_value(message)
    }
    pub(crate) fn parse_reject_value(&mut self, message: u32) -> Result<(), String> {
        let path = self.read32(0, 4);
        let message = self.parse_text(6, path, message)?;
        let original = self.read32(0, 16);
        self.copy(message, 0, original, LOC_BYTES);
        let error = self.read32(0, 8);
        self.extend([
            I::LocalGet(error),
            I::LocalGet(message),
            I::I32Store(memory(0, 2)),
            I::I32Const(0),
            I::Return,
        ]);
        Ok(())
    }
    pub(crate) fn parse_text(&mut self, operation: i32, a: u32, b: u32) -> Result<u32, String> {
        let span = self.local(ValType::I32);
        self.extend([
            I::I32Const(operation),
            I::LocalGet(a),
            I::LocalGet(b),
            I::I32Const(0),
            I::Call(TEXT_BUILD),
            I::LocalSet(span),
        ]);
        self.text_span_value(self.string_type()?, span)
    }
    pub(crate) fn parse_propagate(&mut self, value: u32) {
        self.extend([
            I::LocalGet(value),
            I::I32Eqz,
            I::If(BlockType::Empty),
            I::I32Const(0),
            I::Return,
            I::End,
        ]);
    }
    pub(crate) fn parse_context(&mut self, path: u32) -> u32 {
        let context = self.alloc(24);
        self.copy(context, 0, 0, 24);
        let depth = self.read32(0, 12);
        self.extend([
            I::LocalGet(context),
            I::LocalGet(path),
            I::I32Store(memory(4, 2)),
            I::LocalGet(context),
            I::LocalGet(depth),
            I::I32Const(1),
            I::I32Add,
            I::I32Store(memory(12, 2)),
        ]);
        context
    }
    pub fn parse_type(&mut self, target: TypeId) -> Result<u32, String> {
        let node = self.key.node;
        let depth = self.read32(0, 12);
        self.extend([
            I::LocalGet(depth),
            I::I32Const(512),
            I::I32GtU,
            I::If(BlockType::Empty),
        ]);
        self.parse_reject("string parse nesting limit")?;
        self.emit(I::End);
        if self.mir.types[target.index()].constructor == T::Option {
            self.extend([I::LocalGet(1), I::I32Eqz, I::If(BlockType::Empty)]);
            let none = self.enum_value(node, target, 0, None)?;
            let original = self.read32(0, 16);
            self.copy(none, 0, original, LOC_BYTES);
            self.extend([I::LocalGet(none), I::Return, I::End]);
            let path = self.read32(0, 4);
            let context = self.parse_context(path);
            let inner = self.mir.types[target.index()].arguments[0];
            let value = self.parse_call(inner, context, 1)?;
            self.parse_propagate(value);
            let some = self.enum_value(node, target, 1, Some(value))?;
            self.copy(some, 0, 1, LOC_BYTES);
            return Ok(some);
        }
        self.extend([I::LocalGet(1), I::I32Eqz, I::If(BlockType::Empty)]);
        self.parse_reject("required capture is absent")?;
        self.emit(I::End);
        match self.mir.types[target.index()].constructor {
            T::String => Ok(1),
            T::Int | T::Float => {
                let integer = self.mir.types[target.index()].constructor == T::Int;
                let value = self.value_as(node, target, SCALAR_BYTES)?;
                self.copy(value, 0, 1, LOC_BYTES);
                self.extend([
                    I::I32Const(if integer { 4 } else { 5 }),
                    I::LocalGet(1),
                    I::LocalGet(value),
                    I::I32Const(DATA as i32),
                    I::I32Add,
                    I::Call(TEXT_QUERY),
                    I::I32Eqz,
                    I::If(BlockType::Empty),
                ]);
                self.parse_reject(if integer {
                    "input is not a valid Int"
                } else {
                    "input is not a finite Float"
                })?;
                self.emit(I::End);
                Ok(value)
            }
            _ => self.parse_record(target),
        }
    }
}
