//! Relocatable instruction encoding for the statically linked backend.
//! Function indices and function pointers are different relocation kinds.
use wasm_encoder::{CodeSection, CustomSection, Encode, Function, Instruction};

#[derive(Clone, Copy)]
struct Relocation {
    kind: u8,
    offset: u32,
    symbol: u32,
}

pub struct ObjectFunction {
    function: Function,
    relocations: Vec<Relocation>,
}

impl ObjectFunction {
    pub fn new(function: Function) -> Self {
        Self {
            function,
            relocations: Vec::new(),
        }
    }

    pub fn instruction(&mut self, instruction: &Instruction<'_>) -> &mut Self {
        self.function.instruction(instruction);
        self
    }

    /// Call the function identified by a linking symbol, not a final index.
    pub fn call(&mut self, symbol: u32) -> &mut Self {
        self.reference(0x10, 0, symbol)
    }

    /// Push a linker-assigned indirect-function-table slot.
    pub fn function_pointer(&mut self, symbol: u32) -> &mut Self {
        self.reference(0x41, 1, symbol)
    }

    pub fn linked_instruction(
        &mut self,
        instruction: &Instruction<'_>,
        functions: u32,
    ) -> &mut Self {
        match instruction {
            Instruction::Call(index) => self.call(*index),
            Instruction::GlobalGet(index) => self.reference(0x23, 7, functions + index),
            Instruction::GlobalSet(index) => self.reference(0x24, 7, functions + index),
            Instruction::CallIndirect {
                type_index,
                table_index,
            } => {
                assert_eq!(*table_index, 0);
                self.reference(0x11, 6, *type_index);
                self.relocations.push(Relocation {
                    kind: 20,
                    offset: self.function.byte_len() as u32,
                    symbol: functions + 2,
                });
                self.function.raw([0x80, 0x80, 0x80, 0x80, 0]);
                self
            }
            _ => self.instruction(instruction),
        }
    }

    pub(crate) fn relocate(function: &Function, functions: u32) -> Result<Self, String> {
        let mut encoded = Vec::new();
        function.encode(&mut encoded);
        let mut size = wasmparser::BinaryReader::new(&encoded, 0);
        size.read_var_u32().map_err(|e| e.to_string())?;
        let bytes = &encoded[size.original_position() as usize..];
        let body = wasmparser::FunctionBody::new(wasmparser::BinaryReader::new(bytes, 0));
        let mut locals = Vec::new();
        for local in body.get_locals_reader().map_err(|e| e.to_string())? {
            let (count, ty) = local.map_err(|e| e.to_string())?;
            let ty = match ty {
                wasmparser::ValType::I32 => wasm_encoder::ValType::I32,
                wasmparser::ValType::I64 => wasm_encoder::ValType::I64,
                wasmparser::ValType::F64 => wasm_encoder::ValType::F64,
                _ => return Err("Wasm object: unsupported local type".into()),
            };
            locals.push((count, ty));
        }
        let mut output = Self::new(Function::new(locals));
        let mut reader = body.get_operators_reader().map_err(|e| e.to_string())?;
        while !reader.eof() {
            let start = reader.original_position();
            let operator = reader.read().map_err(|e| e.to_string())?;
            use wasmparser::Operator as O;
            match operator {
                O::Call { function_index } => {
                    output.call(function_index);
                }
                O::GlobalGet { global_index } => {
                    output.reference(0x23, 7, functions + global_index);
                }
                O::GlobalSet { global_index } => {
                    output.reference(0x24, 7, functions + global_index);
                }
                O::CallIndirect {
                    type_index,
                    table_index,
                } => {
                    output.linked_instruction(
                        &Instruction::CallIndirect {
                            type_index,
                            table_index,
                        },
                        functions,
                    );
                }
                _ => {
                    output.function.raw(
                        bytes[start as usize..reader.original_position() as usize]
                            .iter()
                            .copied(),
                    );
                }
            }
        }
        Ok(output)
    }

    fn reference(&mut self, opcode: u8, kind: u8, symbol: u32) -> &mut Self {
        let offset = self.function.byte_len() as u32 + 1;
        self.function.raw([opcode, 0x80, 0x80, 0x80, 0x80, 0x00]);
        self.relocations.push(Relocation {
            kind,
            offset,
            symbol,
        });
        self
    }
}

#[derive(Default)]
pub struct ObjectCode {
    functions: Vec<ObjectFunction>,
}

impl ObjectCode {
    pub fn function(&mut self, function: ObjectFunction) {
        self.functions.push(function);
    }

    /// Offsets are relative to the code payload, including its count and each
    /// body's size prefix. Neither prefix has a fixed LEB width.
    pub fn finish(self, code_section_index: u32) -> (CodeSection, CustomSection<'static>) {
        let mut code = CodeSection::new();
        let mut count = Vec::new();
        (self.functions.len() as u32).encode(&mut count);
        let mut offset = count.len() as u32;
        let mut relocations = Vec::new();
        for function in self.functions {
            let mut body = Vec::new();
            function.function.encode(&mut body);
            let prefix = body.len() - function.function.byte_len();
            for mut relocation in function.relocations {
                relocation.offset += offset + prefix as u32;
                relocations.push(relocation);
            }
            offset += body.len() as u32;
            code.function(&function.function);
        }
        let mut data = Vec::new();
        code_section_index.encode(&mut data);
        (relocations.len() as u32).encode(&mut data);
        for relocation in relocations {
            data.push(relocation.kind);
            relocation.offset.encode(&mut data);
            relocation.symbol.encode(&mut data);
        }
        (
            code,
            CustomSection {
                name: "reloc.CODE".into(),
                data: data.into(),
            },
        )
    }
}
