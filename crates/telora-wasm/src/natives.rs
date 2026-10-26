use crate::{
    abi::*,
    emit::Emitter,
    plan::{Key, Special},
};
use telora_core::mir::{HirId, HirKind, Mir, TypeConstructor as T};
use wasm_encoder::{BlockType, Instruction as I, ValType};

/// Native ABI dispatch requires a declaration in an admitted numeric module.
/// Aliases retain the resolved declaration identity; user names never activate it.
pub(crate) fn identity(mir: &Mir, node: HirId) -> Option<(u32, &str)> {
    if !matches!(
        mir.hir[node.index()].kind,
        HirKind::Binding {
            kind: telora_core::syntax::kinds::BindingKind::Native,
            ..
        }
    ) {
        return None;
    }
    let symbol = &mir.symbols[mir.hir_symbols[node.index()]?.index()];
    Some((
        mir.modules[symbol.module?.index()].native.as_ref()?.id,
        &symbol.name,
    ))
}

impl Emitter<'_> {
    pub fn native(&mut self) -> Result<u32, String> {
        match identity(self.mir, self.key.node) {
            Some((1, "equal")) => self.equal_native(),
            Some((2, name)) => self.dynamic_native(name),
            Some((3, name)) => self.reflect_native(name),
            Some((5, name)) => self.array_native(name),
            Some((36, "compute")) => self.sequence_compute(),
            Some((6, name)) => self.dict_native(name),
            Some((7, name)) => self.string_native(name),
            Some((27, "parse_with")) => self.parse_native(),
            Some((8, name)) => self.path_native(name),
            Some((35, name)) => self.uri_native(name),
            Some((19, name)) => self.regex_native(name),
            Some((16, name)) => self.hash_native(name),
            Some((13, "encode_with")) => self.codec_encode_native(),
            Some((13, "decode_with")) => self.codec_decode_native(),
            Some((17, name @ ("stringify" | "stringify_pretty"))) => self.json_native(name),
            Some((17, "parse_raw")) => self.data_parse_native(JSON_PARSE),
            Some((9, "parse_raw")) => self.data_parse_native(TOML_PARSE),
            Some((24, "parse_raw")) => self.data_parse_native(YAML_PARSE),
            Some((20, name)) => self.format_native(name),
            Some((26, "call_with_diagnostics")) => self.capture_diagnostics(),
            Some((26, "build_time_active")) => self.build_time_active_native(),
            Some((26, "publish_service_metadata")) => self.information_native(SERVICE_METADATA),
            Some((26, "publish_build_information")) => self.information_native(BUILD_INFORMATION),
            Some((26, "service_reply")) => self.service_reply_native(),
            Some((34, name @ ("message" | "with_message"))) => self.blame_native(name),
            Some((33, name)) => self.test_native(name),
            Some((18, "property")) => self.property_factory(),
            Some((
                25,
                name @ ("get_type_prop" | "get_field_prop" | "get_variant_prop" | "evidence"
                | "required" | "optional"),
            )) => self.property_query(name),
            other => Err(format!("Wasm: native ABI not implemented: {other:?}")),
        }
    }
    fn build_time_active_native(&mut self) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        let result = self.value_as(
            node,
            *args.last().ok_or("invalid build scope signature")?,
            SCALAR_BYTES,
        )?;
        self.extend([
            I::LocalGet(result),
            I::Call(BUILD_TIME_ACTIVE),
            I::I64ExtendI32U,
            I::I64Store(memory(DATA, 3)),
        ]);
        Ok(result)
    }

    fn information_native(&mut self, import: u32) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        if args.len() != 3
            || args[..2]
                .iter()
                .any(|ty| self.mir.types[ty.index()].constructor != T::String)
            || self.mir.types[args[2].index()].constructor != T::Bool
        {
            return Err("Wasm: invalid Service metadata signature".into());
        }
        let name = self.parameter(0);
        let value = self.parameter(1);
        let result = self.value_as(node, args[2], SCALAR_BYTES)?;
        self.extend([
            I::LocalGet(result),
            I::LocalGet(name),
            I::LocalGet(value),
            I::Call(import),
            I::I64ExtendI32U,
            I::I64Store(memory(DATA, 3)),
        ]);
        Ok(result)
    }

    fn property_factory(&mut self) -> Result<u32, String> {
        let node = self.key.node;
        let signature = self.ty(node)?;
        let args = &self.mir.types[signature.index()].arguments;
        let output = *args.last().ok_or("Wasm: property factory has no result")?;
        if self.key.special != Special::Configured {
            if args.len() != 2 || self.mir.types[args[0].index()].constructor != T::PropertyTarget {
                return Err("Wasm: property factory ABI mismatch".into());
            }
            let target = self.parameter(0);
            return self.function_value(
                node,
                Key {
                    special: Special::Configured,
                    ..self.key
                },
                output,
                &[(target, args[0])],
            );
        }
        if args.len() != 3
            || self.mir.types[args[0].index()].constructor != T::Type
            || self.mir.types[args[1].index()].constructor != T::Option
            || self.mir.types[args[1].index()].arguments != [output]
        {
            return Err("Wasm: property provider ABI mismatch".into());
        }
        let members = &self.plan.layouts[output.index()]
            .object
            .as_ref()
            .ok_or("Wasm: property marker has no layout")?
            .members;
        if members.len() != 1 || members[0].name != "bits" {
            return Err("Wasm: property marker fields differ from ABI".into());
        }
        let integer =
            self.plan.layouts[members[0].type_id.ok_or("Wasm: marker bits have no type")?].id();
        if self.mir.types[integer.index()].constructor != T::Int {
            return Err("Wasm: marker bits are not Int".into());
        }
        let target = self.local(ValType::I32);
        self.extend([
            I::LocalGet(0),
            I::I32Load(memory(8, 2)),
            I::I32Load(memory(DATA, 2)),
            I::LocalSet(target),
        ]);
        let bits = self.local(ValType::I64);
        // Canonical PropertyTarget member order comes from the admitted ABI.
        for (index, mask) in [4i64, 16, 8, 2, 1, 32].into_iter().enumerate() {
            self.extend([
                I::LocalGet(target),
                I::I32Const(index as i32),
                I::I32Eq,
                I::If(BlockType::Empty),
                I::I64Const(mask),
                I::LocalSet(bits),
                I::End,
            ]);
        }
        let previous = self.parameter(1);
        self.extend([
            I::LocalGet(previous),
            I::I32Load(memory(DATA, 2)),
            I::I32Const(1),
            I::I32Eq,
            I::If(BlockType::Empty),
        ]);
        let marker = self.enum_payload(args[1], 1, previous)?;
        let contents = self.table_data(RECORDS, marker, DATA);
        self.extend([
            I::LocalGet(bits),
            I::LocalGet(contents),
            I::I64Load(memory(DATA, 3)),
            I::I64Or,
            I::LocalSet(bits),
            I::End,
        ]);
        let field = self.scalar_as(node, integer, 0)?;
        self.extend([
            I::LocalGet(field),
            I::LocalGet(bits),
            I::I64Store(memory(DATA, 3)),
        ]);
        self.aggregate_result(node, output, RECORDS, field, self.width(integer)?)
    }
}
