//! Shared byte-content storage. The production word arena lives in Wasm RT.

pub mod content;
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Overflow,
    Bounds,
    Phase,
}
