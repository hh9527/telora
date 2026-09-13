//! Decode named fields into a sealed RecordTable object.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{PropertySite, TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn codec_decode_record(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
        rename: bool,
    ) -> Result<u32, String> {
        let members: Vec<_> = self.plan.layouts[target.index()]
            .object
            .as_ref()
            .ok_or("Wasm: decode record layout missing")?
            .members
            .iter()
            .map(|m| (m.name.clone(), m.type_id))
            .collect();
        let names =
            match crate::codec_names::external_names(members.iter().map(|m| m.0.clone()), rename) {
                Ok(names) => names,
                Err(_) => {
                    self.codec_error(input, "duplicate external member name")?;
                    return Ok(self.local(ValType::I32));
                }
            };
        let index = self.plan.layouts[source.index()]
            .variants
            .iter()
            .position(|v| v.name == "Object")
            .ok_or("Wasm: codec Value lacks Object")?;
        self.extend([
            I::LocalGet(input),
            I::I32Load(memory(DATA, 2)),
            I::I32Const(index as i32),
            I::I32Ne,
            I::If(BlockType::Empty),
        ]);
        self.codec_decode_reject("expected Object", input)?;
        self.emit(I::End);
        let collection = self.enum_payload(source, index as u32, input)?;
        let path = self.read32(0, 24);
        let width = self.width(source)?;
        let keys = self.table_data(ARRAYS, collection, DATA);
        let values = self.table_data(ARRAYS, collection, 24);
        let count = self.read32(collection, 20);
        let mut fields = Vec::with_capacity(members.len());
        for ((_, ty), name) in members.iter().zip(&names) {
            let key = self.text_as(self.key.node, self.string_type()?, name.as_bytes())?;
            let field = self.dictionary_lookup(collection, key, width);
            fields.push((
                self.plan.layouts[ty.ok_or("Wasm: decode field type missing")?].id(),
                key,
                field,
            ));
        }
        // Reject unknown input fields before missing fields or child decoding.
        let cursor = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(cursor),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let key = self.local(ValType::I32);
        let known = self.local(ValType::I32);
        self.extend([
            I::LocalGet(keys),
            I::LocalGet(cursor),
            I::I32Const(32),
            I::I32Mul,
            I::I32Add,
            I::LocalSet(key),
            I::I32Const(0),
            I::LocalSet(known),
        ]);
        for (_, expected, _) in &fields {
            self.extend([
                I::LocalGet(known),
                I::LocalGet(key),
                I::LocalGet(*expected),
                I::Call(STRING_COMPARE),
                I::I32Eqz,
                I::I32Or,
                I::LocalSet(known),
            ]);
        }
        self.extend([I::LocalGet(known), I::I32Eqz, I::If(BlockType::Empty)]);
        let child_path = self.parse_text(7, path, key)?;
        let child = self.local(ValType::I32);
        self.extend([
            I::LocalGet(values),
            I::LocalGet(cursor),
            I::I32Const(width as i32),
            I::I32Mul,
            I::I32Add,
            I::LocalSet(child),
        ]);
        self.codec_decode_reject_at("unknown field", child, child_path)?;
        self.extend([
            I::End,
            I::LocalGet(cursor),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(cursor),
            I::Br(0),
            I::End,
            I::End,
        ]);
        for (ty, key, field) in fields.iter().rev() {
            if self.mir.types[ty.index()].constructor == T::Option {
                continue;
            }
            self.extend([I::LocalGet(*field), I::I32Eqz, I::If(BlockType::Empty)]);
            let child_path = self.parse_text(7, path, *key)?;
            self.codec_decode_reject_at("missing required field", input, child_path)?;
            self.emit(I::End);
        }
        let context = self.alloc(32);
        self.copy(context, 0, 0, 32);
        let mut decoded = Vec::with_capacity(fields.len());
        for (ty, key, field) in fields {
            if self.mir.types[ty.index()].constructor == T::Option {
                self.extend([I::LocalGet(field), I::I32Eqz, I::If(BlockType::Empty)]);
                let absent = self.codec_variant(source, "None", None, input)?;
                self.extend([I::LocalGet(absent), I::LocalSet(field), I::End]);
            }
            let child_path = self.parse_text(7, path, key)?;
            self.extend([
                I::LocalGet(context),
                I::LocalGet(child_path),
                I::I32Store(memory(24, 2)),
            ]);
            let value = self.codec_decode_call(source, ty, field, context)?;
            self.parse_propagate(value);
            decoded.push(value);
        }
        let value = self.packed_tuple(target, &decoded)?;
        self.copy(value, 0, input, 12);
        let error = self.read32(0, 28);
        self.construction_check_with_rejection(
            self.key.node,
            target,
            PropertySite::Type,
            value,
            Some(error),
        )?;
        Ok(value)
    }
}
