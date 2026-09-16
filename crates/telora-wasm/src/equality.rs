//! Specialized equality consumes pointers with a statically known common TypeId.
use crate::{abi::*, emit::Emitter, plan::child};
use telora_core::{
    candidate_layout::State,
    mir::{HirId, NativeTypeId, Role, TypeConstructor as T, TypeId},
};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn compare_call(&mut self, ty: TypeId, left: u32, right: u32) -> Result<(), String> {
        let key = self
            .plan
            .comparisons
            .get(&ty)
            .ok_or("Wasm: comparison type was not planned")?;
        self.extend([
            I::LocalGet(left),
            I::LocalGet(right),
            I::Call(self.plan.functions[key]),
        ]);
        Ok(())
    }
    pub(crate) fn unequal_if(&mut self) {
        self.extend([I::If(BlockType::Empty), I::I32Const(0), I::Return, I::End]);
    }
    fn compare_child(&mut self, ty: TypeId, left: u32, right: u32) -> Result<(), String> {
        self.compare_call(ty, left, right)?;
        self.emit(I::I32Eqz);
        self.unequal_if();
        Ok(())
    }
    pub fn equal_left(&mut self, node: HirId, negate: bool, left: u32) -> Result<u32, String> {
        let lhs = child(self.mir, node, Role::Left)?;
        let rhs = child(self.mir, node, Role::Right)?;
        let a = self.effective_ty(lhs)?;
        let b = self.effective_ty(rhs)?;
        if self.width(a)? == 0 {
            return Ok(left);
        }
        let right = self.expression(rhs)?;
        if self.width(b)? == 0 {
            return Ok(right);
        }
        let result = self.value(node, SCALAR_BYTES)?;
        self.emit(I::LocalGet(result));
        if a == b {
            self.compare_call(a, left, right)?;
        } else if [a, b]
            .iter()
            .all(|ty| matches!(self.mir.types[ty.index()].constructor, T::Type | T::TypeOf))
        {
            self.bits(left);
            self.bits(right);
            self.emit(I::I64Eq);
        } else {
            self.emit(I::I32Const(0));
        }
        if negate {
            self.emit(I::I32Eqz);
        }
        self.extend([I::I64ExtendI32U, I::I64Store(memory(DATA, 3))]);
        Ok(result)
    }
    pub fn equal_native(&mut self) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        if args.len() != 3
            || args[0] != args[1]
            || self.mir.types[args[2].index()].constructor != T::Bool
        {
            return Err("Wasm: equality native signature mismatch".into());
        }
        let left = self.parameter(0);
        let right = self.parameter(1);
        let result = self.value_as(node, args[2], SCALAR_BYTES)?;
        self.emit(I::LocalGet(result));
        self.compare_call(args[0], left, right)?;
        self.extend([I::I64ExtendI32U, I::I64Store(memory(DATA, 3))]);
        Ok(result)
    }
    pub fn compare_type(&mut self, ty: TypeId) -> Result<(), String> {
        let width = self.width(ty)?;
        if width == 0 {
            self.emit(I::Unreachable);
            return Ok(());
        }
        if width == HEADER_BYTES {
            self.emit(I::I32Const(1));
            return Ok(());
        }
        match self.mir.types[ty.index()].constructor {
            T::Native(id) if (id.module, id.slot) == (16, 3) => {
                let left = self.read32(0, DATA);
                let right = self.read32(1, DATA);
                self.extend([
                    I::I32Const(6),
                    I::LocalGet(left),
                    I::LocalGet(right),
                    I::Call(HASH),
                ]);
                return Ok(());
            }
            T::Native(id) if (id.module, id.slot) == (19, 0) => {
                let left = self.read32(0, DATA);
                let right = self.read32(1, DATA);
                self.extend([
                    I::I32Const(2),
                    I::LocalGet(left),
                    I::LocalGet(right),
                    I::Call(REGEX),
                ]);
                return Ok(());
            }
            T::Native(id) if (id.module, id.slot) == (20, 1) => {
                return self.compare_format(ty);
            }
            T::Dyn => {
                self.extend([
                    I::LocalGet(0),
                    I::I64Load(memory(DATA + 8, 3)),
                    I::LocalGet(1),
                    I::I64Load(memory(DATA + 8, 3)),
                    I::I64Eq,
                ]);
                return Ok(());
            }
            T::Int | T::Bool | T::Type | T::TypeOf | T::Function => {
                self.bits(0);
                self.bits(1);
                self.emit(I::I64Eq);
                return Ok(());
            }
            T::Native(NativeTypeId::BLAME_ERROR)
            | T::Native(NativeTypeId {
                module: 33,
                slot: 0,
            }) => {
                self.bits(0);
                self.bits(1);
                self.emit(I::I64Eq);
                return Ok(());
            }
            T::Float => {
                self.bits(0);
                self.emit(I::F64ReinterpretI64);
                self.bits(1);
                self.extend([I::F64ReinterpretI64, I::F64Eq]);
                return Ok(());
            }
            T::String => {
                self.extend([
                    I::LocalGet(0),
                    I::LocalGet(1),
                    I::Call(STRING_COMPARE),
                    I::I32Eqz,
                ]);
                return Ok(());
            }
            T::Array | T::Dict | T::Bytes => return self.compare_sequence(ty),
            _ => {}
        }
        let variants = &self.plan.layouts[ty.index()].variants;
        if !variants.is_empty() {
            let variants = variants.iter().map(|v| v.type_id).collect::<Vec<_>>();
            self.bits(0);
            self.bits(1);
            self.emit(I::I64Ne);
            self.unequal_if();
            for (index, payload) in variants.into_iter().enumerate() {
                self.bits(0);
                self.extend([I::I64Const(index as i64), I::I64Eq, I::If(BlockType::Empty)]);
                if let Some(payload) = payload {
                    let payload = self.plan.layouts[payload].id();
                    if self.width(payload)? == 0 {
                        self.emit(I::Unreachable);
                    } else {
                        let left = self.enum_payload(ty, index as u32, 0)?;
                        let right = self.enum_payload(ty, index as u32, 1)?;
                        self.compare_child(payload, left, right)?;
                    }
                }
                self.extend([I::I32Const(1), I::Return, I::End]);
            }
            self.emit(I::Unreachable);
            return Ok(());
        }
        let table = match &self.plan.layouts[ty.index()].layout {
            State::Known { shape } => match shape.table {
                Some("RecordTable") => RECORDS,
                Some("NewtypeTable") => NEWTYPES,
                _ => {
                    return Err(format!(
                        "Wasm: equality not implemented for sealed type {ty:?}"
                    ));
                }
            },
            _ => return Err("Wasm: equality type has no layout".into()),
        };
        let members = self.plan.layouts[ty.index()]
            .object
            .as_ref()
            .ok_or("Wasm: equality members missing")?
            .members
            .iter()
            .map(|m| (m.type_id, m.offset))
            .collect::<Vec<_>>();
        let left = self.table_data(table, 0, DATA);
        let right = self.table_data(table, 1, DATA);
        for (member, offset) in members {
            let member =
                self.plan.layouts[member.ok_or("Wasm: equality member type missing")?].id();
            let offset = offset.ok_or("Wasm: equality member offset missing")?;
            let a = self.local(ValType::I32);
            let b = self.local(ValType::I32);
            for (base, value) in [(left, a), (right, b)] {
                self.extend([
                    I::LocalGet(base),
                    I::I32Const(offset as i32),
                    I::I32Add,
                    I::LocalSet(value),
                ]);
            }
            self.compare_child(member, a, b)?;
        }
        self.emit(I::I32Const(1));
        Ok(())
    }
    fn compare_sequence(&mut self, ty: TypeId) -> Result<(), String> {
        let kind = &self.mir.types[ty.index()].constructor;
        let bytes = *kind == T::Bytes;
        let dict = *kind == T::Dict;
        let element = (!bytes).then(|| self.mir.types[ty.index()].arguments[0]);
        let width = if let Some(element) = element {
            self.width(element)?
        } else {
            1
        };
        let (left, a) = self.comparison_parts(0, width, bytes, dict);
        let (right, b) = self.comparison_parts(1, width, bytes, dict);
        self.extend([I::LocalGet(a), I::LocalGet(b), I::I32Ne]);
        self.unequal_if();
        let keys = if dict {
            Some((
                self.table_data(ARRAYS, 0, DATA),
                self.table_data(ARRAYS, 1, DATA),
            ))
        } else {
            None
        };
        let index = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(a),
            I::I32GeU,
            I::BrIf(1),
        ]);
        if let Some((left, right)) = keys {
            let left = self.array_item(left, index, STRING_BYTES);
            let right = self.array_item(right, index, STRING_BYTES);
            self.extend([
                I::LocalGet(left),
                I::LocalGet(right),
                I::Call(STRING_COMPARE),
            ]);
            self.unequal_if();
        }
        let left = self.array_item(left, index, width);
        let right = self.array_item(right, index, width);
        if let Some(element) = element {
            self.compare_child(element, left, right)?;
        } else {
            self.extend([
                I::LocalGet(left),
                I::I32Load8U(memory(0, 0)),
                I::LocalGet(right),
                I::I32Load8U(memory(0, 0)),
                I::I32Ne,
            ]);
            self.unequal_if();
        }
        self.extend([
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
            I::I32Const(1),
        ]);
        Ok(())
    }
    fn comparison_parts(&mut self, value: u32, width: u32, bytes: bool, dict: bool) -> (u32, u32) {
        if !bytes && !dict {
            return self.array_parts(value, width);
        }
        let data = self.table_data(
            if bytes { BYTES } else { ARRAYS },
            value,
            if dict { DATA + 8 } else { DATA },
        );
        let count = self.local(ValType::I32);
        if bytes {
            self.extend([
                I::LocalGet(data),
                I::LocalGet(value),
                I::I32Load(memory(DATA + 4, 2)),
                I::I32Add,
                I::LocalSet(data),
                I::LocalGet(value),
                I::I32Load(memory(DATA + 8, 2)),
                I::LocalGet(value),
                I::I32Load(memory(DATA + 4, 2)),
                I::I32Sub,
                I::LocalSet(count),
            ]);
        } else {
            self.extend([
                I::LocalGet(value),
                I::I32Load(memory(DATA + 4, 2)),
                I::LocalSet(count),
            ]);
        }
        (data, count)
    }
}
