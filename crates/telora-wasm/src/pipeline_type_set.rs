//! Dense visited bits for the explicitly gated handwritten construction oracle.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::TypeConstructor as T;
use wasm_encoder::{Instruction as I, ValType};
impl Emitter<'_> {
    pub(super) fn oracle_type_set_native(&mut self, name: &str) -> Result<u32, String> {
        let args = self.mir.types[self.ty(self.key.node)?.index()]
            .arguments
            .clone();
        let output = *args.last().ok_or("missing oracle type-set signature")?;
        let set = if name == "oracle_type_set" {
            output
        } else {
            args[0]
        };
        if self.sequence_name(set, 39)? != "OracleTypeSet" {
            return Err("invalid experimental type-set identity".into());
        }
        let bits_ty = self.diagnostic_field_type(set, "bits")?;
        if self.mir.types[bits_ty.index()].constructor != T::Bytes {
            return Err("invalid experimental type-set storage".into());
        }
        if name == "oracle_type_set" && args.len() == 1 {
            let size = u32::try_from(self.mir.types.len().div_ceil(8).max(16))
                .map_err(|_| "type-set size overflow")?;
            let data = self.alloc(size);
            self.extend([
                I::LocalGet(data),
                I::I32Const(0),
                I::I32Const(size as i32),
                I::MemoryFill(0),
            ]);
            let count = self.local(ValType::I32);
            self.extend([I::I32Const(size as i32), I::LocalSet(count)]);
            let bits = self.byte_span_value(bits_ty, data, count)?;
            self.root_value(bits, bits_ty);
            return self.packed_tuple(output, &[bits]);
        }
        if name != "oracle_type_first"
            || args.len() != 3
            || self.mir.types[args[1].index()].constructor != T::Type
            || self.mir.types[output.index()].constructor != T::Bool
        {
            return Err("invalid experimental type-set operation".into());
        }
        let set = self.parameter(0);
        let ty = self.parameter(1);
        self.bits(ty);
        self.extend([I::I64Const(self.mir.types.len() as i64), I::I64GeU]);
        self.fail_if(self.key.node, ERROR_INDEX);
        let (bits, _) = self.sequence_field(set, args[0], "bits")?;
        // At least 16 bytes: CONTENT_WRITE uses owned Raw storage (no inline case).
        let raw = self.read32(bits, DATA);
        let id = self.read32(ty, DATA);
        let address = self.local(ValType::I32);
        let mask = self.local(ValType::I32);
        self.extend([
            I::LocalGet(raw),
            I::I32Const(8),
            I::I32Add,
            I::LocalGet(id),
            I::I32Const(3),
            I::I32ShrU,
            I::I32Add,
            I::LocalSet(address),
            I::I32Const(1),
            I::LocalGet(id),
            I::I32Const(7),
            I::I32And,
            I::I32Shl,
            I::LocalSet(mask),
        ]);
        let old = self.local(ValType::I32);
        self.extend([
            I::LocalGet(address),
            I::I32Load8U(memory(0, 0)),
            I::LocalSet(old),
            I::LocalGet(address),
            I::LocalGet(old),
            I::LocalGet(mask),
            I::I32Or,
            I::I32Store8(memory(0, 0)),
        ]);
        let result = self.scalar_as(self.key.node, output, 0)?;
        self.extend([
            I::LocalGet(result),
            I::LocalGet(old),
            I::LocalGet(mask),
            I::I32And,
            I::I32Eqz,
            I::I64ExtendI32U,
            I::I64Store(memory(DATA, 3)),
        ]);
        Ok(result)
    }
}
