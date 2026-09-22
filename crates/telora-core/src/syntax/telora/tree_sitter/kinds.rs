use super::{Rule, TsNode};
use std::sync::OnceLock;

struct Kind {
    rules: Option<&'static [Rule]>,
    token: Option<super::Token>,
}

fn table() -> &'static [Kind] {
    static TABLE: OnceLock<Vec<Kind>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let language: tree_sitter::Language = tree_sitter_telora::LANGUAGE.into();
        (0..language.node_kind_count())
            .map(|id| {
                let id = id as u16;
                let name = language.node_kind_for_id(id).expect("bundled symbol");
                Kind {
                    rules: if language.node_kind_is_named(id) {
                        named_rules(name)
                    } else {
                        Some(&[])
                    },
                    token: super::tokens::leaf(name),
                }
            })
            .collect()
    })
}

pub(super) fn token(node: TsNode<'_>) -> Option<super::Token> {
    table()
        .get(usize::from(node.kind_id()))
        .and_then(|kind| kind.token)
}

/// Tree-sitter keeps editor wrappers which the semantic CST intentionally omits.
pub(super) fn rules(node: TsNode<'_>, parent: Option<TsNode<'_>>) -> Option<&'static [Rule]> {
    if node.is_error() {
        return Some(&[Rule::Error]);
    }
    if !node.is_named() {
        return Some(&[]);
    }
    if node.kind() == "int_expr"
        && parent.is_some_and(|p| matches!(p.kind(), "projection_suffix" | "native_type_binding"))
    {
        return Some(&[]);
    }
    table()
        .get(usize::from(node.kind_id()))
        .and_then(|kind| kind.rules)
}

fn named_rules(name: &str) -> Option<&'static [Rule]> {
    Some(match name {
        "source_file" => &[],
        "block_body" => &[Rule::Body],
        "concat_string" => &[Rule::StringExpr, Rule::ConcatExpression],
        "spread_item" => &[Rule::SpreadExpr],
        "module_binding"
        | "primary"
        | "expression_statement"
        | "type_initializer"
        | "contract_argument"
        | "dict_item"
        | "array_item"
        | "ctrl_block"
        | "identifier"
        | "placeholder"
        | "indexed_placeholder"
        | "section_lparen"
        | "raw_string"
        | "raw_start"
        | "raw_text"
        | "raw_end"
        | "concat_fragment"
        | "comment"
        | "quote_start"
        | "quote_end"
        | "string_text"
        | "escape_sequence"
        | "spaces"
        | "tabs"
        | "newline" => &[],
        "ERROR" | "invalid_function_parameter" => &[Rule::Error],
        "argument" => &[Rule::Argument],
        "arguments" => &[Rule::Arguments],
        "array_expr" => &[Rule::ArrayExpr],
        "binary_expr" => &[Rule::BinaryExpr],
        "binding" => &[Rule::Binding],
        "block" => &[Rule::Block],
        "bytes_expr" => &[Rule::BytesExpr],
        "call_expr" => &[Rule::CallExpr],
        "closure" => &[Rule::Closure],
        "constructor_pattern" => &[Rule::ConstructorPattern],
        "contract" => &[],
        "contract_array" => &[Rule::ContractArray],
        "contract_expr" => &[Rule::ContractExpr],
        "decl_binding" => &[Rule::DeclBinding],
        "data_binding" => &[Rule::DataBinding],
        "data_format" => &[Rule::DataFormat],
        "data_import" => &[Rule::DataImport],
        "decorator" => &[Rule::Decorator],
        "decorator_path" => &[Rule::DecoratorPath],
        "def_binding" => &[Rule::DefBinding],
        "dict_expr" => &[Rule::DictExpr],
        "dict_field" => &[Rule::DictField],
        "do_expr" => &[Rule::DoExpr],
        "dot_postfix_expr" => &[Rule::DotPostfixExpr],
        "enum_initializer" => &[Rule::EnumInitializer],
        "enum_initializer_variant" => &[Rule::EnumInitializerVariant],
        "field_projection_entry" => &[Rule::FieldProjectionEntry],
        "field_projection_suffix" => &[Rule::FieldProjectionSuffix],
        "float_expr" => &[Rule::FloatExpr],
        "float_pattern" => &[Rule::FloatPattern],
        "function_contract" => &[Rule::FunctionContract],
        "identifier_pattern" => &[Rule::IdentifierPattern],
        "if_expr" => &[Rule::IfExpr],
        "if_let_expr" => &[Rule::IfLetExpr],
        "impl_binding" => &[Rule::ImplBinding],
        "impl_member" => &[Rule::ImplMember],
        "index_expr" => &[Rule::IndexExpr],
        "int_expr" => &[Rule::IntExpr],
        "int_pattern" => &[Rule::IntPattern],
        "interpolation" => &[Rule::Interpolation],
        "interpreter_intrinsic" => &[Rule::InterpreterIntrinsic],
        "legacy_interpreter_expr" => &[Rule::LegacyInterpreterExpr],
        "let_binding" => &[Rule::LetBinding],
        "let_else_binding" => &[Rule::LetElseBinding],
        "let_pattern_binding" => &[Rule::LetPatternBinding],
        "match_arm" => &[Rule::MatchArm],
        "match_expr" => &[Rule::MatchExpr],
        "metadata_suffix" => &[Rule::MetadataSuffix],
        "module_body" => &[Rule::ModuleBody],
        "module_declaration" => &[Rule::ModuleDeclaration],
        "named_intrinsic" => &[Rule::NamedIntrinsic],
        "native_binding" => &[Rule::NativeBinding],
        "native_type_binding" => &[Rule::NativeTypeBinding],
        "parameter" => &[Rule::Parameter],
        "parameters" => &[Rule::Parameters],
        "paren_expr" => &[Rule::ParenExpr],
        "pattern" => &[Rule::Pattern],
        "pipeline_expr" => &[Rule::PipelineExpr],
        "postfix_intrinsic_suffix" => &[Rule::PostfixIntrinsicSuffix],
        "projection_suffix" => &[Rule::ProjectionSuffix],
        "propagate_expr" => &[Rule::PropagateExpr],
        "return_expr" => &[Rule::ReturnExpr],
        "section_arguments" => &[Rule::SectionArguments],
        "section_expr" => &[Rule::SectionExpr],
        "string_expr" => &[Rule::StringExpr],
        "string_literal" => &[Rule::StringLiteral],
        "string_pattern" => &[Rule::StringPattern],
        "static_path" => &[Rule::StaticPath],
        "static_path_expr" => &[Rule::StaticPathExpr],
        "struct_initializer" => &[Rule::StructInitializer],
        "struct_initializer_field" => &[Rule::StructInitializerField],
        "struct_pattern" => &[Rule::StructPattern],
        "struct_pattern_field" => &[Rule::StructPatternField],
        "trait_binding" => &[Rule::TraitBinding],
        "trait_bound" => &[Rule::TraitBound],
        "trait_member" => &[Rule::TraitMember],
        "tuple_pattern" => &[Rule::TuplePattern],
        "type_apply_expr" => &[Rule::TypeApplyExpr],
        "type_argument" => &[Rule::TypeArgument],
        "type_arguments" => &[Rule::TypeArguments],
        "type_binding" => &[Rule::TypeBinding],
        "type_parameter" => &[Rule::TypeParameter],
        "type_parameters" => &[Rule::TypeParameters],
        "type_scheme" => &[Rule::TypeScheme],
        "unary_expr" => &[Rule::UnaryExpr],
        "unit_contract" => &[Rule::UnitContract],
        "use_binding" => &[Rule::UseBinding],
        "use_item" => &[Rule::UseItem],
        "use_items" => &[Rule::UseItems],
        "use_path" => &[Rule::UsePath],
        "use_selector" => &[Rule::UseSelector],
        "variable_expr" => &[Rule::VariableExpr],
        "visibility" => &[Rule::Visibility],
        _ => return None,
    })
}
