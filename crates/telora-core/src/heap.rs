include!("heap/value.rs");
include!("heap/value-builder.rs");
include!("heap/object.rs");
include!("heap/storage.rs");
include!("heap/view.rs");
include!("heap/type-family.rs");
include!("heap/publish.rs");
include!("heap/semantic.rs");
include!("heap/copy.rs");

#[cfg(test)]
#[path = "heap/tests/mod.rs"]
mod tests;
