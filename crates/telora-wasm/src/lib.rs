//! Independent Wasm backend. No interpreter or native-backend fallback.
#[cfg(test)]
extern crate alloc;
pub mod abi;
mod aggregates;
mod array_build;
mod array_ops;
pub mod artifact;
mod capture;
mod capture_values;
mod checks;
mod codec_arrays;
mod codec_decode;
mod codec_decode_arrays;
mod codec_decode_nominal;
mod codec_decode_record;
mod codec_decode_tuple;
mod codec_display;
mod codec_enums;
mod codec_names;
mod codec_ops;
mod codec_plan;
mod codec_properties;
mod codec_records;
mod codegen;
mod data_input;
pub mod diagnostic_output;
mod diagnostics;
mod dict_build;
mod dict_literals;
mod dict_ops;
mod dictionaries;
mod dynamic_fields;
mod dynamic_kind;
mod dynamic_members;
mod dynamic_named_field;
mod dynamic_ops;
mod dynamic_sequences;
mod dynamic_tuples;
mod dynamic_variants;
mod emit;
mod entry;
mod enums;
mod equality;
mod equality_format;
mod equality_plan;
mod format_ops;
mod functions;
mod hash_ops;
mod input;
mod input_heap;
mod input_types;
mod json_ops;
mod json_parse_ops;
mod link;
mod natives;
pub mod object;
mod output;
mod parse_ops;
mod parse_plan;
mod parse_record;
mod path_ops;
mod patterns;
mod plan;
mod properties;
mod records;
mod reflection_collections;
mod reflection_data;
mod reflection_ops;
mod regex_ops;
mod regex_prepare;
#[path = "../rt/abi.rs"]
mod runtime_abi;
mod scalars;
mod sequences;
pub mod session;
mod string_ops;
mod template_ops;

pub use codegen::compile_executable;

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "../rt/json_text.rs"]
mod json_text_tests;

#[cfg(test)]
#[path = "../rt/json_parse.rs"]
mod json_parse_tests;
