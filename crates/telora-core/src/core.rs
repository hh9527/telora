use crate::value::{
    CoreArrayFunction, CoreAttributesFunction, CoreCodecFunction, CoreDictFunction,
    CoreDynFunction, CoreEqFunction, CoreHashFunction, CoreJsonFunction, CoreModelFunction,
    CorePathFunction, CoreResultFunction, CoreStringFunction, CoreTypeDescFunction, NativeFunction,
};

pub(crate) const PRELUDE_MODULE: &str = "core/prelude";
pub(crate) const ARRAY_MODULE: &str = "std/array";
pub(crate) const ATTRIBUTES_MODULE: &str = "std/attributes";
pub(crate) const DICT_MODULE: &str = "std/dict";
pub(crate) const BUILD_MODULE: &str = "std/build";
pub(crate) const EXEC_MODULE: &str = "std/rt-types/exec.telora";
pub(crate) const ARGV_MODULE: &str = "std/argv";
pub(crate) const CODEC_MODULE: &str = "std/codec";
pub(crate) const OPTION_MODULE: &str = "std/option";
pub(crate) const RESULT_MODULE: &str = "std/result";
pub(crate) const JSON_MODULE: &str = "std/json";
pub(crate) const VALUE_MODULE: &str = "std/value";
pub(crate) const YAML_MODULE: &str = "std/yaml";
pub(crate) const HASH_MODULE: &str = "std/hash";
pub(crate) const STRING_MODULE: &str = "std/string";
pub(crate) const PATH_MODULE: &str = "std/path";
pub(crate) const TOML_MODULE: &str = "std/toml";
pub(crate) const TYPE_DESC_MODULE: &str = "std/type-desc";
pub(crate) const DYN_MODULE: &str = "std/dyn";
pub(crate) const EQ_MODULE: &str = "std/eq";
pub(crate) const REGEX_MODULE: &str = "std/regex";
pub(crate) const FMT_MODULE: &str = "std/fmt";
pub(crate) const EDGE_RUNTIME_MODULE: &str = "std/rt.priv.telora";

pub(crate) fn run_entry_source() -> &'static str {
    include_str!("../modules/run-entry.telora")
}

pub(crate) fn edge_runtime_source() -> &'static str {
    include_str!("../modules/std/rt.priv.telora")
}

pub(crate) struct CoreModuleSpec {
    pub(crate) native_id: u32,
    pub(crate) name: &'static str,
    pub(crate) source: &'static str,
    pub(crate) functions: Vec<(&'static str, NativeFunction)>,
}

pub(crate) fn module_specs() -> Vec<CoreModuleSpec> {
    vec![
        CoreModuleSpec {
            native_id: 23,
            name: VALUE_MODULE,
            source: include_str!("../modules/std/value.telora"),
            functions: vec![],
        },
        CoreModuleSpec {
            native_id: 1,
            name: EQ_MODULE,
            source: include_str!("../modules/std/eq.native.telora"),
            functions: vec![("equal", NativeFunction::core_eq(CoreEqFunction::Equal))],
        },
        CoreModuleSpec {
            native_id: 2,
            name: DYN_MODULE,
            source: include_str!("../modules/std/dyn.native.telora"),
            functions: vec![
                ("pack", NativeFunction::core_dyn(CoreDynFunction::Pack)),
                (
                    "project_with",
                    NativeFunction::core_dyn(CoreDynFunction::ProjectWith),
                ),
                ("desc", NativeFunction::core_dyn(CoreDynFunction::Desc)),
                ("kind", NativeFunction::core_dyn(CoreDynFunction::Kind)),
                (
                    "check_int",
                    NativeFunction::core_dyn(CoreDynFunction::CheckInt),
                ),
                (
                    "check_float",
                    NativeFunction::core_dyn(CoreDynFunction::CheckFloat),
                ),
                (
                    "check_string",
                    NativeFunction::core_dyn(CoreDynFunction::CheckString),
                ),
                (
                    "check_bytes",
                    NativeFunction::core_dyn(CoreDynFunction::CheckBytes),
                ),
                ("field", NativeFunction::core_dyn(CoreDynFunction::Field)),
                ("fields", NativeFunction::core_dyn(CoreDynFunction::Fields)),
                (
                    "array_items",
                    NativeFunction::core_dyn(CoreDynFunction::ArrayItems),
                ),
                (
                    "tuple_items",
                    NativeFunction::core_dyn(CoreDynFunction::TupleItems),
                ),
                ("tag", NativeFunction::core_dyn(CoreDynFunction::Tag)),
                (
                    "payload",
                    NativeFunction::core_dyn(CoreDynFunction::Payload),
                ),
            ],
        },
        CoreModuleSpec {
            native_id: 3,
            name: TYPE_DESC_MODULE,
            source: include_str!("../modules/std/type-desc.native.telora"),
            functions: vec![
                (
                    "children",
                    NativeFunction::core_type_desc(CoreTypeDescFunction::Children),
                ),
                (
                    "kind",
                    NativeFunction::core_type_desc(CoreTypeDescFunction::Kind),
                ),
                (
                    "opaque_name",
                    NativeFunction::core_type_desc(CoreTypeDescFunction::OpaqueName),
                ),
                (
                    "resolve",
                    NativeFunction::core_type_desc(CoreTypeDescFunction::Resolve),
                ),
            ],
        },
        CoreModuleSpec {
            native_id: 4,
            name: ATTRIBUTES_MODULE,
            source: include_str!("../modules/std/attributes.native.telora"),
            functions: vec![
                (
                    "add",
                    NativeFunction::core_attributes(CoreAttributesFunction::Add),
                ),
                (
                    "all",
                    NativeFunction::core_attributes(CoreAttributesFunction::All),
                ),
                (
                    "get",
                    NativeFunction::core_attributes(CoreAttributesFunction::Get),
                ),
                (
                    "has",
                    NativeFunction::core_attributes(CoreAttributesFunction::Has),
                ),
                (
                    "normalize",
                    NativeFunction::core_attributes(CoreAttributesFunction::Normalize),
                ),
                (
                    "strip",
                    NativeFunction::core_attributes(CoreAttributesFunction::Strip),
                ),
            ],
        },
        CoreModuleSpec {
            native_id: 5,
            name: ARRAY_MODULE,
            source: include_str!("../modules/std/array.native.telora"),
            functions: vec![
                ("all", NativeFunction::core_array(CoreArrayFunction::All)),
                ("any", NativeFunction::core_array(CoreArrayFunction::Any)),
                (
                    "enumerate",
                    NativeFunction::core_array(CoreArrayFunction::Enumerate),
                ),
                ("get", NativeFunction::core_array(CoreArrayFunction::Get)),
                ("push", NativeFunction::core_array(CoreArrayFunction::Push)),
                (
                    "concat",
                    NativeFunction::core_array(CoreArrayFunction::Concat),
                ),
                ("zip", NativeFunction::core_array(CoreArrayFunction::Zip)),
                (
                    "filter",
                    NativeFunction::core_array(CoreArrayFunction::Filter),
                ),
                (
                    "flat_map",
                    NativeFunction::core_array(CoreArrayFunction::FlatMap),
                ),
                ("fold", NativeFunction::core_array(CoreArrayFunction::Fold)),
                (
                    "fold_control",
                    NativeFunction::core_array(CoreArrayFunction::FoldControl),
                ),
                ("find", NativeFunction::core_array(CoreArrayFunction::Find)),
                (
                    "length",
                    NativeFunction::core_array(CoreArrayFunction::Length),
                ),
                ("map", NativeFunction::core_array(CoreArrayFunction::Map)),
            ],
        },
        CoreModuleSpec {
            native_id: 6,
            name: DICT_MODULE,
            source: include_str!("../modules/std/dict.native.telora"),
            functions: vec![
                (
                    "filter",
                    NativeFunction::core_dict(CoreDictFunction::Filter),
                ),
                ("fold", NativeFunction::core_dict(CoreDictFunction::Fold)),
                ("get", NativeFunction::core_dict(CoreDictFunction::Get)),
                (
                    "from_pairs",
                    NativeFunction::core_dict(CoreDictFunction::FromPairs),
                ),
                ("keys", NativeFunction::core_dict(CoreDictFunction::Keys)),
                ("merge", NativeFunction::core_dict(CoreDictFunction::Merge)),
                (
                    "map_values",
                    NativeFunction::core_dict(CoreDictFunction::MapValues),
                ),
                ("pairs", NativeFunction::core_dict(CoreDictFunction::Pairs)),
                (
                    "values",
                    NativeFunction::core_dict(CoreDictFunction::Values),
                ),
            ],
        },
        CoreModuleSpec {
            native_id: 7,
            name: STRING_MODULE,
            source: include_str!("../modules/std/string.native.telora"),
            functions: vec![
                (
                    "contains",
                    NativeFunction::core_string(CoreStringFunction::Contains),
                ),
                (
                    "ends_with",
                    NativeFunction::core_string(CoreStringFunction::EndsWith),
                ),
                (
                    "join",
                    NativeFunction::core_string(CoreStringFunction::Join),
                ),
                (
                    "join_lines",
                    NativeFunction::core_string(CoreStringFunction::JoinLines),
                ),
                (
                    "length",
                    NativeFunction::core_string(CoreStringFunction::Length),
                ),
                (
                    "lines",
                    NativeFunction::core_string(CoreStringFunction::Lines),
                ),
                (
                    "replace",
                    NativeFunction::core_string(CoreStringFunction::Replace),
                ),
                (
                    "indent",
                    NativeFunction::core_string(CoreStringFunction::Indent),
                ),
                (
                    "ensure_trailing_newline",
                    NativeFunction::core_string(CoreStringFunction::EnsureTrailingNewline),
                ),
                (
                    "trim_margin",
                    NativeFunction::core_string(CoreStringFunction::TrimMargin),
                ),
                (
                    "split",
                    NativeFunction::core_string(CoreStringFunction::Split),
                ),
                (
                    "starts_with",
                    NativeFunction::core_string(CoreStringFunction::StartsWith),
                ),
                (
                    "parse",
                    NativeFunction::new("std/string.parse", 2, crate::regex::native_parse),
                ),
                (
                    "decode_by_parse",
                    NativeFunction::new(
                        "std/string.decode_by_parse",
                        2,
                        crate::regex::native_decode_by_parse,
                    ),
                ),
                (
                    "encode_by_display",
                    NativeFunction::new(
                        "std/string.encode_by_display",
                        2,
                        crate::regex::native_encode_by_display,
                    ),
                ),
            ],
        },
        CoreModuleSpec {
            native_id: 8,
            name: PATH_MODULE,
            source: include_str!("../modules/std/path.native.telora"),
            functions: vec![
                (
                    "file_name",
                    NativeFunction::core_path(CorePathFunction::FileName),
                ),
                ("join", NativeFunction::core_path(CorePathFunction::Join)),
                (
                    "normalize",
                    NativeFunction::core_path(CorePathFunction::Normalize),
                ),
                (
                    "parent",
                    NativeFunction::core_path(CorePathFunction::Parent),
                ),
            ],
        },
        CoreModuleSpec {
            native_id: 9,
            name: TOML_MODULE,
            source: include_str!("../modules/std/toml.telora"),
            functions: vec![(
                "parse_raw",
                NativeFunction::core_json(CoreJsonFunction::ParseToml),
            )],
        },
        CoreModuleSpec {
            native_id: 24,
            name: YAML_MODULE,
            source: include_str!("../modules/std/yaml.native.telora"),
            functions: vec![(
                "parse_raw",
                NativeFunction::core_json(CoreJsonFunction::ParseYaml),
            )],
        },
        CoreModuleSpec {
            native_id: 11,
            name: BUILD_MODULE,
            source: include_str!("../modules/std/build.telora"),
            functions: vec![],
        },
        CoreModuleSpec {
            native_id: 13,
            name: CODEC_MODULE,
            source: include_str!("../modules/std/codec.native.telora"),
            functions: vec![
                (
                    "decode",
                    NativeFunction::core_codec(CoreCodecFunction::Decode),
                ),
                (
                    "encode",
                    NativeFunction::core_codec(CoreCodecFunction::Encode),
                ),
            ],
        },
        CoreModuleSpec {
            native_id: 14,
            name: OPTION_MODULE,
            source: include_str!("../modules/std/option.telora"),
            functions: vec![],
        },
        CoreModuleSpec {
            native_id: 15,
            name: RESULT_MODULE,
            source: include_str!("../modules/std/result.native.telora"),
            functions: vec![(
                "unwrap",
                NativeFunction::core_result(CoreResultFunction::Unwrap),
            )],
        },
        CoreModuleSpec {
            native_id: 16,
            name: HASH_MODULE,
            source: include_str!("../modules/std/hash.native.telora"),
            functions: vec![
                (
                    "sha256",
                    NativeFunction::core_hash(CoreHashFunction::Sha256),
                ),
                (
                    "new",
                    NativeFunction::new_with_native_type(
                        "std/hash.new",
                        0,
                        3,
                        crate::sha256::native_new,
                    ),
                ),
                (
                    "update_bytes",
                    NativeFunction::new_with_native_type(
                        "std/hash.update_bytes",
                        2,
                        3,
                        crate::sha256::native_update_bytes,
                    ),
                ),
                (
                    "update_string",
                    NativeFunction::new_with_native_type(
                        "std/hash.update_string",
                        2,
                        3,
                        crate::sha256::native_update_string,
                    ),
                ),
                (
                    "update_int",
                    NativeFunction::new_with_native_type(
                        "std/hash.update_int",
                        2,
                        3,
                        crate::sha256::native_update_int,
                    ),
                ),
                (
                    "finish",
                    NativeFunction::new_with_native_type(
                        "std/hash.finish",
                        1,
                        3,
                        crate::sha256::native_finish,
                    ),
                ),
            ],
        },
        CoreModuleSpec {
            native_id: 17,
            name: JSON_MODULE,
            source: include_str!("../modules/std/json.native.telora"),
            functions: vec![
                (
                    "parse_raw",
                    NativeFunction::core_json(CoreJsonFunction::Parse),
                ),
                (
                    "decode",
                    NativeFunction::core_json(CoreJsonFunction::Decode),
                ),
                (
                    "default",
                    NativeFunction::core_json(CoreJsonFunction::Default),
                ),
                (
                    "flatten",
                    NativeFunction::core_json(CoreJsonFunction::Flatten),
                ),
                (
                    "untagged",
                    NativeFunction::core_json(CoreJsonFunction::Untagged),
                ),
                (
                    "schema",
                    NativeFunction::core_json(CoreJsonFunction::Schema),
                ),
                (
                    "rename",
                    NativeFunction::core_json(CoreJsonFunction::Rename),
                ),
                (
                    "rename_all",
                    NativeFunction::core_json(CoreJsonFunction::RenameAll),
                ),
                (
                    "skip_serializing_if",
                    NativeFunction::core_json(CoreJsonFunction::SkipSerializingIf),
                ),
                (
                    "stringify",
                    NativeFunction::core_json(CoreJsonFunction::Stringify),
                ),
                (
                    "stringify_pretty",
                    NativeFunction::core_json(CoreJsonFunction::StringifyPretty),
                ),
            ],
        },
        CoreModuleSpec {
            native_id: 18,
            name: PRELUDE_MODULE,
            source: include_str!("../modules/core/prelude.native.telora"),
            functions: vec![
                (
                    "union",
                    NativeFunction::core_model(CoreModelFunction::Union),
                ),
                (
                    "validate",
                    NativeFunction::new("validate", 2, crate::types::native_validate),
                ),
            ],
        },
        CoreModuleSpec {
            native_id: 19,
            name: REGEX_MODULE,
            source: include_str!("../modules/std/regex.native.telora"),
            functions: vec![
                (
                    "compile",
                    NativeFunction::new_with_native_type(
                        "std/regex.compile",
                        1,
                        0,
                        crate::regex::native_compile,
                    ),
                ),
                (
                    "is_match",
                    NativeFunction::new_with_native_type(
                        "std/regex.is_match",
                        2,
                        0,
                        crate::regex::native_is_match,
                    ),
                ),
                (
                    "prepare",
                    NativeFunction::new_with_native_type(
                        "std/regex.prepare",
                        3,
                        0,
                        crate::regex::native_prepare,
                    ),
                ),
            ],
        },
        CoreModuleSpec {
            native_id: 20,
            name: FMT_MODULE,
            source: include_str!("../modules/std/fmt.native.telora"),
            functions: vec![
                (
                    "prepare",
                    NativeFunction::new_with_native_type(
                        "std/fmt.prepare",
                        3,
                        0,
                        crate::fmt::native_prepare,
                    ),
                ),
                (
                    "display",
                    NativeFunction::new_with_native_type(
                        "std/fmt.display",
                        2,
                        0,
                        crate::fmt::native_display,
                    ),
                ),
            ],
        },
        CoreModuleSpec {
            native_id: 21,
            name: EXEC_MODULE,
            source: include_str!("../modules/std/rt-types/exec.telora"),
            functions: vec![],
        },
        CoreModuleSpec {
            native_id: 22,
            name: ARGV_MODULE,
            source: include_str!("../modules/std/argv.telora"),
            functions: vec![],
        },
    ]
}
