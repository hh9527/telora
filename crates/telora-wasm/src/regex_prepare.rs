//! Bind each sealed struct's capture contract before calling fixed Rust RT.
use crate::{abi::*, emit::Emitter};
use telora_core::{
    candidate_layout::State,
    mir::{PropertySite, TypeConstructor as T},
};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub fn regex_prepare(&mut self) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        if args.len() != 4
            || args[0] != args[3]
            || !matches!(self.mir.types[args[0].index()].constructor, T::Native(id) if (id.module,id.slot)==(19,0))
            || args[1..3]
                .iter()
                .any(|ty| self.mir.types[ty.index()].constructor != T::Type)
        {
            return Err("Wasm: regex prepare signature mismatch".into());
        }
        let regex = self.parameter(0);
        let property = self.parameter(1);
        let owner = self.parameter(2);
        let id = self.read32(regex, DATA);
        for layout in &self.plan.layouts {
            if !matches!(
                self.mir.types[layout.type_id].constructor,
                T::Nominal(_) | T::Record(_)
            ) || !matches!(&layout.layout, State::Known {shape} if shape.table == Some("RecordTable"))
            {
                continue;
            }
            let members = &layout
                .object
                .as_ref()
                .ok_or("Wasm: struct contract has no object")?
                .members;
            self.bits(owner);
            self.extend([
                I::I64Const(layout.type_id as i64),
                I::I64Eq,
                I::If(BlockType::Empty),
            ]);
            let packet = self.alloc(4 + members.len() as u32 * 12);
            self.store32(packet, 0, members.len() as u32);
            for (index, member) in members.iter().enumerate() {
                let mut ty = self.plan.layouts
                    [member.type_id.ok_or("Wasm: contract field type missing")?]
                .id();
                let optional = self.mir.types[ty.index()].constructor == T::Option;
                if optional {
                    ty = self.mir.types[ty.index()].arguments[0];
                }
                let offset = 4 + index as u32 * 12;
                let name = self.text_as(node, self.string_type()?, member.name.as_bytes())?;
                self.extend([
                    I::LocalGet(packet),
                    I::LocalGet(name),
                    I::I32Store(memory(offset as u64, 2)),
                ]);
                self.store32(packet, (offset + 4) as u64, u32::from(optional));
                self.emit(I::LocalGet(packet));
                if matches!(
                    self.mir.types[ty.index()].constructor,
                    T::Int | T::Float | T::String
                ) {
                    self.emit(I::I32Const(1));
                } else {
                    self.emit(I::I32Const(0));
                    for &index in self.plan.properties.keys() {
                        let record = &self.mir.properties[index];
                        if record.owner == ty && record.site == PropertySite::Type {
                            self.bits(property);
                            self.extend([
                                I::I64Const(record.property.index() as i64),
                                I::I64Eq,
                                I::I32Or,
                            ]);
                        }
                    }
                }
                self.emit(I::I32Store(memory((offset + 8) as u64, 2)));
            }
            let error = self.local(ValType::I32);
            self.extend([
                I::I32Const(3),
                I::LocalGet(id),
                I::LocalGet(packet),
                I::Call(REGEX),
                I::LocalSet(error),
                I::LocalGet(error),
                I::If(BlockType::Empty),
            ]);
            let message = self.text_span_value(self.string_type()?, error)?;
            let count = self.local(ValType::I32);
            self.extend([I::I32Const(1), I::LocalSet(count)]);
            self.report(node, message, owner, count, false);
            self.extend([I::End, I::LocalGet(regex), I::Return, I::End]);
        }
        self.reflection_failure(owner, "std/regex.parse_by requires a struct type")?;
        Ok(regex)
    }
}
