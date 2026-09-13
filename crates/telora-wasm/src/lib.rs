//! Independent Wasm backend. No interpreter or native-backend fallback.
pub mod abi;
mod aggregates;
pub mod artifact;
mod codegen;
mod dictionaries;
mod emit;
mod functions;
mod input;
mod output;
mod plan;
mod runtime;
mod scalars;
pub mod session;
mod strings;
mod tables;

pub use codegen::compile_executable;

#[cfg(test)]
mod tests;
