//! Mechanical JSON traversal of the sealed diagnostic schema, without Value trees.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    fn reply_text(&mut self, writer: u32, text: &str) -> Result<(), String> {
        let value = self.text_as(self.key.node, self.string_type()?, text.as_bytes())?;
        self.json_write(10, writer, value);
        Ok(())
    }

    pub fn service_reply_native(&mut self) -> Result<u32, String> {
        let args = self.mir.types[self.ty(self.key.node)?.index()].arguments.clone();
        if args.len() != 4 || self.mir.types[args[0].index()].constructor != T::String
            || self.mir.types[args[1].index()].constructor != T::Bool
            || self.mir.types[args[2].index()].constructor != T::Array || args[3] != args[0] {
            return Err("Wasm: invalid service reply signature".into());
        }
        let indent = self.local(ValType::I32);
        self.extend([I::I32Const(-1), I::LocalSet(indent)]);
        let zero = self.local(ValType::I32);
        let writer = self.json_write(0, indent, zero);
        self.reply_text(writer, "{\"schema\":\"telora.service/v1\",\"ok\":")?;
        let payload = self.parameter(0);
        self.json_write(10, writer, payload);
        self.reply_text(writer, ",\"error\":")?;
        let error = self.parameter(1);
        self.reply_value(args[1], error, writer)?;
        self.reply_text(writer, ",\"diagnostics\":")?;
        let diagnostics = self.parameter(2);
        self.reply_value(args[2], diagnostics, writer)?;
        self.reply_text(writer, "}")?;
        let span = self.json_write(1, writer, zero);
        self.text_span_value(args[3], span)
    }

    fn reply_value(&mut self, ty: TypeId, value: u32, writer: u32) -> Result<(), String> {
        match self.mir.types[ty.index()].constructor {
            T::String => { self.json_write(2, writer, value); }
            T::Int => { self.json_write(3, writer, value); }
            T::Bool => {
                let bit = self.read32(value, DATA);
                self.extend([I::LocalGet(bit), I::If(BlockType::Empty)]);
                self.json_immediate(5, writer, 1);
                self.emit(I::Else);
                self.json_immediate(5, writer, 2);
                self.emit(I::End);
            }
            T::Enum(_) | T::Nominal(_) if !self.plan.layouts[ty.index()].variants.is_empty() => {
                let variants = self.plan.layouts[ty.index()].variants.iter()
                    .map(|v| (v.name.clone(), v.type_id)).collect::<Vec<_>>();
                for (index, (name, payload)) in variants.iter().enumerate() {
                    if payload.is_some() { return Err("Wasm: diagnostic enum has payload".into()); }
                    let tag = self.read32(value, DATA);
                    self.extend([I::LocalGet(tag), I::I32Const(index as i32), I::I32Eq, I::If(BlockType::Empty)]);
                    let name = self.text_as(self.key.node, self.string_type()?, name.as_bytes())?;
                    self.json_write(2, writer, name);
                    self.emit(I::End);
                }
            }
            T::Record(_) | T::Nominal(_) => {
                let members = self.plan.layouts[ty.index()].object.as_ref()
                    .ok_or("Wasm: diagnostic record has no layout")?.members.iter()
                    .map(|m| (m.name.clone(), m.offset, m.type_id)).collect::<Vec<_>>();
                let base = self.table_data(RECORDS, value, DATA);
                self.json_immediate(6, writer, 1);
                for (index, (name, offset, field_ty)) in members.iter().enumerate() {
                    self.json_immediate(7, writer, index as i32);
                    let key = self.text_as(self.key.node, self.string_type()?, name.as_bytes())?;
                    self.json_write(2, writer, key);
                    self.json_immediate(8, writer, 0);
                    let field = self.local(ValType::I32);
                    self.extend([I::LocalGet(base), I::I32Const(offset.ok_or("Wasm: missing diagnostic field offset")? as i32), I::I32Add, I::LocalSet(field)]);
                    let field_ty = self.plan.layouts[field_ty.ok_or("Wasm: missing diagnostic field type")?].id();
                    self.reply_value(field_ty, field, writer)?;
                }
                self.json_immediate(9, writer, if members.is_empty() { 1 } else { 3 });
            }
            T::Array => {
                let element = self.mir.types[ty.index()].arguments[0];
                let width = self.width(element)?;
                let (base, count) = self.array_parts(value, width);
                let index = self.local(ValType::I32);
                self.extend([I::I32Const(0), I::LocalSet(index)]);
                self.json_immediate(6, writer, 0);
                self.extend([I::Block(BlockType::Empty), I::Loop(BlockType::Empty),
                    I::LocalGet(index), I::LocalGet(count), I::I32GeU, I::BrIf(1)]);
                self.json_write(7, writer, index);
                let item = self.array_item(base, index, width);
                self.reply_value(element, item, writer)?;
                self.extend([I::LocalGet(index), I::I32Const(1), I::I32Add, I::LocalSet(index),
                    I::Br(0), I::End, I::End]);
                let flags = self.local(ValType::I32);
                self.extend([I::LocalGet(count), I::I32Const(0), I::I32Ne,
                    I::I32Const(1), I::I32Shl, I::LocalSet(flags)]);
                self.json_write(9, writer, flags);
            }
            ref other => return Err(format!("Wasm: unsupported service diagnostic field {ty:?}: {other:?}")),
        }
        Ok(())
    }
}
