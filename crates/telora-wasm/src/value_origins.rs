//! New native values consume a call-site constant; forwarding never rewrites it.
use crate::{abi::*, emit::Emitter, plan::Special};
use std::{cell::RefCell, collections::BTreeMap};
use telora_core::mir::HirId;
use wasm_encoder::{BlockType, Instruction as I, ValType};

#[derive(Default)]
pub(crate) struct Constants {
    pub bytes: Vec<u8>,
    offsets: BTreeMap<u64, u32>,
}

pub(crate) type OriginConstants = RefCell<Constants>;

impl Emitter<'_> {
    pub fn is_native_body(&self) -> bool {
        self.key.callable
            && self.key.special == Special::Normal
            && crate::natives::identity(self.mir, self.key.node).is_some()
    }

    /// The optional final argument word is an address, not an allocated Loc.
    pub fn native_origin(&mut self) -> Result<u32, String> {
        let arity = self.mir.types[self.ty(self.key.node)?.index()]
            .arguments
            .len()
            - 1;
        let origin = self.local(ValType::I32);
        self.extend([
            I::LocalGet(1),
            I::I32Load(memory(arity as u64 * 4, 2)),
            I::LocalSet(origin),
        ]);
        Ok(origin)
    }

    pub fn computation_origin(&mut self, node: HirId) -> Result<u32, String> {
        if self.is_native_body() && node == self.key.node {
            return self.native_origin();
        }
        let loc = self.mir.hir[node.index()].location;
        let packed = telora_wasm_shared::source_range::SourceRange::checked(
            loc.source.get(),
            loc.start,
            loc.end,
            telora_wasm_shared::source_range::OFFSET_LIMIT - 1,
        )
        .ok_or("Wasm: source location exceeds packed range")?
        .packed();
        let offset = {
            let mut constants = self.plan.origins.borrow_mut();
            if let Some(&offset) = constants.offsets.get(&packed) {
                offset
            } else {
                let offset = u32::try_from(self.plan.reflection.len() + constants.bytes.len())
                    .map_err(|_| "Wasm: source constants exceed wasm32")?;
                constants.bytes.extend_from_slice(&packed.to_le_bytes());
                constants.offsets.insert(packed, offset);
                offset
            }
        };
        let base = self.static_base();
        self.extend([
            I::LocalGet(base),
            I::I32Const(offset as i32),
            I::I32Add,
            I::LocalSet(base),
        ]);
        Ok(base)
    }

    pub fn produced_origin(&mut self, value: u32, node: HirId) -> Result<(), String> {
        if self.is_native_body() && node == self.key.node {
            let origin = self.native_origin()?;
            self.extend([I::LocalGet(origin), I::If(BlockType::Empty)]);
            self.copy(value, 0, origin, LOC_BYTES);
            self.emit(I::Else);
            self.store64(value, SOURCE, 0);
            self.emit(I::End);
        } else {
            self.store_location(value, self.mir.hir[node.index()].location);
        }
        Ok(())
    }
}
