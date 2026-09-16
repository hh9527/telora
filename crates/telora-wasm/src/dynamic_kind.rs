//! Value-kind queries classify a closed witness, never infer a type.
use crate::{
    abi::*,
    emit::Emitter,
    reflection_data::{MEMBER, ROW},
};
use telora_core::mir::TypeConstructor as T;
use wasm_encoder::{BlockType, Instruction as I};

impl Emitter<'_> {
    pub fn dynamic_value_kind(&mut self) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        if args.len() != 2 || self.mir.types[args[0].index()].constructor != T::Dyn {
            return Err("Wasm: Dyn kind signature mismatch".into());
        }
        let output = args[1];
        let input = self.parameter(0);
        let id = self.read32(input, DATA);
        let (base, row) = self.type_row(id);
        let body = self.read32(row, 4);
        self.extend([
            I::LocalGet(body),
            I::I32Const(-1),
            I::I32Ne,
            I::If(BlockType::Empty),
            I::LocalGet(base),
            I::LocalGet(body),
            I::I32Const(ROW as i32),
            I::I32Mul,
            I::I32Add,
            I::LocalSet(row),
            I::End,
        ]);
        let kind = self.read32(row, 0);
        for (tag, name) in [
            (1, "Type"),
            (2, "Type"),
            (3, "Int"),
            (4, "Float"),
            (5, "String"),
            (6, "Bytes"),
            (7, "Array"),
            (8, "Dict"),
            (9, "Tuple"),
            (10, "Dict"),
            (11, "Tuple"),
            (13, "Func"),
            (14, "Opaque"),
            (16, "Dyn"),
        ] {
            self.extend([
                I::LocalGet(kind),
                I::I32Const(tag),
                I::I32Eq,
                I::If(BlockType::Empty),
            ]);
            let result = self.dynamic_kind_variant(output, name, input)?;
            self.extend([I::LocalGet(result), I::Return, I::End]);
        }
        self.extend([
            I::LocalGet(kind),
            I::I32Const(12),
            I::I32Eq,
            I::If(BlockType::Empty),
        ]);
        let value = self.table_data(VALUES, input, DATA + 8);
        let tag = self.read32(value, DATA);
        let members = self.read32(row, 16);
        self.extend([
            I::LocalGet(base),
            I::LocalGet(members),
            I::I32Add,
            I::LocalSet(members),
        ]);
        let member = self.array_item(members, tag, MEMBER);
        let payload = self.read32(member, 8);
        self.extend([
            I::LocalGet(payload),
            I::I32Const(-1),
            I::I32Eq,
            I::If(BlockType::Empty),
        ]);
        let atom = self.dynamic_kind_variant(output, "Atom", input)?;
        self.extend([I::LocalGet(atom), I::Return, I::Else]);
        let tagged = self.dynamic_kind_variant(output, "Tagged", input)?;
        self.extend([
            I::LocalGet(tagged),
            I::Return,
            I::End,
            I::End,
            I::Unreachable,
        ]);
        Ok(input)
    }

    fn dynamic_kind_variant(
        &mut self,
        output: telora_core::mir::TypeId,
        name: &str,
        input: u32,
    ) -> Result<u32, String> {
        let index = self.plan.layouts[output.index()]
            .variants
            .iter()
            .position(|v| v.name == name && v.type_id.is_none())
            .ok_or("Wasm: Dyn ValueKind variant missing")?;
        let result = self.enum_value(self.key.node, output, index as u32, None)?;
        self.copy(result, 0, input, LOC_BYTES);
        Ok(result)
    }
}
