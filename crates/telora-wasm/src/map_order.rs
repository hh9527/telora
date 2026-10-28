//! Deterministic key order for scalar keys and finite products/newtypes.
use crate::{abi::*, emit::Emitter};
use telora_core::{
    candidate_layout::State,
    mir::{TypeConstructor as T, TypeId},
};
use wasm_encoder::{BlockType, Instruction as I, ValType};
impl Emitter<'_> {
    pub(crate) fn map_order_value(
        &mut self,
        ty: TypeId,
        left: u32,
        right: u32,
        depth: usize,
    ) -> Result<u32, String> {
        if depth > 64 {
            return Err("Map key order does not support recursive key types".into());
        }
        let out = self.local(ValType::I32);
        if self.width(ty)? == 0 {
            self.emit(I::Unreachable);
            return Ok(out);
        }
        if self.mir.types[ty.index()].constructor == T::Tuple
            && self.mir.types[ty.index()].arguments.is_empty()
        {
            return Ok(out);
        }
        match self.mir.types[ty.index()].constructor {
            T::String | T::Bytes => self.extend([
                I::LocalGet(left),
                I::LocalGet(right),
                I::Call(STRING_COMPARE),
                I::LocalSet(out),
            ]),
            T::Int | T::Bool | T::Type | T::TypeOf => {
                let a = self.local(ValType::I64);
                let b = self.local(ValType::I64);
                self.bits(left);
                self.emit(I::LocalSet(a));
                self.bits(right);
                self.emit(I::LocalSet(b));
                self.extend([
                    I::LocalGet(a),
                    I::LocalGet(b),
                    I::I64GtS,
                    I::LocalGet(a),
                    I::LocalGet(b),
                    I::I64LtS,
                    I::I32Sub,
                    I::LocalSet(out),
                ]);
            }
            _ => {
                let table =
                    match &self.plan.layouts[ty.index()].layout {
                        State::Known { shape } => match shape.table {
                            Some("RecordTable") => RECORDS,
                            Some("NewtypeTable") => NEWTYPES,
                            _ => return Err(
                                "Map key order supports scalar keys, structs, tuples and newtypes"
                                    .into(),
                            ),
                        },
                        _ => return Err("Map key layout missing".into()),
                    };
                let fields = self.plan.layouts[ty.index()]
                    .object
                    .as_ref()
                    .ok_or("Map key fields missing")?
                    .members
                    .iter()
                    .map(|m| {
                        (
                            self.plan.layouts[m.type_id.unwrap()].id(),
                            m.offset.unwrap() as u32,
                        )
                    })
                    .collect::<Vec<_>>();
                let l = self.table_data(table, left, DATA);
                let r = self.table_data(table, right, DATA);
                self.emit(I::Block(BlockType::Empty));
                for (t, offset) in fields {
                    let a = self.local(ValType::I32);
                    let b = self.local(ValType::I32);
                    for (base, dst) in [(l, a), (r, b)] {
                        self.extend([
                            I::LocalGet(base),
                            I::I32Const(offset as i32),
                            I::I32Add,
                            I::LocalSet(dst),
                        ])
                    }
                    let c = self.map_order_value(t, a, b, depth + 1)?;
                    self.extend([I::LocalGet(c), I::LocalTee(out), I::BrIf(0)]);
                }
                self.emit(I::End);
            }
        }
        Ok(out)
    }
    pub(crate) fn map_key_compare(&mut self) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        if args.len() != 3
            || args[0] != args[1]
            || self.mir.types[args[2].index()].constructor != T::Int
        {
            return Err("Map comparator signature mismatch".into());
        }
        let left = self.parameter(0);
        let right = self.parameter(1);
        let result = self.value_as(node, args[2], SCALAR_BYTES)?;
        let order = self.map_order_value(args[0], left, right, 0)?;
        self.extend([
            I::LocalGet(result),
            I::LocalGet(order),
            I::I64ExtendI32S,
            I::I64Store(memory(DATA, 3)),
        ]);
        Ok(result)
    }
}
