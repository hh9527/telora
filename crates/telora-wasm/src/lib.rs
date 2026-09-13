//! Independent Wasm backend. No interpreter or native-backend fallback.
pub mod abi;
mod aggregates;
pub mod artifact;
mod checks;
mod codegen;
mod data_input;
pub mod diagnostic_output;
mod diagnostics;
mod dictionaries;
mod emit;
mod entry;
mod enums;
mod functions;
mod input;
mod input_heap;
mod input_types;
mod natives;
mod output;
mod patterns;
mod plan;
mod properties;
mod runtime;
mod scalars;
pub mod session;
mod strings;
mod tables;

pub use codegen::compile_executable;

#[cfg(test)]
mod tests;
