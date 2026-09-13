use crate::{abi::*, emit::Emitter};
use telora_core::{candidate_layout::State, mir::TypeId};
use wasm_encoder::{Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn codec_encode_record(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
    ) -> Result<u32, String> {
        if self
            .plan
            .properties
            .keys()
            .any(|&index| self.mir.properties[index].owner == source)
        {
            return Err("Wasm: codec property rules are not yet implemented".into());
        }
        let layout = &self.plan.layouts[source.index()];
        if !matches!(&layout.layout, State::Known { shape } if shape.table == Some("RecordTable")) {
            return Err("Wasm: codec nominal type is not yet supported".into());
        }
        let mut members: Vec<_> = layout
            .object
            .as_ref()
            .ok_or("Wasm: codec Record layout missing")?
            .members
            .iter()
            .map(|m| (m.name.clone(), m.type_id, m.offset))
            .collect();
        members.sort_by(|a, b| a.0.cmp(&b.0));
        let width = self.width(target)?;
        let count =
            u32::try_from(members.len()).map_err(|_| "Wasm: codec Record field count overflow")?;
        let keys = self.alloc(
            count
                .checked_mul(32)
                .ok_or("Wasm: codec key size overflow")?,
        );
        let values = self.alloc(
            count
                .checked_mul(width)
                .ok_or("Wasm: codec value size overflow")?,
        );
        let base = self.table_data(RECORDS, input, DATA);
        for (index, (name, ty, offset)) in members.iter().enumerate() {
            let key = self.text_as(self.key.node, self.string_type()?, name.as_bytes())?;
            self.copy(key, 0, input, 12);
            self.copy(keys, index as u32 * 32, key, 32);
            let field = self.local(ValType::I32);
            self.extend([
                I::LocalGet(base),
                I::I32Const(offset.ok_or("Wasm: codec field offset missing")? as i32),
                I::I32Add,
                I::LocalSet(field),
            ]);
            let ty = self.plan.layouts[ty.ok_or("Wasm: codec field type missing")?].id();
            let value = self.codec_encode_scalar(ty, target, field)?;
            self.copy(values, index as u32 * width, value, width);
        }
        let payload_ty = self.plan.layouts[target.index()]
            .variants
            .iter()
            .find(|v| v.name == "Object")
            .and_then(|v| v.type_id)
            .ok_or("Wasm: codec Object payload missing")?;
        let len = self.local(ValType::I32);
        self.extend([I::I32Const(count as i32), I::LocalSet(len)]);
        let payload =
            self.dict_result(self.plan.layouts[payload_ty].id(), keys, values, len, width)?;
        self.copy(payload, 0, input, 12);
        self.codec_variant(target, "Object", Some(payload), input)
    }
}
