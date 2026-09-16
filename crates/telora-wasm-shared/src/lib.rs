//! ABI and type-independent text primitives shared by codegen and the Wasm runtime.
#![no_std]
extern crate alloc;

pub mod abi;
pub mod arena;
pub mod json_text;
pub mod source_range;

pub mod service;

pub mod diagnostics;
