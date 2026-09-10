//! Embedded source inventory only: no native functions, types, Engine or VM.
macro_rules! sources {
    ($($name:literal),* $(,)?) => { &[$((concat!("std/", $name),
        include_str!(concat!("../modules/std/", $name, ".telora")))),*] };
}
pub const BUILTINS: &[(&str, &str)] = sources![
    "_codec",
    "_rt",
    "actor",
    "argv",
    "array",
    "blame",
    "codec",
    "dict",
    "dyn",
    "ees",
    "entry",
    "eq",
    "fmt",
    "hash",
    "json",
    "option",
    "path",
    "prelude",
    "regex",
    "result",
    "rt-types/exec",
    "string",
    "test",
    "toml",
    "type-desc",
    "type-property",
    "value",
    "yaml",
];

/// Trusted source admission assigns ABI identities before any source is read.
/// Only this inventory layer deals with module names; consumers use numeric IDs.
pub fn native_module(name: &str) -> Option<crate::mir::NativeModule> {
    use crate::mir::{NativeModule, NativeTypeRule as R, TypeConstructor as T, TypeFunction as F};
    let (id, types) = match name {
        "std/prelude" => (
            18,
            vec![
                (0, R::Primitive(T::Type)),
                (1, R::Primitive(T::Dyn)),
                (2, R::Primitive(T::Never)),
                (3, R::Primitive(T::Tuple)),
                (4, R::Primitive(T::Int)),
                (5, R::Primitive(T::Float)),
                (6, R::Primitive(T::String)),
                (7, R::Primitive(T::Bytes)),
                (8, R::Primitive(T::Bool)),
                (9, R::Primitive(T::PropertyTarget)),
                (10, R::Constructor(F::Array)),
                (11, R::Constructor(F::Dict)),
                (12, R::Constructor(F::TypeOf)),
                (13, R::Constructor(F::Unchecked)),
                (14, R::Constructor(F::Tuple)),
                (15, R::Constructor(F::Func)),
                (16, R::Constructor(F::Option)),
                (17, R::Constructor(F::Result)),
                (18, R::Constructor(F::FoldControl)),
                (19, R::Constructor(F::Property)),
            ],
        ),
        "std/regex" => (19, vec![(0, R::Opaque)]),
        "std/fmt" => (20, vec![(1, R::Opaque)]),
        "std/hash" => (16, vec![(3, R::Opaque)]),
        "std/test" => (33, vec![(0, R::Opaque)]),
        "std/blame" => (
            crate::mir::NativeTypeId::BLAME_ERROR.module,
            vec![(crate::mir::NativeTypeId::BLAME_ERROR.slot, R::Opaque)],
        ),
        _ => return None,
    };
    Some(NativeModule { id, types })
}
