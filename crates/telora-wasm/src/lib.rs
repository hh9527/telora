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
mod dict_build;
mod dict_literals;
mod dict_ops;
mod dictionaries;
mod emit;
mod entry;
mod enums;
mod equality;
mod equality_plan;
mod functions;
mod input;
mod input_heap;
mod input_types;
mod link;
mod natives;
pub mod object;
mod output;
mod path_ops;
mod patterns;
mod plan;
mod properties;
mod records;
#[path = "../rt/abi.rs"]
mod runtime_abi;
mod scalars;
mod sequences;
pub mod session;
mod string_ops;

pub use codegen::compile_executable;

#[cfg(test)]
mod tests;
