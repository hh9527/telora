# Rust test migration inventory

This inventory records Rust tests whose public contract is exercised by a
language fixture. Rust tests remain in place during classification and are
removed together only after all suites have been reviewed.

## Fully replaceable

The `check/compiler-semantics` fixture fully covers these compiler tests. Each
ID is an independently evaluated export, so best-effort checking identifies a
failed expectation without stopping later exports.

| Fixture export | Rust test |
| --- | --- |
| `t0001_should_ok` | `executes_precedence_blocks_and_dict_access` |
| `t0002_should_ok` | `compares_tagged_tuples_structurally` |
| `t0003_should_ok` | `compares_functions_by_opaque_identity` |
| `t0004_should_ok` | `executes_single_assignment_recursive_definitions` |
| `t0005_should_ok`, `t0006_should_ok` | `executes_complete_numeric_comparison_semantics` |
| `t0007_should_ok` | `compares_strings_by_internal_utf8_byte_sequence` |
| `t0008_should_ok` | `inequality_preserves_structural_equality_semantics` |
| `t0009_should_ok` | `recursive_definitions_capture_their_lexical_context` |
| `t0010_should_ok` | `executes_inferred_direct_and_mutual_recursive_definitions` |
| `t0011_should_ok` | `captures_values_and_calls_closures` |
| `t0012_should_ok` | `executes_partially_annotated_closures_without_runtime_annotation_work` |
| `t0013_should_ok` | `erases_explicit_type_application_from_runtime_calls` |
| `t0014_should_ok` | `executes_inferred_generic_closures_without_runtime_instances` |
| `t0015_should_ok` | `indexes_arrays_and_projects_tuples` |
| `t0018_should_ok` | `if_evaluates_only_the_selected_branch`, `control_flow_else_chains_evaluate_like_nested_expressions` |
| `t0019_should_ok` | `match_destructures_tagged_tuples` |
| `t0020_should_ok` | `atom_call_constructs_tagged_value_and_pattern_destructures_it` |
| `t0021_should_ok` | `struct_patterns_select_nested_fields_and_fall_through_dynamically` |
| `t0022_should_ok` | `local_destructuring_let_preserves_order_scope_and_nested_selection` |
| `t0023_should_ok` | `propagates_option_and_result_from_the_nearest_function` |
| `t0024_should_ok` | `infers_propagation_boundary_from_success_constructor` |
| `t0026_should_ok` | `returns_values_from_the_nearest_function` |
| `t0027_should_ok` | `match_guards_use_pattern_bindings_and_continue_after_false` |
| `t0028_should_ok` | `array_spread_flattens_fragments_in_source_order` |
| `t0029_should_ok` | `dict_spread_merges_in_source_order_with_later_values_winning` |
| `t0030_should_ok` | `evaluates_escaped_raw_and_continued_strings` |
| `t0031_should_ok` | `checked_cast_preserves_representation_and_nominal_identity` |
| `t0032_should_ok`, `t0325_should_error` | `remainder_supports_int_float_precedence_and_dynamic_boundaries` |
| `t0033_should_ok`, `t0326_should_error` | `boolean_operators_short_circuit_and_preserve_precedence` |
| `t0034_should_ok`, `t0328_should_error`, `t0329_should_error` | `logical_negation_executes_with_unary_precedence_and_dynamic_checks` |
| `t0035_should_ok`, `t0327_should_error` | `bitwise_integer_operators_execute_with_stable_precedence` |
| `t0101_should_ok` | `definition_contracts_evaluate_referenced_concrete_types_first` |
| `t0108_should_ok` | `monomorphic_recursive_closures_infer_direct_mutual_and_nested_types` |
| `t0109_should_ok` | `acyclic_definitions_generalize_in_dependency_order` |
| `t0112_should_ok` | `nested_closures_share_only_body_constraints` |
| `t0200_should_ok` | `core_option_and_result_combinators_are_generic_telora_definitions` |
| `t0201_should_ok` | `core_dict_enumerates_constructs_and_merges_in_canonical_order` |
| `t0205_should_ok` | `builtin_bool_and_option_keep_natural_json_codec_and_schema_forms` |
| `t0301_should_error` | `projection_types_are_checked_statically` |
| `t0303_should_error`, `t0315_should_error` | `rejects_mixed_and_unsupported_propagation` |
| `t0305_should_error`, `t0316_should_error` | `panic_requires_one_string_message` |
| `t0306_should_error`, `t0317_should_error` | `fail_requires_a_string_message` |
| `t0307_should_error`, `t0018_should_ok` | `if_let_selects_and_scopes_structural_patterns` |
| `t0312_should_error`, `t0028_should_ok` | `array_spread_requires_an_array_operand`, `array_spread_flattens_fragments_in_source_order` |
| `t0313_should_error`, `t0318_should_error` | `dict_spread_requires_dict_without_adding_struct_update` |
| `t0314_should_error`, `t0319_should_error` | `reports_unknown_bindings_and_arity_errors` |
| `t0320_should_error`, `t0016_should_ok` | `pipeline_is_uniform_reverse_application` |
| `t0321_should_error` | `dynamic_ordered_comparison_rejects_mismatched_domains` |
| `t0323_should_error`, `t0324_should_error` | `rejects_return_outside_functions_and_wrong_result_types` |
| `t0400_should_ok` | `generic_definition_exports_instantiate_per_member_access` |
| `t0402_should_ok` | `parameterized_type_families_construct_local_concrete_types` |
| `t0403_should_ok` | `declaration_initializers_preserve_structural_family_and_recursive_behavior` |
| `t0405_should_ok` | `imported_function_contracts_brand_nested_private_nominal_literals` |
| `t0406_should_ok` | `imported_families_preserve_recursive_arguments_in_top_level_aliases` |
| `t0408_should_ok` | `exports_imported_namespace_as_a_semantic_module` |
| `t0409_should_ok` | `exports_open_imported_locals_through_multihop_facades` |
| `t0410_should_ok` | `exports_imported_opaque_types_with_provider_identity` |
| `t0411_should_ok` | `imported_generic_apis_widen_singleton_fields_in_anonymous_records` |
| `t0412_should_ok` | `reexported_families_preserve_provider_recursive_fields` |
| `t0413_should_ok` | `generic_array_callbacks_preserve_recursive_nominal_items` |
| `t0414_should_ok` | `imported_family_aliases_preserve_provider_local_concrete_arguments` |
| `t0017_should_ok` | `call_sections_elaborate_to_ordinary_closures` |
| `t0203_should_ok`, `t0330_should_error` | `fmt_fragments_render_primitives_and_structured_concatenation` |
| `t0404_should_ok`, `t0331_should_error` | `equality_contextualizes_nominal_literals_in_both_operand_orders` |
| `t0407_should_ok`, `t0332_should_error` | `exports_imported_local_bindings_without_creating_local_aliases` |

The pilot fixtures fully cover these tests:

- `interpolates_strings_numbers_and_atoms`
- `reports_structural_annotation_mismatch`

## Partial coverage

These behaviors are exercised by a fixture, but their Rust tests also assert
an error path or an internal representation and are not yet replaceable:

- `t0025_should_ok`: `propagates_from_module_blocks_and_isolates_nested_functions`
- `t0302_should_error`, `t0322_should_error`: `checks_known_and_dynamic_unsupported_interpolation_values` (the CLI diagnostic wording differs from the direct compiler API)
- `t0202_should_ok`: `fmt_display_uses_the_display_by_blanket_implementation` (direct property display is covered; blanket trait dispatch remains in Rust)
- `t0401_should_ok`: `parameterized_type_families_preserve_schemes_across_import_forms` (one re-exported selective form is covered)
- `t0100_should_ok`: `generic_definition_contracts_check_rigidly_and_instantiate_at_each_use`
- `t0102_should_ok`: `contracted_definitions_preserve_generic_callback_result_precision`
- `t0103_should_ok`, `t0104_should_ok`: `recursive_concrete_types_remain_strict_in_definition_contracts_and_families`
- `t0105_should_ok`: `generic_use_refines_option_result_of_a_let_bound_callback`
- `t0106_should_ok`: `generic_calls_combine_singleton_atoms_with_closed_enum_evidence`
- `t0107_should_ok`: explicit and partial type application tests
- `t0110_should_ok`: `eligible_let_closures_generalize_and_instantiate_independently`
- `t0111_should_ok`: `local_generalization_respects_annotations_aliases_constraints_and_scopes`
- `t0113_should_ok`: `ordinary_expressions_use_bidirectional_checking_without_schemes`
- `t0114_should_ok`: `generic_contract_parameters_are_available_in_implementation_annotations`
- `t0115_should_ok`: recursive parameterized type-family behavior

## Retained Rust coverage

`bounded_generic_calls_forward_hidden_trait_evidence` remains a Rust compiler
test. The public package execution path currently reports an internal trait impl
binding while loading the equivalent fixture, so that path is not yet valid
replacement evidence.
