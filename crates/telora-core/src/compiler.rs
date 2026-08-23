include!("compiler/frontend.rs");
include!("compiler/block.rs");
include!("compiler/expression.rs");
include!("compiler/control.rs");
include!("compiler/analysis.rs");

#[cfg(test)]
#[path = "compiler/tests/mod.rs"]
mod tests;
