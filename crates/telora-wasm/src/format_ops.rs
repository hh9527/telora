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
        let operation = match name {
            "make_empty" => 0,
            "from_string" => 1,
            "from_int" => 2,
            "from_float" => 3,
            "concat" => 4,
            "render" => 5,
            "write_str" => 6,
            "write_int" => 7,
            "write_float" => 8,
            "encode_json_string" => 9,
            "write_fmt" => 10,
            "encode_json_float" => 11,
            _ => return Err(format!("Wasm: Fmt native not implemented: {name}")),
        };
        let arity = match operation {
            0 => 0,
            4 | 6..=11 => 2,
            _ => 1,
        };
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
            0 => is_format(kind(output)),
            6 | 9 => {
                is_format(kind(args[0])) && kind(args[1]) == &T::String && is_format(kind(output))
            }
            7 => is_format(kind(args[0])) && kind(args[1]) == &T::Int && is_format(kind(output)),
            8 | 11 => {
                is_format(kind(args[0])) && kind(args[1]) == &T::Float && is_format(kind(output))
            }
            10 => args.iter().all(|&ty| is_format(kind(ty))),
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
        if operation == 0 {
            return self.format_empty(node, output);
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
        let second = if arity == 2 {
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
                    I::I32Load(memory(DATA + 8, 2)),
                    I::LocalGet(value),
                    I::I32Load(memory(DATA + 4, 2)),
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
        if operation == 11 {
            self.extend([
                I::LocalGet(second),
                I::I64Load(memory(DATA, 3)),
                I::F64ReinterpretI64,
                I::F64Abs,
                I::F64Const(f64::MAX.into()),
                I::F64Le,
                I::I32Eqz,
                I::If(BlockType::Empty),
            ]);
            let message = self.text_as(
                node,
                self.string_type()?,
                b"JSON cannot encode a non-finite Float",
            )?;
            let count = self.local(ValType::I32);
            self.extend([I::I32Const(1), I::LocalSet(count)]);
            self.report(node, message, second, count, false);
            self.emit(I::End);
        }
        if self.writer_mode {
            let writer = self.region_writer();
            let op = match operation {
                6 => 10,
                7 => 3,
                8 => 12,
                9 => 2,
                11 => 4,
                _ => return Err("invalid formatter specialization".into()),
            };
            self.extend([
                I::I32Const(op),
                I::LocalGet(writer),
                I::LocalGet(second),
                I::Call(JSON_WRITE),
                I::Drop,
            ]);
            return Ok(first);
        }
        // Deferred nodes own plain operands after their caller's frame ends.
        let first = self.escape_plain(first, args[0])?;
        let second = if arity == 2 {
            self.escape_plain(second, args[1])?
        } else {
            second
        };
        let data = self.alloc(20);
        self.store32(data, 0, operation as u32);
        for (offset, value) in [(4, first), (8, second)] {
            self.extend([
                I::LocalGet(data),
                I::LocalGet(value),
                I::I32Store(memory(offset, 2)),
            ]);
        }
        self.store32(data, 12, args[0].index() as u32);
        self.store32(
            data,
            16,
            args.get(1).copied().unwrap_or(args[0]).index() as u32,
        );
        let id = self.table_push(FORMATS, data, 20, None)?;
        let result = self.value_as(node, output, SCALAR_BYTES)?;
        self.extend([
            I::LocalGet(result),
            I::LocalGet(id),
            I::I64ExtendI32U,
            I::I64Store(memory(DATA, 3)),
        ]);
        Ok(result)
    }

    pub(super) fn format_empty(&mut self, node: HirId, output: TypeId) -> Result<u32, String> {
        let data = self.alloc(20);
        for offset in [0, 4, 8, 12, 16] {
            self.store32(data, offset, 0);
        }
        let id = self.table_push(FORMATS, data, 20, None)?;
        let result = self.value_as(node, output, SCALAR_BYTES)?;
        self.extend([
            I::LocalGet(result),
            I::LocalGet(id),
            I::I64ExtendI32U,
            I::I64Store(memory(DATA, 3)),
        ]);
        Ok(result)
    }

    pub(super) fn format_result(
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
        self.store_location(value, loc);
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
        // An unknown/custom Fmt anywhere keeps the entire original path, including
        // recursion failure subjects and Display evaluation/diagnostic order.
        let primitives_only = children.iter().all(|edge| {
            self.effective_ty(edge.node)
                .is_ok_and(|ty| self.mir.types[ty.index()].constructor == T::String)
                || self.primitive_format_input(edge.node).is_some()
        });
        let parts = self.alloc(bytes);
        let mut subject = None;
        for (index, edge) in children.iter().enumerate() {
            if edge.role != Role::Part {
                return Err("Wasm: invalid interpolation edge".into());
            }
            let primitive = primitives_only
                .then(|| self.primitive_format_input(edge.node))
                .flatten();
            let (operation, value) = if let Some((operation, input, callee)) = primitive {
                (
                    operation,
                    self.primitive_format_value(edge.node, input, callee)?,
                )
            } else {
                let kind = &self.mir.types[self.effective_ty(edge.node)?.index()].constructor;
                let operation = if *kind == T::String {
                    1
                } else if is_format(kind) {
                    2
                } else {
                    return Err(
                        "Wasm: interpolation part lacks sealed String/Fmt conversion".into(),
                    );
                };
                if operation == 2 {
                    if let Some(value) = self.format_region_value(edge.node, string)? {
                        self.root_value(value, string);
                        (1, value)
                    } else {
                        (operation, self.expression(edge.node)?)
                    }
                } else {
                    (operation, self.expression(edge.node)?)
                }
            };
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
