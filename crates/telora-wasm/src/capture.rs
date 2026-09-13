//! Type-bound diagnostic scope glue. RT sees no template parameters.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::TypeConstructor as T;
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub fn capture_diagnostics(&mut self) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        if args.len() != 7 {
            return Err("Wasm: diagnostic scope arity mismatch".into());
        }
        let callback = self.mir.types[args[0].index()].clone();
        let output = self.mir.types[args[6].index()].clone();
        if callback.constructor != T::Function
            || callback.arguments.len() != 2
            || callback.arguments[0] != args[1]
            || output.constructor != T::Result
            || output.arguments.len() != 2
        {
            return Err("Wasm: diagnostic scope signature mismatch".into());
        }
        let success = output.arguments[0];
        let reports_ty = output.arguments[1];
        let reports = &self.mir.types[reports_ty.index()];
        if reports.constructor != T::Array
            || reports.arguments.len() != 1
            || self.mir.types[success.index()].constructor != T::Tuple
            || self.mir.types[success.index()].arguments != [callback.arguments[1], reports_ty]
        {
            return Err("Wasm: diagnostic scope result mismatch".into());
        }
        let diagnostic = reports.arguments[0];
        let severity = self.diagnostic_field_type(diagnostic, "severity")?;
        let labels = self.diagnostic_field_type(diagnostic, "labels")?;
        if self.mir.types[labels.index()].constructor != T::Array {
            return Err("Wasm: diagnostic labels are not Array".into());
        }
        let label = self.mir.types[labels.index()].arguments[0];
        let range = self.diagnostic_field_type(label, "location")?;
        for (&witness, expected) in args[2..6].iter().zip([diagnostic, severity, label, range]) {
            let ty = &self.mir.types[witness.index()];
            if ty.constructor != T::TypeOf || ty.arguments != [expected] {
                return Err("Wasm: diagnostic witness differs from sealed result".into());
            }
        }
        let start = self.local(ValType::I32);
        let phase = self.local(ValType::I32);
        let error = self.local(ValType::I32);
        self.extend([
            I::I32Const(table_address(DIAGNOSTICS) as i32),
            I::I32Load(memory(4, 2)),
            I::LocalSet(start),
            I::GlobalGet(PHASE_GLOBAL),
            I::LocalSet(phase),
            I::GlobalGet(ERROR_GLOBAL),
            I::LocalSet(error),
        ]);
        let callable = self.parameter(0);
        let argument = self.parameter(1);
        let arguments = self.alloc(4);
        self.extend([
            I::LocalGet(arguments),
            I::LocalGet(argument),
            I::I32Store(memory(0, 2)),
        ]);
        let returned = self.local(ValType::I32);
        // Deliberately no checked(): zero is the captured language failure.
        // Engine traps still unwind the engine and are not converted to Err.
        self.extend([
            I::LocalGet(callable),
            I::LocalGet(arguments),
            I::Call(INVOKE),
            I::LocalSet(returned),
        ]);
        let count = self.local(ValType::I32);
        self.extend([
            I::I32Const(table_address(DIAGNOSTICS) as i32),
            I::I32Load(memory(4, 2)),
            I::LocalGet(start),
            I::I32Sub,
            I::LocalSet(count),
        ]);
        let width = self.width(diagnostic)?;
        let data = self.array_storage(count, width);
        let index = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let packet = self.local(ValType::I32);
        self.extend([
            I::I32Const(table_address(DIAGNOSTICS) as i32),
            I::LocalGet(start),
            I::LocalGet(index),
            I::I32Add,
            I::Call(TABLE_GET),
            I::I32Load(memory(0, 2)),
            I::LocalSet(packet),
        ]);
        let report = self.diagnostic_value(diagnostic, packet)?;
        let destination = self.array_item(data, index, width);
        self.copy(destination, 0, report, width);
        self.extend([
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
        ]);
        let reports = self.array_result(reports_ty, data, count, width)?;
        self.extend([
            I::I32Const(table_address(DIAGNOSTICS) as i32),
            I::LocalGet(start),
            I::I32Store(memory(4, 2)),
            I::LocalGet(phase),
            I::GlobalSet(PHASE_GLOBAL),
            I::LocalGet(error),
            I::GlobalSet(ERROR_GLOBAL),
        ]);
        let result = self.local(ValType::I32);
        self.extend([I::LocalGet(returned), I::I32Eqz, I::If(BlockType::Empty)]);
        let failure = self.enum_value(node, args[6], 0, Some(reports))?;
        self.extend([I::LocalGet(failure), I::LocalSet(result), I::Else]);
        if self.mir.types[callback.arguments[1].index()].constructor == T::Never {
            self.emit(I::Unreachable);
        } else {
            let payload = self.packed_tuple(success, &[returned, reports])?;
            let value = self.enum_value(node, args[6], 1, Some(payload))?;
            self.extend([I::LocalGet(value), I::LocalSet(result)]);
        }
        self.emit(I::End);
        Ok(result)
    }
}
