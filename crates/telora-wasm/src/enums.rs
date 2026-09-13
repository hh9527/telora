use crate::{abi::*, emit::Emitter, plan::child};
use telora_core::mir::{
    HirId, HirKind, MemberSelection, Mir, ResolveState, Role, TypeConstructor as T, TypeId,
};
use wasm_encoder::{Instruction as I, ValType};

pub(crate) fn selection(mir: &Mir, mut node: HirId) -> Option<MemberSelection> {
    let mut visited = std::collections::BTreeSet::new();
    while visited.insert(node) {
        if let Some(selection) = mir.member_selections[node.index()] {
            return Some(selection);
        }
        if matches!(mir.hir[node.index()].kind, HirKind::TypeApply) {
            node = child(mir, node, Role::Callee).ok()?;
            continue;
        }
        let slot = mir.hir[node.index()].resolution?;
        let ResolveState::Bound(symbol) = mir.resolve_slots[slot.index()] else {
            return None;
        };
        let declaration = *mir.symbols[symbol.index()].declarations.last()?;
        node = child(mir, declaration, Role::Value).ok()?;
    }
    None
}

impl Emitter<'_> {
    pub fn adapt(
        &mut self,
        node: HirId,
        actual: TypeId,
        expected: TypeId,
        value: u32,
    ) -> Result<u32, String> {
        if actual == expected {
            return Ok(value);
        }
        // A sealed Never expression has already emitted a terminator. No value
        // conversion or materialization occurs in this unreachable continuation.
        if self.mir.types[actual.index()].constructor == T::Never {
            return Ok(value);
        }
        if self.mir.types[actual.index()].constructor == T::TypeOf
            && self.mir.types[expected.index()].constructor == T::Type
        {
            let result = self.value_as(node, expected, self.width(expected)?)?;
            self.copy(result, 0, value, self.width(expected)?);
            self.store32(result, TYPE, expected.index() as u32);
            return Ok(result);
        }
        Err(format!(
            "Wasm: sealed boundary adaptation missing: {actual:?} -> {expected:?} at {node:?}"
        ))
    }
    pub fn parameter(&mut self, index: u32) -> u32 {
        let value = self.local(ValType::I32);
        self.extend([
            I::LocalGet(1),
            I::I32Load(memory(u64::from(index) * 4, 2)),
            I::LocalSet(value),
        ]);
        value
    }
    pub fn enum_value(
        &mut self,
        node: HirId,
        ty: TypeId,
        index: u32,
        payload: Option<u32>,
    ) -> Result<u32, String> {
        if let Some(payload) = payload {
            self.construction_check(
                node,
                ty,
                telora_core::mir::PropertySite::Variant(index),
                payload,
            )?;
        }
        let branch = self.plan.layouts[ty.index()]
            .variants
            .get(index as usize)
            .ok_or("Wasm: enum variant not sealed")?;
        let result = self.value_as(node, ty, self.width(ty)?)?;
        self.extend([
            I::LocalGet(result),
            I::I64Const(index as i64),
            I::I64Store(memory(DATA, 3)),
        ]);
        match (branch.type_id, payload) {
            (None, None) => {}
            (Some(payload_ty), Some(value)) => {
                let width = self.width(self.plan.layouts[payload_ty].id())?;
                match branch.storage {
                    "full_value" => self.copy(result, 24, value, width),
                    "heap_id" => {
                        let id = self.table_push(VALUES, value, width);
                        self.extend([
                            I::LocalGet(result),
                            I::LocalGet(id),
                            I::I64ExtendI32U,
                            I::I64Store(memory(24, 3)),
                        ]);
                    }
                    _ => return Err("Wasm: enum payload storage is not materializable".into()),
                }
            }
            _ => return Err("Wasm: enum payload arity mismatch".into()),
        }
        Ok(result)
    }
    pub fn enum_payload(&mut self, ty: TypeId, index: u32, value: u32) -> Result<u32, String> {
        let branch = self.plan.layouts[ty.index()]
            .variants
            .get(index as usize)
            .ok_or("Wasm: missing sealed enum branch")?;
        if branch.type_id.is_none() {
            return Err("Wasm: nullary branch has no payload".into());
        }
        match branch.storage {
            "full_value" => {
                let result = self.local(ValType::I32);
                self.extend([
                    I::LocalGet(value),
                    I::I32Const(24),
                    I::I32Add,
                    I::LocalSet(result),
                ]);
                Ok(result)
            }
            "heap_id" => Ok(self.table_data(VALUES, value, 24)),
            _ => Err("Wasm: enum payload storage is not materializable".into()),
        }
    }
    pub fn constructor(&mut self) -> Result<u32, String> {
        let node = self.key.node;
        let signature = self.ty(node)?;
        let arguments = &self.mir.types[signature.index()].arguments;
        if arguments.len() != 2 {
            return Err("Wasm: constructor must have a closed unary signature".into());
        }
        let output = arguments[1];
        let payload = self.parameter(0);
        match selection(self.mir, node).ok_or("Wasm: missing constructor selection")? {
            MemberSelection::EnumVariant { index } => {
                self.enum_value(node, output, index, Some(payload))
            }
            MemberSelection::NewtypeConstructor => {
                self.construction_check(
                    node,
                    output,
                    telora_core::mir::PropertySite::Type,
                    payload,
                )?;
                let id = self.table_push(NEWTYPES, payload, self.width(arguments[0])?);
                let result = self.value_as(node, output, self.width(output)?)?;
                self.extend([
                    I::LocalGet(result),
                    I::LocalGet(id),
                    I::I64ExtendI32U,
                    I::I64Store(memory(DATA, 3)),
                ]);
                Ok(result)
            }
            _ => Err("Wasm: not a callable constructor".into()),
        }
    }
}
