//! Array primitives operate on packed, statically typed elements inside Wasm.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub fn array_parts(&mut self, value: u32, width: u32) -> (u32, u32) {
        let base = self.table_data(ARRAYS, value, DATA);
        let count = self.local(ValType::I32);
        self.extend([
            I::LocalGet(base),
            I::LocalGet(value),
            I::I32Load(memory(20, 2)),
            I::I32Const(width as i32),
            I::I32Mul,
            I::I32Add,
            I::LocalSet(base),
            I::LocalGet(value),
            I::I32Load(memory(24, 2)),
            I::LocalGet(value),
            I::I32Load(memory(20, 2)),
            I::I32Sub,
            I::LocalSet(count),
        ]);
        (base, count)
    }
    pub fn array_storage(&mut self, count: u32, width: u32) -> u32 {
        if width != 0 {
            self.extend([
                I::LocalGet(count),
                I::I32Const((u32::MAX / width) as i32),
                I::I32GtU,
            ]);
            self.fail_if(self.key.node, ERROR_OVERFLOW);
        }
        let data = self.local(ValType::I32);
        self.extend([
            I::LocalGet(count),
            I::I32Const(width as i32),
            I::I32Mul,
            I::Call(ALLOC),
            I::LocalSet(data),
        ]);
        data
    }
    pub fn array_result(
        &mut self,
        ty: TypeId,
        data: u32,
        count: u32,
        width: u32,
    ) -> Result<u32, String> {
        let id = self.local(ValType::I32);
        self.extend([
            I::I32Const(table_address(ARRAYS) as i32),
            I::LocalGet(data),
            I::LocalGet(count),
            I::I32Const(width as i32),
            I::I32Mul,
            I::Call(TABLE_PUSH),
            I::LocalSet(id),
        ]);
        let result = self.value_as(self.key.node, ty, self.width(ty)?)?;
        self.extend([
            I::LocalGet(result),
            I::LocalGet(id),
            I::I32Store(memory(DATA, 2)),
            I::LocalGet(result),
            I::LocalGet(count),
            I::I32Store(memory(24, 2)),
        ]);
        Ok(result)
    }
    pub fn array_item(&mut self, base: u32, index: u32, width: u32) -> u32 {
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(base),
            I::LocalGet(index),
            I::I32Const(width as i32),
            I::I32Mul,
            I::I32Add,
            I::LocalSet(result),
        ]);
        result
    }
    pub fn array_native(&mut self, name: &str) -> Result<u32, String> {
        let node = self.key.node;
        let signature = self.ty(node)?;
        let args = &self.mir.types[signature.index()].arguments;
        if args.len() < 2 || self.mir.types[args[0].index()].constructor != T::Array {
            return Err("Wasm: array native signature mismatch".into());
        }
        let element = self.mir.types[args[0].index()].arguments[0];
        let width = self.width(element)?;
        let output = *args.last().unwrap();
        let value = self.parameter(0);
        let (base, count) = self.array_parts(value, width);
        if name == "length" {
            let result = self.scalar_as(node, output, 0)?;
            self.extend([
                I::LocalGet(result),
                I::LocalGet(count),
                I::I64ExtendI32U,
                I::I64Store(memory(DATA, 3)),
            ]);
            return Ok(result);
        }
        if name == "get" {
            let index = self.parameter(1);
            self.bits(index);
            self.extend([
                I::LocalGet(count),
                I::I64ExtendI32U,
                I::I64GeU,
                I::If(BlockType::Empty),
            ]);
            let none = self.enum_value(node, output, 0, None)?;
            self.extend([I::LocalGet(none), I::Return, I::End]);
            let position = self.local(ValType::I32);
            self.bits(index);
            self.extend([I::I32WrapI64, I::LocalSet(position)]);
            let item = self.array_item(base, position, width);
            return self.enum_value(node, output, 1, Some(item));
        }
        if matches!(name, "push" | "enumerate" | "zip" | "concat" | "flat_map") {
            return self.array_build(name, args, base, count, width);
        }
        if !matches!(
            name,
            "map" | "filter" | "fold" | "fold_control" | "any" | "all" | "find"
        ) {
            return Err(format!("Wasm: array operation not implemented: {name}"));
        }
        let folding = matches!(name, "fold" | "fold_control");
        let callback_index = if folding { 2 } else { 1 };
        let callback_type = &self.mir.types[args[callback_index].index()];
        if callback_type.constructor != T::Function {
            return Err("Wasm: array callback has no sealed function type".into());
        }
        let callback = self.parameter(callback_index as u32);
        let index = self.local(ValType::I32);
        let used = self.local(ValType::I32);
        let packed = matches!(name, "map" | "filter");
        let output_width = if name == "map" {
            self.width(self.mir.types[output.index()].arguments[0])?
        } else {
            width
        };
        let data = packed.then(|| self.array_storage(count, output_width));
        let accumulator = if folding {
            Some(self.parameter(1))
        } else {
            None
        };
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let item = self.array_item(base, index, width);
        let result = self.invoke(
            callback,
            &if let Some(accumulator) = accumulator {
                vec![accumulator, item]
            } else {
                vec![item]
            },
        )?;
        match name {
            "map" | "filter" => {
                if name == "filter" {
                    self.bits(result);
                    self.extend([I::I64Eqz, I::I32Eqz, I::If(BlockType::Empty)]);
                }
                let destination = self.array_item(data.unwrap(), used, output_width);
                self.copy(
                    destination,
                    0,
                    if name == "map" { result } else { item },
                    output_width,
                );
                self.extend([
                    I::LocalGet(used),
                    I::I32Const(1),
                    I::I32Add,
                    I::LocalSet(used),
                ]);
                if name == "filter" {
                    self.emit(I::End);
                }
            }
            "fold" => self.extend([I::LocalGet(result), I::LocalSet(accumulator.unwrap())]),
            "fold_control" => {
                self.extend([
                    I::LocalGet(result),
                    I::I32Load(memory(DATA, 2)),
                    I::I32Eqz,
                    I::If(BlockType::Empty),
                    I::LocalGet(result),
                    I::Return,
                    I::End,
                ]);
                let state = self.enum_payload(output, 1, result)?;
                self.extend([I::LocalGet(state), I::LocalSet(accumulator.unwrap())]);
            }
            "any" | "all" | "find" => {
                self.bits(result);
                self.emit(I::I64Eqz);
                if name != "all" {
                    self.emit(I::I32Eqz);
                }
                self.emit(I::If(BlockType::Empty));
                let result = if name == "find" {
                    self.enum_value(node, output, 1, Some(item))?
                } else {
                    self.scalar_as(node, output, i64::from(name == "any"))?
                };
                self.extend([I::LocalGet(result), I::Return, I::End]);
            }
            _ => unreachable!(),
        }
        self.extend([
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
        ]);
        match name {
            "map" | "filter" => self.array_result(output, data.unwrap(), used, output_width),
            "fold" => Ok(accumulator.unwrap()),
            "fold_control" => self.enum_value(node, output, 1, accumulator),
            "find" => self.enum_value(node, output, 0, None),
            "any" | "all" => self.scalar_as(node, output, i64::from(name == "all")),
            _ => unreachable!(),
        }
    }
}
