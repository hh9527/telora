//! Independent Wasm backend. No interpreter or native-backend fallback.
pub mod abi;
pub mod artifact;
mod codegen;
mod emit;
mod functions;
mod plan;
mod runtime;
mod scalars;
pub mod session;

pub use codegen::compile_executable;

#[cfg(test)]
mod tests;
