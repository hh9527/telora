//! Closed Value traversal; RT only formats tokens into a per-call buffer.
use crate::{
    abi::*,
    emit::Emitter,
    plan::{Key, Special},
};
use telora_core::mir::TypeId;
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn json_write(&mut self, op: i32, writer: u32, argument: u32) -> u32 {
        let result = self.local(ValType::I32);
        self.extend([
            I::I32Const(op),
            I::LocalGet(writer),
            I::LocalGet(argument),
            I::Call(JSON_WRITE),
            I::LocalSet(result),
        ]);
        result
    }
    pub(crate) fn json_immediate(&mut self, op: i32, writer: u32, argument: i32) {
        self.extend([
            I::I32Const(op),
            I::LocalGet(writer),
            I::I32Const(argument),
            I::Call(JSON_WRITE),
            I::Drop,
        ]);
    }
    fn json_error(&mut self, message: &str, subject: u32) -> Result<(), String> {
        let text = self.text_as(self.key.node, self.string_type()?, message.as_bytes())?;
        let count = self.local(ValType::I32);
        self.extend([I::I32Const(1), I::LocalSet(count)]);
        self.report(self.key.node, text, subject, count, false);
        Ok(())
    }
    pub fn json_native(&mut self, name: &str) -> Result<u32, String> {
        let args = self.mir.types[self.ty(self.key.node)?.index()]
            .arguments
            .clone();
        if name == "stringify_pretty" && self.key.special != Special::Configured {
            let indent = self.parameter(0);
            self.extend([
                I::LocalGet(indent),
                I::I64Load(memory(DATA, 3)),
                I::I64Const(16),
                I::I64GtU,
                I::If(BlockType::Empty),
            ]);
            self.json_error(
                "std/json.stringify_pretty indent must be between 0 and 16",
                indent,
            )?;
            self.emit(I::End);
            return self.function_value(
                self.key.node,
                Key {
                    special: Special::Configured,
                    ..self.key
                },
                args[1],
                &[(indent, args[0])],
            );
        }
        let indent = self.local(ValType::I32);
        if name == "stringify_pretty" {
            self.extend([
                I::LocalGet(0),
                I::I32Load(memory(8, 2)),
                I::I32Load(memory(DATA, 2)),
                I::LocalSet(indent),
            ]);
        } else {
            self.extend([I::I32Const(-1), I::LocalSet(indent)]);
        }
        let zero = self.local(ValType::I32);
        let writer = self.json_write(0, indent, zero);
        let input = self.parameter(0);
        let status = self.json_call_result(args[0], writer, input);
        self.extend([I::LocalGet(status), I::I32Eqz, I::If(BlockType::Empty)]);
        self.json_immediate(11, writer, 0);
        self.emit(I::End);
        self.checked(status);
        let span = self.json_write(1, writer, zero);
        self.text_span_value(args[1], span)
    }
    fn json_call(&mut self, ty: TypeId, writer: u32, value: u32) {
        let result = self.json_call_result(ty, writer, value);
        self.checked(result);
    }
    fn json_call_result(&mut self, ty: TypeId, writer: u32, value: u32) -> u32 {
        let key = Key {
            special: Special::Json(ty),
            callable: true,
            ..self.plan.root
        };
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(writer),
            I::LocalGet(value),
            I::Call(self.plan.functions[&key]),
            I::LocalSet(result),
        ]);
        result
    }
    pub fn json_type(&mut self, ty: TypeId) -> Result<(), String> {
        let variants: Vec<_> = self.plan.layouts[ty.index()]
            .variants
            .iter()
            .map(|branch| branch.name.clone())
            .collect();
        for (index, branch) in variants.iter().enumerate() {
            self.extend([
                I::LocalGet(1),
                I::I32Load(memory(DATA, 2)),
                I::I32Const(index as i32),
                I::I32Eq,
                I::If(BlockType::Empty),
            ]);
            match branch.as_str() {
                "None" => self.json_immediate(5, 0, 0),
                "True" => self.json_immediate(5, 0, 1),
                "False" => self.json_immediate(5, 0, 2),
                "Bytes" => self.json_error("JSON cannot encode Bytes", 1)?,
                "LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime" => {
                    self.json_error("JSON cannot encode temporal values; use a codec first", 1)?
                }
                "String" | "Int" | "Float" => {
                    let payload = self.enum_payload(ty, index as u32, 1)?;
                    let op = match branch.as_str() {
                        "String" => 2,
                        "Int" => 3,
                        _ => 4,
                    };
                    let result = self.json_write(op, 0, payload);
                    if op == 4 {
                        self.extend([I::LocalGet(result), I::I32Eqz, I::If(BlockType::Empty)]);
                        self.json_error("JSON cannot encode a non-finite Float", 1)?;
                        self.emit(I::End);
                    }
                }
                "Array" | "Object" => {
                    let object = branch == "Object";
                    let payload = self.enum_payload(ty, index as u32, 1)?;
                    let base =
                        self.table_data(ARRAYS, payload, if object { DATA + 8 } else { DATA });
                    let keys = if object {
                        Some(self.table_data(ARRAYS, payload, DATA))
                    } else {
                        None
                    };
                    let start = if object {
                        self.local(ValType::I32)
                    } else {
                        self.read32(payload, DATA + 4)
                    };
                    let end = self.read32(payload, if object { DATA + 4 } else { DATA + 8 });
                    let cursor = self.local(ValType::I32);
                    self.extend([I::LocalGet(start), I::LocalSet(cursor)]);
                    self.json_immediate(6, 0, i32::from(object));
                    self.extend([
                        I::Block(BlockType::Empty),
                        I::Loop(BlockType::Empty),
                        I::LocalGet(cursor),
                        I::LocalGet(end),
                        I::I32GeU,
                        I::BrIf(1),
                    ]);
                    let relative = self.local(ValType::I32);
                    self.extend([
                        I::LocalGet(cursor),
                        I::LocalGet(start),
                        I::I32Sub,
                        I::LocalSet(relative),
                    ]);
                    self.json_write(7, 0, relative);
                    if let Some(keys) = keys {
                        let key = self.local(ValType::I32);
                        self.extend([
                            I::LocalGet(keys),
                            I::LocalGet(cursor),
                            I::I32Const(STRING_BYTES as i32),
                            I::I32Mul,
                            I::I32Add,
                            I::LocalSet(key),
                        ]);
                        self.json_write(2, 0, key);
                        self.json_immediate(8, 0, 0);
                    }
                    let value = self.local(ValType::I32);
                    self.extend([
                        I::LocalGet(base),
                        I::LocalGet(cursor),
                        I::I32Const(self.width(ty)? as i32),
                        I::I32Mul,
                        I::I32Add,
                        I::LocalSet(value),
                    ]);
                    self.json_call(ty, 0, value);
                    self.extend([
                        I::LocalGet(cursor),
                        I::I32Const(1),
                        I::I32Add,
                        I::LocalSet(cursor),
                        I::Br(0),
                        I::End,
                        I::End,
                    ]);
                    let close = self.local(ValType::I32);
                    self.extend([
                        I::LocalGet(end),
                        I::LocalGet(start),
                        I::I32Ne,
                        I::I32Const(1),
                        I::I32Shl,
                        I::I32Const(i32::from(object)),
                        I::I32Or,
                        I::LocalSet(close),
                    ]);
                    self.json_write(9, 0, close);
                }
                _ => {
                    return Err(format!(
                        "Wasm: unsupported sealed JSON Value variant {}",
                        branch
                    ));
                }
            }
            self.extend([I::I32Const(1), I::Return, I::End]);
        }
        self.emit(I::Unreachable);
        Ok(())
    }
}
