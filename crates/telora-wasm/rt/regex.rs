//! Fixed regex operations; capture-to-type binding belongs to generated code.
use crate::{
    abi::*,
    tables::{telora_table_get, telora_table_push},
    values::word,
};
use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
};
use regex_automata::{
    PatternID,
    nfa::thompson::pikevm::PikeVM,
};

struct Compiled {
    pattern: String,
    engine: PikeVM,
    required: alloc::collections::BTreeSet<String>,
}

fn compile(pattern: &str) -> Result<Compiled, String> {
    let hir = regex_syntax::Parser::new()
        .parse(pattern)
        .map_err(|error| format!("invalid regular expression: {error}"))?;
    let engine = PikeVM::builder()
        .thompson(
            regex_automata::nfa::thompson::Config::new().nfa_size_limit(Some(10 * 1024 * 1024)),
        )
        .build(pattern)
        .map_err(|error| format!("invalid regular expression: {error}"))?;
    for (index, name) in engine
        .get_nfa()
        .group_info()
        .pattern_names(PatternID::ZERO)
        .enumerate()
        .skip(1)
    {
        if name.is_none() {
            return Err(format!("capture group {index} must have a name"));
        }
    }
    Ok(Compiled {
        pattern: pattern.to_string(),
        engine,
        required: crate::regex_contract::required(&hir),
    })
}

pub(crate) unsafe fn pattern(pointer: u32) -> &'static str {
    unsafe { &(*(pointer as *const Compiled)).pattern }
}

pub(crate) unsafe fn release(pointer: u32) {
    unsafe { drop(Box::from_raw(pointer as *mut Compiled)); }
}

pub(crate) fn restore(pattern: &str) -> crate::tables::Slot {
    let compiled = compile(pattern).expect("previously validated regex");
    crate::tables::Slot { payload: Box::into_raw(Box::new(compiled)) as u32,
        bytes: core::mem::size_of::<Compiled>() as u32 }
}

unsafe fn get(id: u32) -> *mut Compiled {
    unsafe { word(telora_table_get(table_address(REGEXES), id), 0) as *mut Compiled }
}

/// Operation 0 returns {HeapId, error span}; 1 matches, 2 compares patterns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_regex(operation: u32, a: u32, b: u32) -> u32 {
    unsafe {
        match operation {
            0 => {
                let packet = crate::telora_alloc(8) as *mut u32;
                let (id, error) = match compile(crate::text::text(a)) {
                    Ok(compiled) => {
                        let pointer = Box::into_raw(Box::new(compiled)) as u32;
                        (
                            telora_table_push(
                                table_address(REGEXES),
                                pointer,
                                core::mem::size_of::<Compiled>() as u32,
                            ),
                            0,
                        )
                    }
                    Err(message) => (0, crate::format::render(format_args!("{message}"))),
                };
                packet.write(id);
                packet.add(1).write(error);
                packet as u32
            }
            1 => {
                let compiled = &mut *get(a);
                compiled
                    .engine
                    .is_match(&mut compiled.engine.create_cache(), crate::text::text(b)) as u32
            }
            2 => ((*get(a)).pattern == (*get(b)).pattern) as u32,
            3 => {
                let compiled = &*get(a);
                let names = compiled
                    .engine
                    .get_nfa()
                    .group_info()
                    .pattern_names(PatternID::ZERO)
                    .skip(1)
                    .flatten()
                    .map(str::to_string)
                    .collect();
                match crate::regex_contract::validate(&names, &compiled.required, b) {
                    Ok(()) => 0,
                    Err(message) => crate::format::render(format_args!("{message}")),
                }
            }
            4 => {
                // Packet: input String, field count, then field-name Strings.
                // Result: matched flag, then {relative byte offset, length, present}.
                // Parsing a preceding field may grow the content arena.
                let compiled = &mut *get(a);
                let input = crate::text::text(word(b, 0));
                let count = word(b, 4);
                let mut captures = compiled.engine.create_captures();
                compiled
                    .engine
                    .captures(&mut compiled.engine.create_cache(), input, &mut captures);
                let output = crate::telora_alloc(4 + count * 12);
                (output as *mut u32).write(captures.is_match() as u32);
                for index in 0..count {
                    let name = crate::text::text(word(b, 8 + u64::from(index) * 4));
                    let capture = captures.get_group_by_name(name);
                    let row = (output + 4 + index * 12) as *mut u32;
                    match capture {
                        Some(span) => {
                            row.write(span.start as u32);
                            row.add(1).write((span.end - span.start) as u32);
                            row.add(2).write(1);
                        }
                        None => {
                            row.write(0);
                            row.add(1).write(0);
                            row.add(2).write(0);
                        }
                    }
                }
                output
            }
            _ => core::arch::wasm32::unreachable(),
        }
    }
}
