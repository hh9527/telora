use crate::{abi::*, emit::Emitter};
use telora_core::mir::{TypeConstructor as T, TypeId};
use wasm_encoder::{Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn codec_display(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
    ) -> Result<u32, String> {
        let evidence = &self.mir.evidence[*self
            .plan
            .display_evidence
            .get(&source)
            .ok_or("Wasm: text codec has no sealed Display evidence")?];
        let implementation = evidence
            .implementation
            .ok_or("Wasm: Display evidence has no implementation")?;
        let key = if let Some(instance) = evidence.instance {
            self.plan.instances.get(&instance)
        } else {
            self.plan.globals.get(&implementation)
        }
        .copied()
        .ok_or("Wasm: Display implementation is not in the sealed executable")?;
        let owner = key.ty(self.mir, key.node)?;
        let field = self.plan.layouts[owner.index()]
            .object
            .as_ref()
            .and_then(|object| object.members.iter().find(|field| field.name == "fmt"))
            .ok_or("Wasm: Display implementation lacks fmt")?;
        let signature = self.plan.layouts[field
            .type_id
            .ok_or("Wasm: Display member has no sealed signature")?]
        .id();
        let shape = &self.mir.types[signature.index()];
        if shape.constructor != T::Function
            || shape.arguments.len() != 3
            || shape.arguments[0] != source
            || !matches!(self.mir.types[shape.arguments[1].index()].constructor, T::Native(id) if (id.module, id.slot) == (20, 1))
        {
            return Err("Wasm: Display member signature mismatch".into());
        }
        let fmt_type = shape.arguments[1];
        if shape.arguments[2] != fmt_type {
            return Err("Wasm: Display result must be Fmt".into());
        }
        let record = self.call_key(key)?;
        let data = self.table_data(RECORDS, record, DATA);
        let callback = self.local(ValType::I32);
        self.extend([
            I::LocalGet(data),
            I::I32Const(
                field
                    .offset
                    .ok_or("Wasm: Display member has no layout offset")? as i32,
            ),
            I::I32Add,
            I::LocalSet(callback),
        ]);
        let empty = self.format_empty(self.key.node, fmt_type)?;
        let formatted = self.invoke(callback, &[input, empty], fmt_type)?;
        let span = self.local(ValType::I32);
        self.extend([
            I::LocalGet(formatted),
            I::Call(FORMAT_RENDER),
            I::LocalSet(span),
        ]);
        let text = self.format_result(self.key.node, self.string_type()?, span, formatted)?;
        self.copy(text, 0, input, LOC_BYTES);
        self.codec_variant(target, "String", Some(text), input)
    }
}
