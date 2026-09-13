//! Per-call definition slots close recursive nominal schema references.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    fn schema_label(&mut self, prefix: &str, index: u32) -> Result<u32, String> {
        let prefix = self.text_as(self.key.node, self.string_type()?, prefix.as_bytes())?;
        self.parse_text(10, prefix, index)
    }
    fn schema_ref(&mut self, target: TypeId, index: u32) -> Result<u32, String> {
        let text = self.schema_label("#/$defs/Type", index)?;
        self.copy(text, 0, 1, 12);
        let value = self.codec_variant(target, "String", Some(text), 1)?;
        self.schema_object(target, vec![("$ref".into(), value)], 1)
    }
    pub(crate) fn schema_definitions(
        &mut self,
        target: TypeId,
        result: u32,
        context: u32,
        input: u32,
    ) -> Result<(), String> {
        let count = self.read32(context, 32);
        self.extend([I::LocalGet(count), I::If(BlockType::Empty)]);
        let definitions = self.read32(context, 28);
        let object = self.schema_object(target, vec![], input)?;
        let index = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let address = self.array_item(definitions, index, 4);
        let definition = self.read32(address, 0);
        let key = self.schema_label("Type", index)?;
        let next = self.schema_add_key(target, object, key, definition, input)?;
        self.extend([
            I::LocalGet(next),
            I::LocalSet(object),
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
        ]);
        let root = self.schema_add_field(target, result, "$defs", object, input)?;
        self.extend([I::LocalGet(root), I::LocalSet(result), I::End]);
        Ok(())
    }
    pub(crate) fn schema_nominal(&mut self, source: TypeId, target: TypeId) -> Result<u32, String> {
        let decode = self.codec_property_present(source, 1);
        let encode = self.codec_property_present(source, 2);
        self.extend([
            I::LocalGet(decode),
            I::LocalGet(encode),
            I::I32Ne,
            I::If(BlockType::Empty),
        ]);
        self.codec_error(
            1,
            "std/string.decode_by_parse and std/string.encode_by_display must be used together",
        )?;
        self.emit(I::End);
        self.extend([I::LocalGet(encode), I::If(BlockType::Empty)]);
        self.codec_property_value(source, 1)?;
        self.codec_property_value(source, 2)?;
        let text = self.schema_kind(target, "string")?;
        self.extend([I::LocalGet(text), I::Return, I::End]);
        let rename = self.codec_rename(source, 1)?;
        let untagged = self.codec_property_value(source, 5)?;
        let links = self.read32(0, 24);
        let previous = self.read32(links, source.index() as u64 * 4);
        self.extend([
            I::LocalGet(previous),
            I::If(BlockType::Empty),
            I::LocalGet(previous),
            I::I32Const(1),
            I::I32Sub,
            I::LocalSet(previous),
        ]);
        let reference = self.schema_ref(target, previous)?;
        self.extend([I::LocalGet(reference), I::Return, I::End]);
        let index = self.read32(0, 32);
        self.extend([
            I::LocalGet(links),
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::I32Store(memory(source.index() as u64 * 4, 2)),
            I::LocalGet(0),
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::I32Store(memory(32, 2)),
        ]);
        let Some(layout) = &self.mir.type_layouts[source.index()] else {
            self.codec_error(1, "JSON Schema requires a concrete type")?;
            return Ok(self.local(ValType::I32));
        };
        let body = layout.body;
        let shape = &self.mir.types[body.index()];
        let value = if let T::Record(names) = &shape.constructor {
            let names = names.clone();
            let types = shape.arguments.clone();
            let value = self.local(ValType::I32);
            self.extend([I::LocalGet(rename), I::If(BlockType::Empty)]);
            let body = self.schema_record(target, names.clone(), types.clone(), true)?;
            self.extend([I::LocalGet(body), I::LocalSet(value), I::Else]);
            let body = self.schema_record(target, names, types, false)?;
            self.extend([I::LocalGet(body), I::LocalSet(value), I::End]);
            value
        } else if shape.constructor == T::Newtype {
            self.schema_call(shape.arguments[0], target, 0, 1)?
        } else if matches!(shape.constructor, T::Enum(_)) {
            let value = self.local(ValType::I32);
            self.extend([
                I::LocalGet(untagged),
                I::If(BlockType::Empty),
                I::LocalGet(rename),
                I::If(BlockType::Empty),
            ]);
            self.codec_error(1, "$: rename_all is not meaningful on an untagged Enum")?;
            self.emit(I::End);
            let value_body = self.schema_enum(body, target, false, true)?;
            self.extend([
                I::LocalGet(value_body),
                I::LocalSet(value),
                I::Else,
                I::LocalGet(rename),
                I::If(BlockType::Empty),
            ]);
            let value_body = self.schema_enum(body, target, true, false)?;
            self.extend([I::LocalGet(value_body), I::LocalSet(value), I::Else]);
            let value_body = self.schema_enum(body, target, false, false)?;
            self.extend([I::LocalGet(value_body), I::LocalSet(value), I::End, I::End]);
            value
        } else {
            self.schema_call(body, target, 0, 1)?
        };
        let definitions = self.read32(0, 28);
        let address = self.array_item(definitions, index, 4);
        self.extend([
            I::LocalGet(address),
            I::LocalGet(value),
            I::I32Store(memory(0, 2)),
        ]);
        self.schema_ref(target, index)
    }
    pub(crate) fn schema_record(
        &mut self,
        target: TypeId,
        names: Vec<String>,
        types: Vec<TypeId>,
        rename: bool,
    ) -> Result<u32, String> {
        let mut seen = std::collections::BTreeSet::new();
        let mut external = Vec::new();
        for name in names {
            let name = crate::codec_names::external_names([name], rename)?.remove(0);
            if !seen.insert(name.clone()) {
                self.codec_error(1, &format!("$.{name}: duplicate external field name"))?;
                return Ok(self.local(ValType::I32));
            }
            external.push(name);
        }
        let names = external;
        let kind = self.schema_string(target, b"object", 1)?;
        let mut fields = Vec::new();
        let mut required = Vec::new();
        for (name, ty) in names.into_iter().zip(types) {
            let child = self.schema_call(ty, target, 0, 1)?;
            if self.mir.types[ty.index()].constructor != T::Option {
                required.push(self.schema_string(target, name.as_bytes(), 1)?);
            }
            fields.push((name, child));
        }
        let properties = self.schema_object(target, fields, 1)?;
        let no = self.codec_variant(target, "False", None, 1)?;
        let mut fields = vec![
            ("type".into(), kind),
            ("properties".into(), properties),
            ("additionalProperties".into(), no),
        ];
        if !required.is_empty() {
            fields.push(("required".into(), self.schema_array(target, &required, 1)?));
        }
        self.schema_object(target, fields, 1)
    }
}
