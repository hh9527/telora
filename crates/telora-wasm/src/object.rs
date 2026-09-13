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
