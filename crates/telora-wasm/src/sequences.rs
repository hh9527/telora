//! Sealed sequence contributions are evaluated once, in source order.
use crate::{abi::*, emit::Emitter, plan::child};
use telora_core::mir::{HirId, HirKind, Role, TypeConstructor as T};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub fn array_expression(&mut self, node: HirId) -> Result<u32, String> {
        let ty = self.effective_ty(node)?;
        if self.mir.types[ty.index()].constructor != T::Array {
            return Err("Wasm: array needs sealed Array type".into());
        }
        let element = self.mir.types[ty.index()].arguments[0];
        let width = self.width(element)?;
        let items = self.mir.hir[node.index()]
            .children
            .iter()
            .filter(|edge| edge.role == Role::Item)
            .map(|edge| edge.node)
            .collect::<Vec<_>>();
        let total = self.local(ValType::I64);
        let mut parts = Vec::new();
        for item in items {
            let (base, count, actual) =
                if matches!(self.mir.hir[item.index()].kind, HirKind::Spread) {
                    let operand = child(self.mir, item, Role::Operand)?;
                    let source = self.effective_ty(operand)?;
                    if self.mir.types[source.index()].constructor != T::Array {
                        return Err("Wasm: array spread requires sealed Array operand".into());
                    }
                    let actual = self.mir.types[source.index()].arguments[0];
                    let value = self.expression(operand)?;
                    let (base, count) = self.array_parts(value, self.width(actual)?);
                    (base, count, actual)
                } else {
                    let actual = self.effective_ty(item)?;
                    let value = self.expression(item)?;
                    let value = self.adapt(item, actual, element, value)?;
                    let count = self.local(ValType::I32);
                    self.extend([I::I32Const(1), I::LocalSet(count)]);
                    (value, count, element)
                };
            self.extend([
                I::LocalGet(total),
                I::LocalGet(count),
                I::I64ExtendI32U,
                I::I64Add,
                I::LocalTee(total),
                I::I64Const(u32::MAX as i64),
                I::I64GtU,
            ]);
            self.fail_if(node, ERROR_OVERFLOW);
            parts.push((item, base, count, actual));
        }
        let count = self.local(ValType::I32);
        self.extend([I::LocalGet(total), I::I32WrapI64, I::LocalSet(count)]);
        let data = self.array_storage(count, width);
        let cursor = self.local(ValType::I32);
        self.extend([I::LocalGet(data), I::LocalSet(cursor)]);
        for (item, base, length, actual) in parts {
            if actual == element {
                self.extend([
                    I::LocalGet(cursor),
                    I::LocalGet(base),
                    I::LocalGet(length),
                    I::I32Const(width as i32),
                    I::I32Mul,
                    I::MemoryCopy {
                        src_mem: 0,
                        dst_mem: 0,
                    },
                ]);
            } else if self.mir.types[actual.index()].constructor == T::Never {
                // Only an empty collection can contain Never. No element exists
                // to convert, and a malformed nonempty one is an ABI violation.
                self.extend([
                    I::LocalGet(length),
                    I::If(BlockType::Empty),
                    I::Unreachable,
                    I::End,
                ]);
            } else {
                let index = self.local(ValType::I32);
                self.extend([
                    I::Block(BlockType::Empty),
                    I::Loop(BlockType::Empty),
                    I::LocalGet(index),
                    I::LocalGet(length),
                    I::I32GeU,
                    I::BrIf(1),
                ]);
                let source = self.array_item(base, index, self.width(actual)?);
                let value = self.adapt(item, actual, element, source)?;
                let target = self.array_item(cursor, index, width);
                self.copy(target, 0, value, width);
                self.extend([
                    I::LocalGet(index),
                    I::I32Const(1),
                    I::I32Add,
                    I::LocalSet(index),
                    I::Br(0),
                    I::End,
                    I::End,
                ]);
            }
            self.extend([
                I::LocalGet(cursor),
                I::LocalGet(length),
                I::I32Const(width as i32),
                I::I32Mul,
                I::I32Add,
                I::LocalSet(cursor),
            ]);
        }
        self.array_result_at(node, ty, data, count, width)
    }

    pub fn tuple_expression(&mut self, node: HirId) -> Result<u32, String> {
        let ty = self.effective_ty(node)?;
        let targets = self.plan.layouts[ty.index()]
            .object
            .as_ref()
            .map(|layout| {
                layout
                    .members
                    .iter()
                    .map(|member| member.type_id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let items = self.mir.hir[node.index()]
            .children
            .iter()
            .filter(|edge| edge.role == Role::Item)
            .map(|edge| edge.node)
            .collect::<Vec<_>>();
        if self.width(ty)? == 0 {
            for item in items {
                let operand = if matches!(self.mir.hir[item.index()].kind, HirKind::Spread) {
                    child(self.mir, item, Role::Operand)?
                } else {
                    item
                };
                let value = self.expression(operand)?;
                if self.width(self.effective_ty(operand)?)? == 0 {
                    // An uninhabited operand emitted a terminator. There is no
                    // tuple object to build and later operands are unreachable.
                    return Ok(value);
                }
            }
            return Err("Wasm: uninhabited tuple has no terminating contribution".into());
        }
        let mut fields = Vec::new();
        for item in items {
            if matches!(self.mir.hir[item.index()].kind, HirKind::Spread) {
                let operand = child(self.mir, item, Role::Operand)?;
                let source = self.effective_ty(operand)?;
                let value = self.expression(operand)?;
                if self.plan.layouts[source.index()].object.is_none()
                    && self.width(source)? != HEADER_BYTES
                {
                    return Err("Wasm: tuple spread has no sealed object layout".into());
                }
                let members = self.plan.layouts[source.index()]
                    .object
                    .as_ref()
                    .map(|layout| {
                        layout
                            .members
                            .iter()
                            .map(|member| (member.type_id, member.offset))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                // Unit spreads still evaluate their operand but have no object.
                if !members.is_empty() {
                    let data = self.table_data(RECORDS, value, DATA);
                    for (type_id, offset) in members {
                        let actual = self.plan.layouts
                            [type_id.ok_or("Wasm: tuple member type missing")?]
                        .id();
                        let expected = self.plan.layouts[targets
                            .get(fields.len())
                            .copied()
                            .flatten()
                            .ok_or("Wasm: tuple spread exceeds sealed shape")?]
                        .id();
                        let field = self.local(ValType::I32);
                        self.extend([
                            I::LocalGet(data),
                            I::I32Const(offset.ok_or("Wasm: tuple offset missing")? as i32),
                            I::I32Add,
                            I::LocalSet(field),
                        ]);
                        fields.push(self.adapt(item, actual, expected, field)?);
                    }
                }
            } else {
                let expected = self.plan.layouts[targets
                    .get(fields.len())
                    .copied()
                    .flatten()
                    .ok_or("Wasm: tuple item exceeds sealed shape")?]
                .id();
                let value = self.expression(item)?;
                fields.push(self.adapt(item, self.effective_ty(item)?, expected, value)?);
            }
        }
        if fields.len() != targets.len() {
            return Err("Wasm: tuple contributions do not fill sealed shape".into());
        }
        self.packed_tuple_at(node, ty, &fields)
    }
}
