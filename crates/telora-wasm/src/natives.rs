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
            kind: telora_core::ast::BindingKind::Native,
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
            Some((5, name)) => self.array_native(name),
            Some((26, "call_with_diagnostics")) => self.capture_diagnostics(),
            Some((18, "property")) => self.property_factory(),
            Some((
                25,
                name @ ("get_type_prop" | "get_field_prop" | "get_variant_prop" | "evidence"),
            )) => self.property_query(name),
            other => Err(format!("Wasm: native ABI not implemented: {other:?}")),
        }
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
                &[target],
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
            I::I32Load(memory(0, 2)),
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
        let id = self.table_push(RECORDS, field, self.width(integer)?);
        let result = self.value_as(node, output, self.width(output)?)?;
        self.extend([
            I::LocalGet(result),
            I::LocalGet(id),
            I::I64ExtendI32U,
            I::I64Store(memory(DATA, 3)),
        ]);
        Ok(result)
    }
}
