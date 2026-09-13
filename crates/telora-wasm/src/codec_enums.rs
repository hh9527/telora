use crate::{abi::*, emit::Emitter};
use telora_core::mir::TypeId;
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn codec_encode_enum(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
    ) -> Result<u32, String> {
        self.codec_encode_enum_names(source, target, input, false)
    }

    pub(crate) fn codec_encode_enum_names(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
        rename: bool,
    ) -> Result<u32, String> {
        let variants: Vec<_> = self.plan.layouts[source.index()]
            .variants
            .iter()
            .map(|v| (v.name.clone(), v.type_id))
            .collect();
        let names = match crate::codec_names::external_names(
            variants.iter().map(|v| v.0.clone()),
            rename,
        ) {
            Ok(names) => names,
            Err(_) => {
                let message = self.text_as(
                    self.key.node,
                    self.string_type()?,
                    b"duplicate external variant name",
                )?;
                let count = self.local(ValType::I32);
                self.extend([I::I32Const(1), I::LocalSet(count)]);
                self.report(self.key.node, message, input, count, false);
                return Ok(self.local(ValType::I32));
            }
        };
        let output = self.local(ValType::I32);
        for (index, (_, ty)) in variants.iter().enumerate() {
            let name = &names[index];
            self.extend([
                I::LocalGet(input),
                I::I32Load(memory(DATA, 2)),
                I::I32Const(index as i32),
                I::I32Eq,
                I::If(BlockType::Empty),
            ]);
            let key = self.text_as(self.key.node, self.string_type()?, name.as_bytes())?;
            self.copy(key, 0, input, 12);
            let result = if let Some(ty) = ty {
                let payload = self.enum_payload(source, index as u32, input)?;
                let value =
                    self.codec_encode_scalar(self.plan.layouts[*ty].id(), target, payload)?;
                let object_ty = self.plan.layouts[target.index()]
                    .variants
                    .iter()
                    .find(|v| v.name == "Object")
                    .and_then(|v| v.type_id)
                    .ok_or("Wasm: codec Object payload missing")?;
                let count = self.local(ValType::I32);
                self.extend([I::I32Const(1), I::LocalSet(count)]);
                let dict = self.dict_result(
                    self.plan.layouts[object_ty].id(),
                    key,
                    value,
                    count,
                    self.width(target)?,
                )?;
                self.copy(dict, 0, input, 12);
                self.codec_variant(target, "Object", Some(dict), input)?
            } else {
                self.codec_variant(target, "String", Some(key), input)?
            };
            self.extend([I::LocalGet(result), I::LocalSet(output), I::End]);
        }
        Ok(output)
    }
}
