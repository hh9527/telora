use crate::value::{
    CoreArrayFunction, CoreAttributesFunction, CoreCodecFunction, CoreDictFunction,
    CoreDynFunction, CoreEqFunction, CoreHashFunction, CoreJsonFunction, CoreModelFunction,
    CorePathFunction, CoreResultFunction, CoreStringFunction, CoreTypeDescFunction, NativeFunction,
};

pub(crate) const PRELUDE_MODULE: &str = "std/prelude";
pub(crate) const ARRAY_MODULE: &str = "std/array";
pub(crate) const ATTRIBUTES_MODULE: &str = "std/attributes";
pub(crate) const DICT_MODULE: &str = "std/dict";
pub(crate) const BUILD_MODULE: &str = "std/build";
pub(crate) const EXEC_MODULE: &str = "std/rt-types/exec";
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
pub(crate) const TYPE_PROPERTY_MODULE: &str = "std/type-property";
pub(crate) const DYN_MODULE: &str = "std/dyn";
pub(crate) const EQ_MODULE: &str = "std/eq";
pub(crate) const REGEX_MODULE: &str = "std/regex";
pub(crate) const FMT_MODULE: &str = "std/fmt";
pub(crate) const FMT_CAPABILITY_BINDING: &str = "\0std:fmt";
pub(crate) const EDGE_RUNTIME_MODULE: &str = "std/_rt";
pub(crate) const DEFAULT_ENTRY_MODULE: &str = "std/entry/default";
pub(crate) const SERVE_ENTRY_MODULE: &str = "std/entry/serve";

pub(crate) fn default_entry_source() -> &'static str {
    include_str!("../modules/std/entry/default.telora")
}

pub(crate) fn serve_entry_source() -> &'static str {
    include_str!("../modules/std/entry/serve.telora")
}

pub(crate) struct BuiltinModuleSpec {
    pub(crate) native_id: u32,
    pub(crate) name: &'static str,
    pub(crate) source: &'static str,
    pub(crate) functions: Vec<(&'static str, NativeFunction)>,
}

pub(crate) fn module_specs() -> Vec<BuiltinModuleSpec> {
    let mut specs = vec![
        BuiltinModuleSpec {
            native_id: 26,
            name: EDGE_RUNTIME_MODULE,
            source: include_str!("../modules/std/_rt.telora"),
            functions: vec![(
                "call_with_diagnostics",
                NativeFunction::core_runtime(
                    crate::value::CoreRuntimeFunction::CallWithDiagnostics,
                ),
            )],
        },
        BuiltinModuleSpec {
            native_id: 25,
            name: TYPE_PROPERTY_MODULE,
            source: include_str!("../modules/std/type-property.telora"),
            functions: vec![
                (
                    "get_type_prop",
                    NativeFunction::new(
                        "std/type-property.get_type_prop",
                        2,
                        crate::property::native_get_type,
                    ),
                ),
                (
                    "get_field_prop",
                    NativeFunction::new(
                        "std/type-property.get_field_prop",
                        3,
                        crate::property::native_get_field,
                    ),
                ),
                (
                    "get_variant_prop",
                    NativeFunction::new(
                        "std/type-property.get_variant_prop",
                        3,
                        crate::property::native_get_variant,
                    ),
                ),
                (
                    "evidence",
                    NativeFunction::new(
                        "std/type-property.evidence",
                        3,
                        crate::property::native_evidence,
                    ),
                ),
            ],
        },
        BuiltinModuleSpec {
            native_id: 23,
            name: VALUE_MODULE,
            source: include_str!("../modules/std/value.telora"),
            functions: vec![],
        },
        BuiltinModuleSpec {
            native_id: 1,
            name: EQ_MODULE,
            source: include_str!("../modules/std/eq.telora"),
            functions: vec![("equal", NativeFunction::core_eq(CoreEqFunction::Equal))],
        },
        BuiltinModuleSpec {
            native_id: 2,
            name: DYN_MODULE,
            source: include_str!("../modules/std/dyn.telora"),
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
                (
                    "get_field_value",
                    NativeFunction::core_dyn(CoreDynFunction::GetFieldValue),
                ),
                (
                    "get_variant_index",
                    NativeFunction::core_dyn(CoreDynFunction::GetVariantIndex),
                ),
                (
                    "get_variant_payload",
                    NativeFunction::core_dyn(CoreDynFunction::GetVariantPayload),
                ),
            ],
        },
        BuiltinModuleSpec {
            native_id: 3,
            name: TYPE_DESC_MODULE,
            source: include_str!("../modules/std/type-desc.telora"),
            functions: vec![
                (
                    "children",
                    NativeFunction::core_type_desc(CoreTypeDescFunction::Children),
                ),
                (
                    "fields",
                    NativeFunction::core_type_desc(CoreTypeDescFunction::Fields),
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
                (
                    "strip_attributes",
                    NativeFunction::core_type_desc(CoreTypeDescFunction::StripAttributes),
                ),
                (
                    "variants",
                    NativeFunction::core_type_desc(CoreTypeDescFunction::Variants),
                ),
            ],
        },
        BuiltinModuleSpec {
            native_id: 4,
            name: ATTRIBUTES_MODULE,
            source: include_str!("../modules/std/attributes.telora"),
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
        BuiltinModuleSpec {
            native_id: 5,
            name: ARRAY_MODULE,
            source: include_str!("../modules/std/array.telora"),
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
        BuiltinModuleSpec {
            native_id: 6,
            name: DICT_MODULE,
            source: include_str!("../modules/std/dict.telora"),
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
        BuiltinModuleSpec {
            native_id: 7,
            name: STRING_MODULE,
            source: include_str!("../modules/std/string.telora"),
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
                    "parse_with",
                    NativeFunction::new("std/string.parse_with", 3, crate::regex::native_parse),
                ),
            ],
        },
        BuiltinModuleSpec {
            native_id: 8,
            name: PATH_MODULE,
            source: include_str!("../modules/std/path.telora"),
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
        BuiltinModuleSpec {
            native_id: 9,
            name: TOML_MODULE,
            source: include_str!("../modules/std/toml.telora"),
            functions: vec![(
                "parse_raw",
                NativeFunction::core_json(CoreJsonFunction::ParseToml),
            )],
        },
        BuiltinModuleSpec {
            native_id: 24,
            name: YAML_MODULE,
            source: include_str!("../modules/std/yaml.telora"),
            functions: vec![(
                "parse_raw",
                NativeFunction::core_json(CoreJsonFunction::ParseYaml),
            )],
        },
        BuiltinModuleSpec {
            native_id: 11,
            name: BUILD_MODULE,
            source: include_str!("../modules/std/build.telora"),
            functions: vec![],
        },
        BuiltinModuleSpec {
            native_id: 13,
            name: CODEC_MODULE,
            source: include_str!("../modules/std/codec.telora"),
            functions: vec![
                (
                    "decode_with",
                    NativeFunction::core_codec(CoreCodecFunction::Decode),
                ),
                (
                    "encode_with",
                    NativeFunction::core_codec(CoreCodecFunction::Encode),
                ),
            ],
        },
        BuiltinModuleSpec {
            native_id: 14,
            name: OPTION_MODULE,
            source: include_str!("../modules/std/option.telora"),
            functions: vec![],
        },
        BuiltinModuleSpec {
            native_id: 15,
            name: RESULT_MODULE,
            source: include_str!("../modules/std/result.telora"),
            functions: vec![(
                "unwrap",
                NativeFunction::core_result(CoreResultFunction::Unwrap),
            )],
        },
        BuiltinModuleSpec {
            native_id: 16,
            name: HASH_MODULE,
            source: include_str!("../modules/std/hash.telora"),
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
        BuiltinModuleSpec {
            native_id: 17,
            name: JSON_MODULE,
            source: include_str!("../modules/std/json.telora"),
            functions: vec![
                (
                    "parse_raw",
                    NativeFunction::core_json(CoreJsonFunction::Parse),
                ),
                (
                    "decode_with",
                    NativeFunction::core_json(CoreJsonFunction::Decode),
                ),
                (
                    "schema_with",
                    NativeFunction::core_json(CoreJsonFunction::Schema),
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
        BuiltinModuleSpec {
            native_id: 18,
            name: PRELUDE_MODULE,
            source: include_str!("../modules/std/prelude.telora"),
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
        BuiltinModuleSpec {
            native_id: 19,
            name: REGEX_MODULE,
            source: include_str!("../modules/std/regex.telora"),
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
        BuiltinModuleSpec {
            native_id: 20,
            name: FMT_MODULE,
            source: include_str!("../modules/std/fmt.telora"),
            functions: vec![
                (
                    "prepare",
                    NativeFunction::new("std/fmt.prepare", 1, crate::fmt::native_prepare),
                ),
                (
                    "from_string",
                    NativeFunction::new_with_native_type(
                        "std/fmt.from_string",
                        1,
                        1,
                        crate::fmt::native_from_string,
                    ),
                ),
                (
                    "from_int",
                    NativeFunction::new_with_native_type(
                        "std/fmt.from_int",
                        1,
                        1,
                        crate::fmt::native_from_int,
                    ),
                ),
                (
                    "from_float",
                    NativeFunction::new_with_native_type(
                        "std/fmt.from_float",
                        1,
                        1,
                        crate::fmt::native_from_float,
                    ),
                ),
                (
                    "from_atom",
                    NativeFunction::new_with_native_type(
                        "std/fmt.from_atom",
                        1,
                        1,
                        crate::fmt::native_from_atom,
                    ),
                ),
                (
                    "concat",
                    NativeFunction::new_with_native_type(
                        "std/fmt.concat",
                        2,
                        1,
                        crate::fmt::native_concat,
                    ),
                ),
                (
                    "render",
                    NativeFunction::new("std/fmt.render", 1, crate::fmt::native_render),
                ),
            ],
        },
        BuiltinModuleSpec {
            native_id: 21,
            name: EXEC_MODULE,
            source: include_str!("../modules/std/rt-types/exec.telora"),
            functions: vec![],
        },
        BuiltinModuleSpec {
            native_id: 22,
            name: ARGV_MODULE,
            source: include_str!("../modules/std/argv.telora"),
            functions: vec![],
        },
    ];
    // Built-in sources are installed in dependency order. Native IDs remain
    // stable and are independent of this installation sequence.
    specs.sort_by_key(|spec| match spec.name {
        TYPE_PROPERTY_MODULE => 0,
        VALUE_MODULE => 1,
        EQ_MODULE => 2,
        DYN_MODULE => 3,
        TYPE_DESC_MODULE => 4,
        ATTRIBUTES_MODULE => 5,
        ARRAY_MODULE => 6,
        DICT_MODULE => 7,
        REGEX_MODULE => 8,
        STRING_MODULE => 9,
        FMT_MODULE => 10,
        CODEC_MODULE => 11,
        JSON_MODULE => 12,
        _ => 13,
    });
    specs
}
