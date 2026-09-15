//! Shared LLW data parsers, source spans and flat data plans for Host and Wasm.
#![no_std]
#[macro_use]
extern crate alloc;

pub mod data_plan;
pub mod document;
pub mod json;
#[cfg(feature = "serde")]
pub mod json_serde;
mod limits;
pub mod source;
pub mod syntax;
mod toml;
mod yaml;

pub use document::DocumentText;
pub use limits::DataLimits;
pub use source::SourceDatabase;

#[cfg(test)]
mod data_plan_test;
#[cfg(test)]
mod stack_tests;
