//! Validate a regex-derived property against its statically known owner.
use crate::{abi::*, emit::Emitter};
use telora_core::{
    candidate_layout::State,
    mir::{Mir, TypeConstructor as T, TypeId, TypeState},
};
use wasm_encoder::{Instruction as I, ValType};

pub(crate) fn property_type(mir: &Mir) -> Option<TypeId> {
    mir.symbols.iter().find_map(|symbol| {
        let module = symbol.module?;
        if symbol.name != "parse_by" || mir.modules[module.index()].native.as_ref()?.id != 19 {
            return None;
        }
        let &node = symbol.declarations.first()?;
        let TypeState::Known(signature) = mir.ty_slots[node.ty().index()] else {
            return None;
        };
        let provider = *mir.types[signature.index()].arguments.last()?;
        let shape = &mir.types[provider.index()];
        (shape.constructor == T::Function).then(|| *shape.arguments.last().unwrap())
    })
}

impl Emitter<'_> {
    pub(crate) fn regex_validate_property(
        &mut self,
        index: usize,
        property: u32,
    ) -> Result<(), String> {
        let record = &self.mir.properties[index];
        let node = record.providers[0];
        let owner = record.owner;
        if !matches!(
            self.mir.types[owner.index()].constructor,
            T::Nominal(_) | T::Record(_)
        ) || !matches!(
            &self.plan.layouts[owner.index()].layout,
            State::Known { shape } if shape.table == Some("RecordTable")
        ) {
            return self.regex_property_error(
                node,
                property,
                "std/regex.parse_by requires a struct type",
            );
        }
        let property_layout = self.plan.layouts[record.property.index()]
            .object
            .as_ref()
            .ok_or("Wasm: regex property layout missing")?;
        let regex_field = property_layout
            .members
            .iter()
            .find(|field| {
                field.type_id.is_some_and(|ty| {
                    matches!(self.mir.types[ty].constructor, T::Native(id) if (id.module,id.slot)==(19,0))
                })
            })
            .ok_or("Wasm: regex property has no Regex field")?;
        let contents = self.table_data(RECORDS, property, DATA);
        let regex = self.local(ValType::I32);
        self.extend([
            I::LocalGet(contents),
            I::I32Const(
                regex_field
                    .offset
                    .ok_or("Wasm: regex field offset missing")? as i32,
            ),
            I::I32Add,
            I::LocalSet(regex),
        ]);
        let regex_id = self.read32(regex, DATA);
        let packet = self.regex_contract_packet(owner)?;
        let error = self.local(ValType::I32);
        self.extend([
            I::I32Const(3),
            I::LocalGet(regex_id),
            I::LocalGet(packet),
            I::Call(REGEX),
            I::LocalSet(error),
            I::LocalGet(error),
            I::If(wasm_encoder::BlockType::Empty),
        ]);
        let message = self.text_span_value(self.string_type()?, error)?;
        let count = self.local(ValType::I32);
        self.extend([I::I32Const(1), I::LocalSet(count)]);
        self.report(node, message, property, count, false);
        self.emit(I::End);
        Ok(())
    }

    fn regex_property_error(
        &mut self,
        node: telora_core::mir::HirId,
        subject: u32,
        message: &str,
    ) -> Result<(), String> {
        let message = self.text_as(node, self.string_type()?, message.as_bytes())?;
        let count = self.local(ValType::I32);
        self.extend([I::I32Const(1), I::LocalSet(count)]);
        self.report(node, message, subject, count, false);
        Ok(())
    }

    pub(crate) fn regex_contract_packet(&mut self, owner: TypeId) -> Result<u32, String> {
        let members = &self.plan.layouts[owner.index()]
            .object
            .as_ref()
            .ok_or("Wasm: struct contract missing")?
            .members;
        let packet = self.alloc(4 + members.len() as u32 * 12);
        self.store32(packet, 0, members.len() as u32);
        for (index, member) in members.iter().enumerate() {
            let mut ty =
                self.plan.layouts[member.type_id.ok_or("Wasm: contract field type missing")?].id();
            let optional = self.mir.types[ty.index()].constructor == T::Option;
            if optional {
                ty = self.mir.types[ty.index()].arguments[0];
            }
            let offset = 4 + index as u64 * 12;
            let name = self.text_as(self.key.node, self.string_type()?, member.name.as_bytes())?;
            self.extend([
                I::LocalGet(packet),
                I::LocalGet(name),
                I::I32Store(memory(offset, 2)),
            ]);
            self.store32(packet, offset + 4, u32::from(optional));
            if !self.plan.parser_evidence.contains_key(&ty) {
                return Err("Wasm: regex field lacks sealed FromStr evidence".into());
            }
            self.store32(packet, offset + 8, 1);
        }
        Ok(packet)
    }
}
