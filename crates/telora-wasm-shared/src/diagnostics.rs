//! Stable runtime failure messages shared by Guest rendering and Host tooling.
pub fn error_message(code: u32) -> &'static str {
    match code {
        1 => "integer arithmetic overflowed",
        2 => "integer division by zero",
        3 => "initialization dependency cycle (cyclic demand)",
        4 => "OutOfRange: array index out of bounds",
        5 => "dictionary key is absent",
        6 => "property type does not support this decorator target",
        7 => "no match arm accepted the value",
        8 => "data module has not been injected before initialization",
        10 => "function called before its declaration was initialized",
        11 => "cannot copy an uninitialized function",
        _ => "Wasm execution failed",
    }
}
