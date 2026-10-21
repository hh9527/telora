//! Explicit shadow slots mirror typed ownership, never the bits in arbitrary locals.
use crate::{abi::*, emit::Emitter, object::ObjectFunction, plan::Special};
use telora_core::mir::{HirKind, TypeId};
use wasm_encoder::Instruction as I;

impl Emitter<'_> {
    pub fn root_clear(&mut self, slot: u32) {
        self.extend([
            I::I32Const(slot as i32),
            I::I32Const(0),
            I::I32Const(ROOT_STORAGE as i32),
            I::I32Const(0),
            I::I32Const(0),
            I::Call(ROOTS_SET),
            I::Drop,
        ]);
    }

    pub fn root_storage(&mut self, local: u32) {
        self.root_publish(local, ROOT_STORAGE, 0, None);
    }

    pub fn root_plan(&mut self, local: u32) {
        self.root_publish(local, ROOT_PARSE_PLAN, 0, None);
    }

    pub fn root_value(&mut self, local: u32, ty: TypeId) {
        self.root_publish(local, ROOT_VALUE, ty.index() as u32, None);
    }

    fn root_publish(&mut self, local: u32, role: u32, ty: u32, extent: Option<u32>) {
        self.extend([
            I::I32Const(local as i32),
            I::LocalGet(local),
            I::I32Const(role as i32),
            I::I32Const(ty as i32),
            extent.map_or(I::I32Const(0), I::LocalGet),
            I::Call(ROOTS_SET),
            I::Drop,
        ]);
    }

    /// Initialized bytes, not capacity. Raw prefixes contain no fabricated type.
    pub fn root_prefix(&mut self, local: u32, tag: u32, ty: TypeId, initialized: u32) {
        self.root_publish(local, tag, ty.index() as u32, Some(initialized));
    }

    pub fn root_array_prefix(&mut self, local: u32, ty: TypeId, count: u32) -> Result<(), String> {
        self.extend([
            I::I32Const(local as i32),
            I::LocalGet(local),
            I::I32Const(ARRAYS as i32),
            I::I32Const(ty.index() as i32),
            I::LocalGet(count),
            I::I32Const(self.width(ty)? as i32),
            I::I32Mul,
            I::Call(ROOTS_SET),
            I::Drop,
        ]);
        Ok(())
    }

    pub fn root_array_constant(&mut self, local: u32, ty: TypeId, bytes: u32) {
        self.extend([
            I::I32Const(local as i32),
            I::LocalGet(local),
            I::I32Const(ARRAYS as i32),
            I::I32Const(ty.index() as i32),
            I::I32Const(bytes as i32),
            I::Call(ROOTS_SET),
            I::Drop,
        ]);
    }

    fn root_arguments(&self) -> Vec<TypeId> {
        if self.parameter_count != 3
            || !self.key.callable
            || !matches!(self.key.special, Special::Normal | Special::Configured)
        {
            return vec![];
        }
        let signature = self.ty(self.key.node).expect("sealed root signature");
        let mut arguments = self.mir.types[signature.index()].arguments.clone();
        arguments.pop().expect("sealed root result");
        arguments
    }

    pub fn root_prologue(&self, function: &mut ObjectFunction, functions: u32, locals: u32) {
        let arguments = self.root_arguments();
        let mut code = vec![
            I::I32Const((locals + arguments.len() as u32) as i32),
            I::Call(ROOTS_ENTER),
            I::Drop,
        ];
        let mut parameter = |slot: u32, role: u32, ty: u32| {
            code.extend([
                I::I32Const(slot as i32),
                I::LocalGet(slot),
                I::I32Const(role as i32),
                I::I32Const(ty as i32),
                I::I32Const(0),
                I::Call(ROOTS_SET),
                I::Drop,
            ]);
        };
        match self.key.special {
            Special::Equal(ty) => {
                parameter(0, ROOT_VALUE, ty.index() as u32);
                parameter(1, ROOT_VALUE, ty.index() as u32);
            }
            Special::Json(ty) => parameter(1, ROOT_VALUE, ty.index() as u32),
            Special::Parse(_) => {
                parameter(
                    0,
                    ROOT_PARSE_CONTEXT,
                    self.string_type().expect("String context").index() as u32,
                );
                parameter(
                    1,
                    ROOT_VALUE,
                    self.string_type().expect("String root").index() as u32,
                );
            }
            Special::Encode(source, _) => parameter(1, ROOT_VALUE, source.index() as u32),
            Special::Decode(source, _) | Special::DecodeVariant(source, _, _) => {
                parameter(
                    0,
                    ROOT_CODEC_CONTEXT,
                    self.string_type().expect("String context").index() as u32,
                );
                parameter(1, ROOT_VALUE, source.index() as u32);
            }
            _ if self.parameter_count == 2 => {
                parameter(0, ROOT_PARSE_PLAN, 0);
                parameter(1, ROOT_STORAGE, 0);
            }
            Special::Normal | Special::Configured if self.key.callable => {
                let constructor = self.key.special == Special::Normal
                    && crate::natives::identity(self.mir, self.key.node).is_none()
                    && !matches!(
                        self.mir.hir[self.key.node.index()].kind,
                        HirKind::Closure | HirKind::Interpreter
                    );
                parameter(
                    0,
                    if constructor {
                        ROOT_VALUE
                    } else {
                        ENVIRONMENTS
                    },
                    self.ty(self.key.node).expect("function root").index() as u32,
                );
                parameter(1, ROOT_STORAGE, 0);
            }
            _ => {}
        }
        if self.parameter_count == 3 {
            parameter(2, ROOT_STORAGE, 0);
        }
        for (index, ty) in arguments.iter().enumerate() {
            code.extend([
                I::I32Const((locals + index as u32) as i32),
                I::LocalGet(1),
                I::I32Load(memory(index as u64 * 4, 2)),
                I::I32Const(ROOT_VALUE as i32),
                I::I32Const(ty.index() as i32),
                I::I32Const(0),
                I::Call(ROOTS_SET),
                I::Drop,
            ]);
        }
        if self.parameter_count == 3
            && self.key.callable
            && matches!(self.key.special, Special::Normal | Special::Configured)
        {
            code.extend([I::Call(ROOTS_SAFEPOINT), I::Drop]);
        }
        for instruction in code {
            function.linked_instruction(&instruction, functions);
        }
    }
}
