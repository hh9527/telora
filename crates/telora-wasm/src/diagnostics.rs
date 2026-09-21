//! Blame objects and diagnostic events are constructed inside Wasm.
use crate::{abi::*, emit::Emitter, plan::child};
use telora_core::{
    mir::{HirId, NativeTypeId, Role, TypeConstructor as T},
    syntax::kinds::BlameAction,
};
use wasm_encoder::{Instruction as I, ValType};

impl Emitter<'_> {
    pub fn blame_parts(&mut self, value: u32) -> (u32, u32, u32) {
        let message = self.table_data(BLAMES, value, DATA);
        let subjects = self.local(ValType::I32);
        let count = self.local(ValType::I32);
        self.extend([
            I::LocalGet(message),
            I::I32Const(BLAME_SUBJECTS as i32),
            I::I32Add,
            I::LocalSet(subjects),
            I::LocalGet(message),
            I::I32Load(memory(BLAME_COUNT as u64, 2)),
            I::LocalSet(count),
        ]);
        (message, subjects, count)
    }
    pub fn report(&mut self, node: HirId, message: u32, subjects: u32, count: u32, warning: bool) {
        let packet = self.alloc(DIAGNOSTIC_BYTES);
        let loc = self.mir.hir[node.index()].location;
        self.store_location(packet, loc);
        for (offset, word) in [(DIAG_CODE, ERROR_USER), (DIAG_WARNING, u32::from(warning))] {
            self.store32(packet, offset, word);
        }
        self.store32(
            packet,
            DIAG_MESSAGE_TYPE,
            self.string_type().expect("sealed String type").index() as u32,
        );
        for (offset, value) in [
            (DIAG_MESSAGE, message),
            (DIAG_SUBJECTS, subjects),
            (DIAG_COUNT, count),
        ] {
            self.extend([
                I::LocalGet(packet),
                I::LocalGet(value),
                I::I32Store(memory(offset, 2)),
            ]);
        }
        self.extend([
            I::LocalGet(packet),
            I::GlobalGet(INITIALIZATION_ROOT_GLOBAL),
            I::I32Store(memory(DIAG_ROOT, 2)),
        ]);
        self.table_push(DIAGNOSTICS, packet, DIAGNOSTIC_BYTES);
        if !warning {
            self.extend([
                I::LocalGet(packet),
                I::GlobalSet(ERROR_GLOBAL),
                I::I32Const(3),
                I::GlobalSet(PHASE_GLOBAL),
                I::I32Const(0),
                I::Return,
            ]);
        }
    }
    pub fn raise_values(
        &mut self,
        node: HirId,
        action: BlameAction,
        message: u32,
        values: &[u32],
    ) -> Result<u32, String> {
        let message_node = child(self.mir, node, Role::Value)?;
        let message_type = self.ty(message_node)?;
        let (message, subjects, count) =
            if self.mir.types[message_type.index()].constructor == T::String {
                let subjects = self.alloc(values.len() as u32 * LOC_BYTES);
                for (index, value) in values.iter().enumerate() {
                    self.copy(subjects, index as u32 * LOC_BYTES, *value, LOC_BYTES);
                }
                let count = self.local(ValType::I32);
                self.extend([I::I32Const(values.len() as i32), I::LocalSet(count)]);
                (message, subjects, count)
            } else if matches!(action, BlameAction::Warn | BlameAction::Raise)
                && self.mir.types[message_type.index()].constructor
                    == T::Native(NativeTypeId::BLAME_ERROR)
            {
                self.blame_parts(message)
            } else {
                return Err("Wasm: diagnostic message does not match its sealed contract".into());
            };
        if action == BlameAction::Build {
            let bytes = BLAME_SUBJECTS + values.len() as u32 * LOC_BYTES;
            let object = self.alloc(bytes);
            self.copy(object, 0, message, STRING_BYTES);
            self.store32(object, BLAME_COUNT as u64, values.len() as u32);
            self.copy(
                object,
                BLAME_SUBJECTS,
                subjects,
                values.len() as u32 * LOC_BYTES,
            );
            let id = self.table_push(BLAMES, object, bytes);
            let result = self.value(node, SCALAR_BYTES)?;
            self.extend([
                I::LocalGet(result),
                I::LocalGet(id),
                I::I64ExtendI32U,
                I::I64Store(memory(DATA, 3)),
            ]);
            return Ok(result);
        }
        let warning = action == BlameAction::Warn;
        self.report(node, message, subjects, count, warning);
        if warning {
            self.enum_value(node, self.effective_ty(node)?, 0, None)
        } else {
            Ok(self.local(ValType::I32))
        }
    }
}
