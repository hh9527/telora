//! Bind the fixed format RT to concrete MIR types and source locations.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{HirId, Role, TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

fn is_format(kind: &T) -> bool {
    matches!(kind, T::Native(id) if (id.module, id.slot) == (20, 1))
}

impl Emitter<'_> {
    pub fn format_native(&mut self, name: &str) -> Result<u32, String> {
        if name == "prepare" {
            return self.template_prepare();
        }
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        let operation = ["from_string", "from_int", "from_float", "concat", "render"]
            .iter()
            .position(|&n| n == name)
            .ok_or_else(|| format!("Wasm: Fmt native not implemented: {name}"))?
            + 1;
        let arity = if operation == 4 { 2 } else { 1 };
        if args.len() != arity + 1 {
            return Err("Wasm: Fmt signature arity mismatch".into());
        }
        let output = args[arity];
        let kind = |ty: TypeId| &self.mir.types[ty.index()].constructor;
        let array = |ty: TypeId, fmt: bool| {
            let shape = &self.mir.types[ty.index()];
            shape.constructor == T::Array
                && shape.arguments.len() == 1
                && if fmt {
                    is_format(kind(shape.arguments[0]))
                } else {
                    kind(shape.arguments[0]) == &T::String
                }
        };
        let valid = match operation {
            1 => kind(args[0]) == &T::String && is_format(kind(output)),
            2 => kind(args[0]) == &T::Int && is_format(kind(output)),
            3 => kind(args[0]) == &T::Float && is_format(kind(output)),
            4 => array(args[0], false) && array(args[1], true) && is_format(kind(output)),
            5 => is_format(kind(args[0])) && kind(output) == &T::String,
            _ => unreachable!(),
        };
        if !valid {
            return Err("Wasm: Fmt signature types mismatch".into());
        }
        let first = self.parameter(0);
        if operation == 5 {
            let span = self.local(ValType::I32);
            self.extend([
                I::LocalGet(first),
                I::Call(FORMAT_RENDER),
                I::LocalSet(span),
            ]);
            return self.format_result(node, output, span, first);
        }
        let second = if operation == 4 {
            self.parameter(1)
        } else {
            self.local(ValType::I32)
        };
        if operation == 4 {
            let strings = self.local(ValType::I32);
            let items = self.local(ValType::I32);
            for (value, count) in [(first, strings), (second, items)] {
                self.extend([
                    I::LocalGet(value),
                    I::I32Load(memory(24, 2)),
                    I::LocalGet(value),
                    I::I32Load(memory(20, 2)),
                    I::I32Sub,
                    I::LocalSet(count),
                ]);
            }
            self.extend([
                I::LocalGet(strings),
                I::I64ExtendI32U,
                I::LocalGet(items),
                I::I64ExtendI32U,
                I::I64Const(1),
                I::I64Add,
                I::I64Ne,
                I::If(BlockType::Empty),
            ]);
            let span = self.local(ValType::I32);
            self.extend([
                I::LocalGet(strings),
                I::LocalGet(items),
                I::Call(FORMAT_MESSAGE),
                I::LocalSet(span),
            ]);
            let message = self.text_span_value(self.string_type()?, span)?;
            let count = self.local(ValType::I32);
            self.extend([I::I32Const(1), I::LocalSet(count)]);
            self.report(node, message, first, count, false);
            self.emit(I::End);
        }
        // Retain stable immutable value pointers, not copies of their heap data.
        let data = self.alloc(12);
        self.store32(data, 0, operation as u32);
        for (offset, value) in [(4, first), (8, second)] {
            self.extend([
                I::LocalGet(data),
                I::LocalGet(value),
                I::I32Store(memory(offset, 2)),
            ]);
        }
        let id = self.table_push(FORMATS, data, 12);
        let result = self.value_as(node, output, SCALAR_BYTES)?;
        self.extend([
            I::LocalGet(result),
            I::LocalGet(id),
            I::I64ExtendI32U,
            I::I64Store(memory(DATA, 3)),
        ]);
        Ok(result)
    }

    fn format_result(
        &mut self,
        node: HirId,
        string: TypeId,
        span: u32,
        subject: u32,
    ) -> Result<u32, String> {
        self.extend([I::LocalGet(span), I::I32Eqz, I::If(BlockType::Empty)]);
        let message = self.text_as(
            node,
            string,
            b"std/fmt value exceeds the recursive rendering limit",
        )?;
        let count = self.local(ValType::I32);
        self.extend([I::I32Const(1), I::LocalSet(count)]);
        self.report(node, message, subject, count, false);
        self.emit(I::End);
        let value = self.text_span_value(string, span)?;
        let loc = self.mir.hir[node.index()].location;
        let loc_words = self.mir.sources.get(loc.source).compact(loc).0;
        self.store32(value, SOURCE, loc_words[0]);
        self.store32(value, START, loc_words[1]);
        self.store32(value, END, loc_words[2]);
        Ok(value)
    }

    pub fn interpolate(&mut self, node: HirId) -> Result<u32, String> {
        let string = self.ty(node)?;
        if self.mir.types[string.index()].constructor != T::String {
            return Err("Wasm: interpolation result is not sealed String".into());
        }
        let children = self.mir.hir[node.index()].children.clone();
        let bytes = u32::try_from(children.len())
            .ok()
            .and_then(|n| n.checked_mul(8))
            .ok_or("Wasm: interpolation too large")?;
        let parts = self.alloc(bytes);
        let mut subject = None;
        for (index, edge) in children.iter().enumerate() {
            if edge.role != Role::Part {
                return Err("Wasm: invalid interpolation edge".into());
            }
            let kind = &self.mir.types[self.effective_ty(edge.node)?.index()].constructor;
            let operation = if *kind == T::String {
                1
            } else if is_format(kind) {
                2
            } else {
                return Err("Wasm: interpolation part lacks sealed String/Fmt conversion".into());
            };
            let value = self.expression(edge.node)?;
            subject = Some(value);
            self.store32(parts, index as u64 * 8, operation);
            self.extend([
                I::LocalGet(parts),
                I::LocalGet(value),
                I::I32Store(memory(index as u64 * 8 + 4, 2)),
            ]);
        }
        let span = self.local(ValType::I32);
        self.extend([
            I::LocalGet(parts),
            I::I32Const(children.len() as i32),
            I::Call(FORMAT_JOIN),
            I::LocalSet(span),
        ]);
        let subject = match subject {
            Some(value) => value,
            None => self.text(node, b"")?,
        };
        self.format_result(node, string, span, subject)
    }
}
