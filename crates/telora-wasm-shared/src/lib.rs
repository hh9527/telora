//! ABI and type-independent text primitives shared by codegen and the Wasm runtime.
#![no_std]
extern crate alloc;

pub mod abi;
pub mod json_text;
pub mod location_tables;
pub mod locations;

pub mod service;
