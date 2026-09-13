//! Independent Wasm backend. No interpreter or native-backend fallback.
mod codegen;

pub use codegen::compile_scalar;

#[cfg(test)]
mod tests;
