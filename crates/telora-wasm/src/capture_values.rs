//! Assemble language diagnostics with compile-time field types and offsets.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub fn diagnostic_field_type(&self, ty: TypeId, name: &str) -> Result<TypeId, String> {
        let member = self.plan.layouts[ty.index()]
            .object
            .as_ref()
            .and_then(|layout| layout.members.iter().find(|m| m.name == name))
            .ok_or_else(|| format!("Wasm: diagnostic contract lacks {name}"))?;
        Ok(self.plan.layouts[member
            .type_id
            .ok_or("Wasm: diagnostic field type missing")?]
        .id())
    }
    fn diagnostic_record(&mut self, ty: TypeId, fields: &[(&str, u32)]) -> Result<u32, String> {
        let layout = self.plan.layouts[ty.index()]
            .object
            .as_ref()
            .ok_or("Wasm: diagnostic record layout missing")?;
        let ordered = layout
            .members
            .iter()
            .map(|m| {
                fields
                    .iter()
                    .find(|(name, _)| *name == m.name)
                    .map(|(_, value)| *value)
                    .ok_or_else(|| format!("Wasm: missing diagnostic field {}", m.name))
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.packed_tuple(ty, &ordered)
    }
    fn text_span_value(&mut self, ty: TypeId, span: u32) -> Result<u32, String> {
        if self.mir.types[ty.index()].constructor != T::String {
            return Err("Wasm: diagnostic text field is not String".into());
        }
        let id = self.local(ValType::I32);
        self.extend([
            I::I32Const(table_address(STRINGS) as i32),
            I::LocalGet(span),
            I::I32Load(memory(0, 2)),
            I::LocalGet(span),
            I::I32Load(memory(4, 2)),
            I::Call(TABLE_PUSH),
            I::LocalSet(id),
        ]);
        let value = self.value_as(self.key.node, ty, 32)?;
        self.store32(value, 16, 1);
        self.extend([
            I::LocalGet(value),
            I::LocalGet(id),
            I::I32Store(memory(20, 2)),
            I::LocalGet(value),
            I::LocalGet(span),
            I::I32Load(memory(4, 2)),
            I::I32Store(memory(28, 2)),
        ]);
        Ok(value)
    }
    fn same_origin(&mut self, a: u32, b: u32) {
        for offset in [0, 4, 8] {
            self.extend([
                I::LocalGet(a),
                I::I32Load(memory(offset, 2)),
                I::LocalGet(b),
                I::I32Load(memory(offset, 2)),
                I::I32Eq,
            ]);
            if offset != 0 {
                self.emit(I::I32And);
            }
        }
    }
    fn append_label(
        &mut self,
        label: TypeId,
        origin: u32,
        message: u32,
        primary: bool,
        data: u32,
        count: u32,
    ) -> Result<(), String> {
        self.extend([
            I::LocalGet(origin),
            I::I32Load(memory(0, 2)),
            I::If(BlockType::Empty),
        ]);
        let range = self.diagnostic_field_type(label, "location")?;
        let string = self.diagnostic_field_type(range, "source")?;
        let span = self.local(ValType::I32);
        self.extend([
            I::LocalGet(origin),
            I::I32Load(memory(0, 2)),
            I::Call(SOURCE_NAME),
            I::LocalSet(span),
        ]);
        let source = self.text_span_value(string, span)?;
        let start_ty = self.diagnostic_field_type(range, "start")?;
        let end_ty = self.diagnostic_field_type(range, "end")?;
        if self.mir.types[start_ty.index()].constructor != T::Int
            || self.mir.types[end_ty.index()].constructor != T::Int
        {
            return Err("Wasm: diagnostic range offsets must be Int".into());
        }
        let start = self.scalar_as(self.key.node, start_ty, 0)?;
        let end = self.scalar_as(self.key.node, end_ty, 0)?;
        for (value, offset) in [(start, 4), (end, 8)] {
            self.extend([
                I::LocalGet(value),
                I::LocalGet(origin),
                I::I32Load(memory(offset, 2)),
                I::I64ExtendI32U,
                I::I64Store(memory(DATA, 3)),
            ]);
        }
        let location =
            self.diagnostic_record(range, &[("source", source), ("start", start), ("end", end)])?;
        let bool_ty = self.diagnostic_field_type(label, "primary")?;
        if self.mir.types[bool_ty.index()].constructor != T::Bool {
            return Err("Wasm: label primary must be Bool".into());
        }
        let flag = self.scalar_as(self.key.node, bool_ty, i64::from(primary))?;
        let value = self.diagnostic_record(
            label,
            &[
                ("location", location),
                ("message", message),
                ("primary", flag),
            ],
        )?;
        let width = self.width(label)?;
        let destination = self.array_item(data, count, width);
        self.copy(destination, 0, value, width);
        self.extend([
            I::LocalGet(count),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(count),
            I::End,
        ]);
        Ok(())
    }
    pub fn diagnostic_value(&mut self, ty: TypeId, packet: u32) -> Result<u32, String> {
        let node = self.key.node;
        let string = self.diagnostic_field_type(ty, "message")?;
        let message = self.local(ValType::I32);
        self.extend([
            I::LocalGet(packet),
            I::I32Load(memory(12, 2)),
            I::I32Const(ERROR_USER as i32),
            I::I32Eq,
            I::If(BlockType::Empty),
            I::LocalGet(packet),
            I::I32Load(memory(16, 2)),
            I::LocalSet(message),
            I::Else,
        ]);
        let default = self.text_as(node, string, b"Wasm execution failed")?;
        self.extend([I::LocalGet(default), I::LocalSet(message)]);
        for code in ERROR_OVERFLOW..=ERROR_DATA {
            self.extend([
                I::LocalGet(packet),
                I::I32Load(memory(12, 2)),
                I::I32Const(code as i32),
                I::I32Eq,
                I::If(BlockType::Empty),
            ]);
            let value = self.text_as(
                node,
                string,
                crate::diagnostic_output::error_message(code).as_bytes(),
            )?;
            self.extend([I::LocalGet(value), I::LocalSet(message), I::End]);
        }
        self.emit(I::End);
        let labels_ty = self.diagnostic_field_type(ty, "labels")?;
        let label = self.mir.types[labels_ty.index()].arguments[0];
        if self.diagnostic_field_type(label, "message")? != string {
            return Err("Wasm: diagnostic label message type mismatch".into());
        }
        let total = self.local(ValType::I32);
        self.extend([
            I::LocalGet(packet),
            I::I32Load(memory(24, 2)),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(total),
        ]);
        let labels_data = self.array_storage(total, self.width(label)?);
        let count = self.local(ValType::I32);
        // This code executes once per packet inside the enclosing scope loop.
        self.extend([I::I32Const(0), I::LocalSet(count)]);
        self.append_label(label, packet, message, true, labels_data, count)?;
        let index = self.local(ValType::I32);
        self.extend([
            I::I32Const(0),
            I::LocalSet(index),
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(packet),
            I::I32Load(memory(24, 2)),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let subject = self.local(ValType::I32);
        self.extend([
            I::LocalGet(packet),
            I::I32Load(memory(20, 2)),
            I::LocalGet(index),
            I::I32Const(12),
            I::I32Mul,
            I::I32Add,
            I::LocalSet(subject),
        ]);
        self.same_origin(subject, packet);
        let duplicate = self.local(ValType::I32);
        self.emit(I::LocalSet(duplicate));
        let earlier = self.local(ValType::I32);
        let prior = self.local(ValType::I32);
        self.extend([
            I::I32Const(0),
            I::LocalSet(earlier),
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(earlier),
            I::LocalGet(index),
            I::I32GeU,
            I::BrIf(1),
            I::LocalGet(packet),
            I::I32Load(memory(20, 2)),
            I::LocalGet(earlier),
            I::I32Const(12),
            I::I32Mul,
            I::I32Add,
            I::LocalSet(prior),
        ]);
        self.same_origin(subject, prior);
        self.extend([
            I::LocalGet(duplicate),
            I::I32Or,
            I::LocalSet(duplicate),
            I::LocalGet(earlier),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(earlier),
            I::Br(0),
            I::End,
            I::End,
            I::LocalGet(duplicate),
            I::I32Eqz,
            I::If(BlockType::Empty),
        ]);
        let label_span = self.local(ValType::I32);
        self.extend([
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::Call(SUBJECT_LABEL),
            I::LocalSet(label_span),
        ]);
        let label_message = self.text_span_value(string, label_span)?;
        self.append_label(label, subject, label_message, false, labels_data, count)?;
        self.extend([
            I::End,
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
        ]);
        let labels = self.array_result(labels_ty, labels_data, count, self.width(label)?)?;
        let notes_ty = self.diagnostic_field_type(ty, "notes")?;
        if self.mir.types[notes_ty.index()].constructor != T::Array
            || self.mir.types[notes_ty.index()].arguments != [string]
        {
            return Err("Wasm: diagnostic notes type mismatch".into());
        }
        let zero = self.local(ValType::I32);
        let empty = self.alloc(0);
        let notes = self.array_result(notes_ty, empty, zero, self.width(string)?)?;
        let severity_ty = self.diagnostic_field_type(ty, "severity")?;
        let variants = &self.plan.layouts[severity_ty.index()].variants;
        let error = variants
            .iter()
            .position(|v| v.name == "Error")
            .ok_or("Wasm: Severity.Error missing")?;
        let warning = variants
            .iter()
            .position(|v| v.name == "Warning")
            .ok_or("Wasm: Severity.Warning missing")?;
        let severity = self.value_as(node, severity_ty, self.width(severity_ty)?)?;
        self.extend([
            I::LocalGet(severity),
            I::LocalGet(packet),
            I::I32Load(memory(28, 2)),
            I::If(BlockType::Result(ValType::I64)),
            I::I64Const(warning as i64),
            I::Else,
            I::I64Const(error as i64),
            I::End,
            I::I64Store(memory(DATA, 3)),
        ]);
        self.diagnostic_record(
            ty,
            &[
                ("severity", severity),
                ("message", message),
                ("labels", labels),
                ("notes", notes),
            ],
        )
    }
}
