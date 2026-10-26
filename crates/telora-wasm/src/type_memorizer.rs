//! Private, densely type-indexed memo slots. Ordinary typed arrays keep cached
//! results visible to the existing collector and snapshot relocation protocol.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub fn type_memorizer_native(&mut self, name: &str) -> Result<u32, String> {
        let args = self.mir.types[self.ty(self.key.node)?.index()]
            .arguments
            .clone();
        let output = *args
            .last()
            .ok_or("Wasm: missing type memorizer signature")?;
        let array = if name == "new_slots" && args.len() == 1 {
            output
        } else if name == "lookup" && args.len() == 4 {
            args[0]
        } else {
            return Err("Wasm: invalid type memorizer operation".into());
        };
        if self.mir.types[array.index()].constructor != T::Array {
            return Err("Wasm: type memorizer slots must be an Array".into());
        }
        let slot = self.mir.types[array.index()].arguments[0];
        let variants = &self.plan.layouts[slot.index()].variants;
        if variants.len() != 3
            || variants[0].name != "Empty"
            || variants[1].name != "Pending"
            || variants[2].name != "Ready"
        {
            return Err("Wasm: invalid type memorizer slot layout".into());
        }
        let result_ty = self.plan.layouts[variants[2]
            .type_id
            .ok_or("Wasm: missing memo result type")?]
        .id();
        let width = self.width(slot)?;
        if name == "new_slots" {
            return self.type_memorizer_slots(array, slot, width);
        }
        if self.mir.types[args[1].index()].constructor != T::Type
            || self.mir.types[args[2].index()].constructor != T::Function
            || self.mir.types[args[2].index()].arguments != [args[1], output]
            || result_ty != output
        {
            return Err("Wasm: invalid type memorizer lookup signature".into());
        }
        let slots = self.parameter(0);
        let ty = self.parameter(1);
        let calc = self.parameter(2);
        let (base, count) = self.array_parts(slots, width);
        self.bits(ty);
        self.extend([I::LocalGet(count), I::I64ExtendI32U, I::I64GeU]);
        self.fail_if(self.key.node, ERROR_INDEX);
        let id = self.read32(ty, DATA);
        let cell = self.array_item(base, id, width);
        let status = self.read32(cell, DATA);
        self.extend([
            I::LocalGet(status),
            I::I32Const(2),
            I::I32Eq,
            I::If(BlockType::Empty),
        ]);
        let ready = self.enum_payload(slot, 2, cell)?;
        self.extend([I::LocalGet(ready), I::Return, I::End]);
        self.extend([
            I::LocalGet(status),
            I::I32Const(1),
            I::I32Eq,
            I::If(BlockType::Empty),
        ]);
        self.reflection_failure(ty, "type memorizer computation depends on itself")?;
        self.emit(I::End);
        // Publish only a valid nullary discriminant before invoking user code.
        // The captured Array remains a typed root across recursive lookups/GC.
        self.store64(cell, DATA, 1);
        let origin = self.computation_origin(self.key.node)?;
        let arguments = self.argument_array(&[ty], Some(origin));
        let buffer = self.output_alloc(output)?;
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(calc),
            I::LocalGet(arguments),
            I::LocalGet(buffer),
            I::Call(INVOKE),
            I::LocalSet(result),
            I::LocalGet(result),
            I::I32Eqz,
            I::If(BlockType::Empty),
        ]);
        // A captured language failure must not poison the slot as Pending.
        // Engine traps abort execution; they are not converted into memo values.
        self.store64(cell, DATA, 0);
        self.extend([I::I32Const(0), I::Return, I::End]);
        self.root_value(result, output);
        let stored = self.enum_value(self.key.node, slot, 2, Some(result))?;
        self.copy(cell, 0, stored, width);
        Ok(result)
    }

    fn type_memorizer_slots(
        &mut self,
        array: TypeId,
        slot: TypeId,
        width: u32,
    ) -> Result<u32, String> {
        let count = self.local(ValType::I32);
        self.extend([I::I32Const(self.mir.types.len() as i32), I::LocalSet(count)]);
        let data = self.array_storage(count, width);
        // Empty is variant zero. Zero initialization creates valid, pointer-free
        // slots before the collector is allowed to inspect the full array.
        self.extend([
            I::LocalGet(data),
            I::I32Const(0),
            I::LocalGet(count),
            I::I32Const(width as i32),
            I::I32Mul,
            I::MemoryFill(0),
        ]);
        self.root_array_prefix(data, slot, count)?;
        self.array_result(array, data, count, width)
    }
}
