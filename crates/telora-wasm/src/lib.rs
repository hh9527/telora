//! Independent Wasm backend. No interpreter or native-backend fallback.
pub mod abi;
mod aggregates;
mod array_build;
mod array_ops;
pub mod artifact;
mod capture;
mod capture_values;
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
mod link;
mod natives;
pub mod object;
mod output;
mod patterns;
mod plan;
mod properties;
#[path = "../rt/abi.rs"]
mod runtime_abi;
mod scalars;
pub mod session;

pub use codegen::compile_executable;

#[cfg(test)]
mod tests;
