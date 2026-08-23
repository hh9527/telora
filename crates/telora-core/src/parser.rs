include!("parser/frontend.rs");
include!("parser/bindings.rs");
include!("parser/expression.rs");
include!("parser/patterns.rs");
include!("parser/helpers.rs");
include!("parser/elaboration.rs");

#[cfg(test)]
#[path = "parser/tests/mod.rs"]
mod tests;
