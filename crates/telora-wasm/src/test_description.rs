//! Test constructors create immutable descriptions; check never invokes them.
//! TestTable slots contain {operation:u32, count:u32, input_value_offsets:[u32]}.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::TypeConstructor as T;
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub fn test_native(&mut self, name: &str) -> Result<u32, String> {
        let operation = [
            "should_ok",
            "should_fail",
            "should_fail_with",
            "with_fixtures",
        ]
        .iter()
        .position(|candidate| *candidate == name)
        .ok_or("Wasm: unknown test constructor")?;
        let args = self.mir.types[self.ty(self.key.node)?.index()]
            .arguments
            .clone();
        let arity = if operation < 2 { 1 } else { 2 };
        if args.len() != arity + 1 {
            return Err("Wasm: test constructor arity mismatch".into());
        }
        let output = args[arity];
        if !matches!(self.mir.types[output.index()].constructor, T::Native(id) if (id.module,id.slot)==(33,0))
        {
            return Err("Wasm: test constructor requires sealed Test identity".into());
        }
        let callback = &self.mir.types[args[usize::from(operation == 3)].index()];
        if callback.constructor != T::Function
            || callback.arguments.len() != if operation == 3 { 2 } else { 1 }
        {
            return Err("Wasm: test callback signature mismatch".into());
        }
        if operation == 2 && self.mir.types[args[1].index()].constructor != T::String {
            return Err("Wasm: test expectation requires String".into());
        }
        if operation == 3 {
            let fixtures = &self.mir.types[args[0].index()];
            if fixtures.constructor != T::Array
                || fixtures.arguments.len() != 1
                || self.mir.types[fixtures.arguments[0].index()].constructor != T::String
                || callback.arguments[1] != output
                || crate::artifact::exported_type(self.mir, 23, "Value")
                    != Some(callback.arguments[0].index() as u32)
            {
                return Err(
                    "Wasm: test fixtures require Array(String) and Fn(Value) -> Test".into(),
                );
            }
        }
        let inputs: Vec<_> = (0..arity)
            .map(|index| self.parameter(index as u32))
            .collect();
        if operation == 2 {
            self.extend([
                I::I32Const(0),
                I::LocalGet(inputs[1]),
                I::I32Const(0),
                I::Call(TEXT_QUERY),
                I::I32Eqz,
                I::If(BlockType::Empty),
            ]);
            let message = self.text_as(
                self.key.node,
                args[1],
                b"should_fail_with requires a nonempty expectation",
            )?;
            let one = self.local(ValType::I32);
            self.extend([I::I32Const(1), I::LocalSet(one)]);
            self.report(self.key.node, message, inputs[1], one, false);
            self.emit(I::End);
        }
        let bytes = 8 + arity as u32 * 8;
        let description = self.alloc(bytes);
        self.store32(description, 0, operation as u32);
        self.store32(description, 4, arity as u32);
        for (index, input) in inputs.into_iter().enumerate() {
            self.extend([
                I::LocalGet(description),
                I::LocalGet(input),
                I::I32Store(memory(8 + index as u64 * 4, 2)),
            ]);
            self.store32(
                description,
                8 + (arity + index) as u64 * 4,
                args[index].index() as u32,
            );
        }
        let id = self.table_push(TESTS, description, bytes);
        let result = self.value_as(self.key.node, output, SCALAR_BYTES)?;
        self.extend([
            I::LocalGet(result),
            I::LocalGet(id),
            I::I64ExtendI32U,
            I::I64Store(memory(DATA, 3)),
        ]);
        Ok(result)
    }
}
