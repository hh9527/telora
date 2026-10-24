//! Encode finalized instruction operands with minimal LEB widths.
use wasm_encoder::{
    CodeSection,
    reencode::{Reencode, RoundtripReencoder},
};

pub(super) fn code(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let reader = wasmparser::CodeSectionReader::new(wasmparser::BinaryReader::new(bytes, 0))
        .map_err(|error| format!("Wasm: invalid linked code: {error}"))?;
    let mut code = CodeSection::new();
    RoundtripReencoder
        .parse_code_section(&mut code, reader)
        .map_err(|error| format!("Wasm: cannot compact linked code: {error}"))?;
    Ok(crate::template::payload(&code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_encoder::{Function, Instruction as I};

    fn padded(value: u32) -> [u8; 5] {
        let mut value = value;
        std::array::from_fn(|index| {
            let byte = (value & 127) as u8 | if index < 4 { 128 } else { 0 };
            value >>= 7;
            byte
        })
    }

    #[test]
    fn unsigned_relocations_use_the_width_of_the_final_index() {
        let mut original = Function::new([]);
        let mut expected = Function::new([]);
        for index in [0, 127, 128, 16_383, 16_384, 2_097_151, 2_097_152, u32::MAX] {
            original.raw([0x10].into_iter().chain(padded(index)));
            expected.instruction(&I::Call(index));
            original.raw([0x23].into_iter().chain(padded(index)));
            expected.instruction(&I::GlobalGet(index));
            original.raw([0x11].into_iter().chain(padded(index)).chain(padded(0)));
            expected.instruction(&I::CallIndirect {
                type_index: index,
                table_index: 0,
            });
        }
        original.instruction(&I::End);
        expected.instruction(&I::End);
        let mut input = CodeSection::new();
        input.function(&original);
        let mut output = CodeSection::new();
        output.function(&expected);
        let compact = code(&crate::template::payload(&input)).unwrap();
        assert_eq!(compact, crate::template::payload(&output));
        assert!(compact.len() < crate::template::payload(&input).len());
    }

    #[test]
    fn signed_addresses_and_float_bits_survive_compaction() {
        let mut original = Function::new([(1, wasm_encoder::ValType::I64)]);
        let mut expected = original.clone();
        for value in [
            i32::MIN,
            -8193,
            -8192,
            -65,
            -64,
            -1,
            0,
            63,
            64,
            8191,
            8192,
            i32::MAX,
        ] {
            let mut bytes = padded(value as u32);
            if value < 0 {
                bytes[4] |= 0x70;
            }
            original.raw([0x41].into_iter().chain(bytes));
            expected.instruction(&I::I32Const(value));
        }
        for function in [&mut original, &mut expected] {
            function.instruction(&I::F32Const(wasm_encoder::Ieee32::new(0xffc0_1234)));
            function.instruction(&I::F64Const(wasm_encoder::Ieee64::new(
                0x7ff8_0000_0000_4321,
            )));
            function.instruction(&I::End);
        }
        let mut input = CodeSection::new();
        input.function(&original);
        let mut output = CodeSection::new();
        output.function(&expected);
        let compact = code(&crate::template::payload(&input)).unwrap();
        assert_eq!(compact, crate::template::payload(&output));
        assert_eq!(code(&compact).unwrap(), compact);
    }
}
