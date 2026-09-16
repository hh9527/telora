use crate::{abi::*, emit::Emitter};
use telora_core::mir::{PropertySite, TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn codec_display(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
    ) -> Result<u32, String> {
        for (&index, &key) in &self.plan.properties {
            let property = &self.mir.properties[index];
            if property.owner != source || property.site != PropertySite::Type {
                continue;
            }
            let Some(object) = &self.plan.layouts[property.property.index()].object else {
                continue;
            };
            let Some(field) = object.members.iter().find(|m| m.name == "display") else {
                continue;
            };
            let signature = &self.mir.types[field
                .type_id
                .ok_or("Wasm: DisplayBy function type missing")?];
            if signature.constructor != T::Function || signature.arguments.len() != 2 {
                continue;
            }
            let args = signature.arguments.clone();
            if self.mir.types[args[0].index()].constructor != T::Dyn
                || !matches!(self.mir.types[args[1].index()].constructor, T::Native(id) if (id.module,id.slot) == (20,1))
            {
                continue;
            }
            let offset = field
                .offset
                .ok_or("Wasm: DisplayBy function offset missing")?;
            self.extend([
                I::LocalGet(0),
                I::I32Load(memory(12, 2)),
                I::I32Const(property.property.index() as i32),
                I::I32Eq,
                I::If(BlockType::Empty),
            ]);
            let capability = self.call_key(key)?;
            let data = self.table_data(RECORDS, capability, DATA);
            let callback = self.local(ValType::I32);
            self.extend([
                I::LocalGet(data),
                I::I32Const(offset as i32),
                I::I32Add,
                I::LocalSet(callback),
            ]);
            let concrete = self.local(ValType::I32);
            let width = self.local(ValType::I32);
            self.extend([
                I::I32Const(source.index() as i32),
                I::LocalSet(concrete),
                I::I32Const(self.width(source)? as i32),
                I::LocalSet(width),
            ]);
            let boxed = self.box_dynamic_pointer(args[0], input, concrete, width)?;
            let formatted = self.invoke(callback, &[boxed])?;
            let span = self.local(ValType::I32);
            self.extend([
                I::LocalGet(formatted),
                I::Call(FORMAT_RENDER),
                I::LocalSet(span),
            ]);
            let text = self.text_span_value(self.string_type()?, span)?;
            self.copy(text, 0, input, LOC_BYTES);
            let value = self.codec_variant(target, "String", Some(text), input)?;
            self.extend([I::LocalGet(value), I::Return, I::End]);
        }
        self.codec_error(input, "text codec requires a DisplayBy property")?;
        Ok(self.local(ValType::I32))
    }
}
