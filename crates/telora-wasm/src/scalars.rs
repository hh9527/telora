use crate::{abi::*, emit::Emitter, plan::child};
use telora_core::{
    ast::{BinaryOperator as B, UnaryOperator as U},
    mir::{HirId, Role, TypeConstructor as T},
};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub fn unary(&mut self, node: HirId, op: U) -> Result<u32, String> {
        let operand_node = child(self.mir, node, Role::Operand)?;
        let ty = self.ty(operand_node)?;
        let operand = self.expression(operand_node)?;
        let bits = self.local(ValType::I64);
        match (&self.mir.types[ty.index()].constructor, op) {
            (T::Int, U::Negate) => {
                self.bits(operand);
                self.extend([I::I64Const(i64::MIN), I::I64Eq]);
                self.fail_if(node, ERROR_OVERFLOW);
                self.emit(I::I64Const(0));
                self.bits(operand);
                self.emit(I::I64Sub);
            }
            (T::Int, U::BitNot | U::Not) => {
                self.bits(operand);
                self.extend([I::I64Const(-1), I::I64Xor]);
            }
            (T::Bool, U::LogicalNot | U::Not) => {
                self.bits(operand);
                self.extend([I::I64Eqz, I::I64ExtendI32U]);
            }
            (T::Float, U::Negate) => {
                self.bits(operand);
                self.extend([I::I64Const(i64::MIN), I::I64Xor]);
            }
            _ => return Err("Wasm: unsupported unary type/operator".into()),
        }
        self.emit(I::LocalSet(bits));
        self.scalar_bits(node, bits)
    }
    fn scalar_bits(&mut self, node: HirId, bits: u32) -> Result<u32, String> {
        let result = self.value(node, SCALAR_BYTES)?;
        self.extend([
            I::LocalGet(result),
            I::LocalGet(bits),
            I::I64Store(memory(DATA, 3)),
        ]);
        Ok(result)
    }
    pub fn binary(&mut self, node: HirId, op: B) -> Result<u32, String> {
        let lhs = child(self.mir, node, Role::Left)?;
        let rhs = child(self.mir, node, Role::Right)?;
        let ty = self.ty(lhs)?;
        if ty != self.ty(rhs)? {
            return Err("Wasm: binary operands need identical sealed types".into());
        }
        let kind = &self.mir.types[ty.index()].constructor;
        if *kind == T::String {
            let left = self.expression(lhs)?;
            let right = self.expression(rhs)?;
            let comparison = match op {
                B::Equal => I::I32Eq,
                B::NotEqual => I::I32Ne,
                B::LessThan => I::I32LtS,
                B::LessThanOrEqual => I::I32LeS,
                B::GreaterThan => I::I32GtS,
                B::GreaterThanOrEqual => I::I32GeS,
                _ => return Err("Wasm: unsupported String operator".into()),
            };
            let bits = self.local(ValType::I64);
            self.extend([
                I::LocalGet(left),
                I::LocalGet(right),
                I::Call(STRING_COMPARE),
                I::I32Const(0),
                comparison,
                I::I64ExtendI32U,
                I::LocalSet(bits),
            ]);
            return self.scalar_bits(node, bits);
        }
        if !matches!(kind, T::Int | T::Float | T::Bool) {
            return Err("Wasm: non-scalar binary operation is not implemented yet".into());
        }
        let left = self.expression(lhs)?;
        let result = self.local(ValType::I64);
        if matches!(op, B::And | B::Or) {
            if *kind != T::Bool {
                return Err("Wasm: logical operands require Bool".into());
            }
            self.bits(left);
            self.emit(I::LocalSet(result));
            self.extend([I::LocalGet(result), I::I64Eqz]);
            if op == B::And {
                self.emit(I::I32Eqz);
            }
            self.emit(I::If(BlockType::Empty));
            let right = self.expression(rhs)?;
            self.bits(right);
            self.extend([I::LocalSet(result), I::End]);
            return self.scalar_bits(node, result);
        }
        let right = self.expression(rhs)?;
        let a = self.local(ValType::I64);
        let b = self.local(ValType::I64);
        self.bits(left);
        self.emit(I::LocalSet(a));
        self.bits(right);
        self.emit(I::LocalSet(b));
        if *kind == T::Float {
            return self.float_binary(node, op, a, b);
        }
        if matches!(op, B::Divide | B::Remainder) {
            self.extend([I::LocalGet(b), I::I64Eqz]);
            self.fail_if(node, ERROR_DIVISION);
            if op == B::Divide {
                self.extend([
                    I::LocalGet(a),
                    I::I64Const(i64::MIN),
                    I::I64Eq,
                    I::LocalGet(b),
                    I::I64Const(-1),
                    I::I64Eq,
                    I::I32And,
                ]);
                self.fail_if(node, ERROR_OVERFLOW);
            }
        }
        let comparison = matches!(
            op,
            B::Equal
                | B::NotEqual
                | B::LessThan
                | B::LessThanOrEqual
                | B::GreaterThan
                | B::GreaterThanOrEqual
        );
        let instruction = match op {
            B::Add => I::I64Add,
            B::Subtract => I::I64Sub,
            B::Multiply => I::I64Mul,
            B::Divide => I::I64DivS,
            B::Remainder => I::I64RemS,
            B::BitAnd => I::I64And,
            B::BitOr => I::I64Or,
            B::BitXor => I::I64Xor,
            B::Equal => I::I64Eq,
            B::NotEqual => I::I64Ne,
            B::LessThan => I::I64LtS,
            B::LessThanOrEqual => I::I64LeS,
            B::GreaterThan => I::I64GtS,
            B::GreaterThanOrEqual => I::I64GeS,
            _ => return Err("Wasm: unsupported scalar operator".into()),
        };
        self.extend([I::LocalGet(a), I::LocalGet(b), instruction]);
        if comparison {
            self.emit(I::I64ExtendI32U);
        }
        self.emit(I::LocalSet(result));
        match op {
            B::Add | B::Subtract => {
                self.extend([I::LocalGet(a), I::LocalGet(result), I::I64Xor]);
                if op == B::Add {
                    self.extend([I::LocalGet(b), I::LocalGet(result), I::I64Xor]);
                } else {
                    self.extend([I::LocalGet(a), I::LocalGet(b), I::I64Xor]);
                }
                self.extend([I::I64And, I::I64Const(0), I::I64LtS]);
                self.fail_if(node, ERROR_OVERFLOW);
            }
            B::Multiply => {
                self.extend([
                    I::LocalGet(a),
                    I::I64Eqz,
                    I::I32Eqz,
                    I::If(BlockType::Empty),
                    I::LocalGet(result),
                    I::I64Const(i64::MIN),
                    I::I64Eq,
                    I::LocalGet(a),
                    I::I64Const(-1),
                    I::I64Eq,
                    I::I32And,
                ]);
                self.fail_if(node, ERROR_OVERFLOW);
                self.extend([
                    I::LocalGet(result),
                    I::LocalGet(a),
                    I::I64DivS,
                    I::LocalGet(b),
                    I::I64Ne,
                ]);
                self.fail_if(node, ERROR_OVERFLOW);
                self.emit(I::End);
            }
            _ => {}
        }
        self.scalar_bits(node, result)
    }
    fn float_binary(&mut self, node: HirId, op: B, a: u32, b: u32) -> Result<u32, String> {
        let comparison = matches!(
            op,
            B::Equal
                | B::NotEqual
                | B::LessThan
                | B::LessThanOrEqual
                | B::GreaterThan
                | B::GreaterThanOrEqual
        );
        let instruction = match op {
            B::Add => I::F64Add,
            B::Subtract => I::F64Sub,
            B::Multiply => I::F64Mul,
            B::Divide => I::F64Div,
            B::Equal => I::F64Eq,
            B::NotEqual => I::F64Ne,
            B::LessThan => I::F64Lt,
            B::LessThanOrEqual => I::F64Le,
            B::GreaterThan => I::F64Gt,
            B::GreaterThanOrEqual => I::F64Ge,
            _ => return Err("Wasm: unsupported Float operator".into()),
        };
        let bits = self.local(ValType::I64);
        self.extend([
            I::LocalGet(a),
            I::F64ReinterpretI64,
            I::LocalGet(b),
            I::F64ReinterpretI64,
            instruction,
        ]);
        self.emit(if comparison {
            I::I64ExtendI32U
        } else {
            I::I64ReinterpretF64
        });
        self.emit(I::LocalSet(bits));
        self.scalar_bits(node, bits)
    }
}
