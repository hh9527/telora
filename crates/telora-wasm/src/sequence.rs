//! Fused execution of immutable plans. Mutable slots are private to this call.
use crate::{
    abi::*,
    emit::Emitter,
    sequence_plan::{Consumer, Stage},
};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn sequence_compute(&mut self) -> Result<u32, String> {
        let args = self.mir.types[self.ty(self.key.node)?.index()]
            .arguments
            .clone();
        let element = self.sequence_source_contract(args[0], 0)?;
        let result = self.sequence_sink_contract(args[1], element, 0)?;
        if result != args[2] {
            return Err("Wasm: sequence compute result mismatch".into());
        }
        let source = self.parameter(0);
        let sink = self.parameter(1);
        let mut stages = Vec::new();
        let (array, array_ty) = self.sequence_source(source, args[0], &mut stages, 0)?;
        let input = self.mir.types[array_ty.index()].arguments[0];
        let width = self.width(input)?;
        let (base, count) = self.array_parts(array, width);
        let consumer = self.sequence_consumer(sink, args[1], 0)?;
        let index = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        self.consumer_done(&consumer);
        self.emit(I::BrIf(1));
        let mut item = self.array_item(base, index, width);
        let mut guards = 0;
        for stage in &stages {
            let (value, guarded) = self.sequence_transform(stage, item)?;
            item = value;
            guards += usize::from(guarded);
        }
        self.consumer_step(&consumer, item)?;
        for _ in 0..guards {
            self.emit(I::End);
        }
        self.extend([
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
        ]);
        self.consumer_finish(&consumer)
    }

    pub(crate) fn sequence_transform(
        &mut self,
        stage: &Stage,
        item: u32,
    ) -> Result<(u32, bool), String> {
        let result = self.invoke(stage.callback, &[item], stage.output)?;
        match stage.kind.as_str() {
            "Map" => Ok((result, false)),
            "Filter" => {
                self.bits(result);
                self.extend([I::I64Eqz, I::I32Eqz, I::If(BlockType::Empty)]);
                Ok((item, true))
            }
            "FilterMap" => {
                self.extend([
                    I::LocalGet(result),
                    I::I32Load(memory(DATA, 2)),
                    I::If(BlockType::Empty),
                ]);
                Ok((self.enum_payload(stage.output, 1, result)?, true))
            }
            _ => Err("Wasm: unsupported sequence transform".into()),
        }
    }

    pub(crate) fn consumer_done(&mut self, consumer: &Consumer) {
        match consumer {
            Consumer::Both { left, right, .. } => {
                self.consumer_done(left);
                self.consumer_done(right);
                self.emit(I::I32And);
            }
            Consumer::Adapt { next, .. } => self.consumer_done(next),
            Consumer::Leaf { done, .. } => self.emit(I::LocalGet(*done)),
        }
    }

    fn consumer_step(&mut self, consumer: &Consumer, item: u32) -> Result<(), String> {
        self.consumer_done(consumer);
        self.extend([I::I32Eqz, I::If(BlockType::Empty)]);
        match consumer {
            Consumer::Both { left, right, .. } => {
                self.consumer_step(left, item)?;
                self.consumer_step(right, item)?;
            }
            Consumer::Adapt { stage, next } => {
                let (value, guarded) = self.sequence_transform(stage, item)?;
                self.consumer_step(next, value)?;
                if guarded {
                    self.emit(I::End);
                }
            }
            Consumer::Leaf {
                kind,
                input,
                output,
                callback,
                state,
                done,
                count,
                capacity,
            } => match kind.as_str() {
                "Collect" => self.sequence_append(*state, *count, *capacity, *input, item)?,
                "Count" => {
                    self.extend([
                        I::LocalGet(*state),
                        I::LocalGet(*state),
                        I::I64Load(memory(DATA, 3)),
                        I::I64Const(1),
                        I::I64Add,
                        I::I64Store(memory(DATA, 3)),
                    ]);
                }
                "Fold" => {
                    let (callback, result_ty) = callback.unwrap();
                    let result = self.invoke(callback, &[*state, item], result_ty)?;
                    self.extend([I::LocalGet(result), I::LocalSet(*state)]);
                    self.root_value(*state, *output);
                }
                "Any" | "All" | "Find" => {
                    let (callback, result_ty) = callback.unwrap();
                    let result = self.invoke(callback, &[item], result_ty)?;
                    self.bits(result);
                    self.emit(I::I64Eqz);
                    if kind != "All" {
                        self.emit(I::I32Eqz);
                    }
                    self.emit(I::If(BlockType::Empty));
                    self.extend([I::I32Const(1), I::LocalSet(*done)]);
                    if kind == "Find" {
                        self.copy(*state, 0, item, self.width(*input)?);
                        self.root_value(*state, *input);
                    } else {
                        self.store64(*state, DATA, u64::from(kind == "Any"));
                    }
                    self.emit(I::End);
                }
                _ => unreachable!(),
            },
        }
        self.emit(I::End);
        Ok(())
    }

    fn consumer_finish(&mut self, consumer: &Consumer) -> Result<u32, String> {
        match consumer {
            Consumer::Both {
                left,
                right,
                output,
            } => {
                let left = self.consumer_finish(left)?;
                let right = self.consumer_finish(right)?;
                self.packed_tuple(*output, &[left, right])
            }
            Consumer::Adapt { next, .. } => self.consumer_finish(next),
            Consumer::Leaf {
                kind,
                input,
                output,
                state,
                done,
                count,
                capacity,
                ..
            } => {
                if kind == "Collect" {
                    self.array_result_with_capacity(
                        *output,
                        *state,
                        *count,
                        *capacity,
                        self.width(*input)?,
                    )
                } else if kind == "Find" {
                    let result = self.local(ValType::I32);
                    self.extend([I::LocalGet(*done), I::If(BlockType::Empty)]);
                    let found = self.enum_value(self.key.node, *output, 1, Some(*state))?;
                    self.extend([I::LocalGet(found), I::LocalSet(result), I::Else]);
                    let missing = self.enum_value(self.key.node, *output, 0, None)?;
                    self.extend([I::LocalGet(missing), I::LocalSet(result), I::End]);
                    self.root_value(result, *output);
                    Ok(result)
                } else {
                    Ok(*state)
                }
            }
        }
    }

    fn sequence_append(
        &mut self,
        data: u32,
        count: u32,
        capacity: u32,
        input: telora_core::mir::TypeId,
        item: u32,
    ) -> Result<(), String> {
        let width = self.width(input)?;
        self.extend([
            I::LocalGet(count),
            I::LocalGet(capacity),
            I::I32Eq,
            I::If(BlockType::Empty),
        ]);
        self.extend([
            I::LocalGet(capacity),
            I::I32Const((u32::MAX / 2) as i32),
            I::I32GtU,
        ]);
        self.fail_if(self.key.node, ERROR_OVERFLOW);
        self.extend([
            I::LocalGet(capacity),
            I::I32Eqz,
            I::If(BlockType::Result(ValType::I32)),
            I::I32Const(8),
            I::Else,
            I::LocalGet(capacity),
            I::I32Const(2),
            I::I32Mul,
            I::End,
            I::LocalSet(capacity),
        ]);
        let next = self.array_storage(capacity, width);
        self.extend([
            I::LocalGet(next),
            I::LocalGet(data),
            I::LocalGet(count),
            I::I32Const(width as i32),
            I::I32Mul,
            I::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            },
            I::LocalGet(next),
            I::LocalSet(data),
        ]);
        self.root_array_prefix(data, input, count)?;
        self.emit(I::End);
        let destination = self.array_item(data, count, width);
        self.copy(destination, 0, item, width);
        self.extend([
            I::LocalGet(count),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(count),
        ]);
        self.root_array_prefix(data, input, count)?;
        Ok(())
    }
}
