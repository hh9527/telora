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
    nfa::thompson::pikevm::{Cache, PikeVM},
};

struct Compiled {
    pattern: String,
    engine: PikeVM,
    cache: Cache,
}

fn compile(pattern: &str) -> Result<Compiled, String> {
    regex_syntax::Parser::new()
        .parse(pattern)
        .map_err(|error| format!("invalid regular expression: {error}"))?;
    let engine = PikeVM::builder()
        .thompson(regex_automata::nfa::thompson::Config::new().nfa_size_limit(Some(10 * 1024 * 1024)))
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
    let cache = engine.create_cache();
    Ok(Compiled {
        pattern: pattern.to_string(),
        engine,
        cache,
    })
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
                    .is_match(&mut compiled.cache, crate::text::text(b)) as u32
            }
            2 => ((*get(a)).pattern == (*get(b)).pattern) as u32,
            _ => core::arch::wasm32::unreachable(),
        }
    }
}
