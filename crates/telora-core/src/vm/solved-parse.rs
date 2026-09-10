fn run_solved_parse(
    operation: CoreJsonFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let types = background.solved_types.as_ref().expect("solved parser");
    let target = solved_metadata_id(arguments[0], types, function, pc)?;
    let input = arguments[1];
    propagate_direct_failure(&input, function, pc)?;
    let view = HeapView {
        current,
        background: Some(background),
    };
    let text = (ValueRef { value: input, view })
        .as_str()
        .ok_or_else(|| runtime_shallow_type_error("String", input, function, pc))?;
    let size = text.len();
    let location = input
        .loc()
        .or_else(|| instruction_location(function, pc))
        .ok_or_else(|| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                "solved text parser requires source provenance",
                function,
                pc,
            )
        })?;
    let limits = account.data_limits;
    if size > limits.file_size {
        return Err(allocation_error(
            "parsed text exceeds file_size limit",
            function,
            pc,
        ));
    }
    let mut sources = SourceDatabase::default();
    let source_name = match operation {
        CoreJsonFunction::Parse => "<json string>",
        CoreJsonFunction::ParseYaml => "<yaml string>",
        CoreJsonFunction::ParseToml => "<toml string>",
        _ => unreachable!("text parser operation"),
    };
    let source = sources.add(source_name, text.as_str());
    let plan = match operation {
        CoreJsonFunction::Parse => crate::json::validate_json_registered(&sources, source),
        CoreJsonFunction::ParseYaml => crate::yaml::validate_yaml_registered(&sources, source),
        CoreJsonFunction::ParseToml => crate::toml::validate_toml_registered(&sources, source),
        _ => unreachable!("text parser operation"),
    };
    match plan {
        Ok(mut plan) => {
            let stats = plan
                .enforce_limits(limits, size)
                .map_err(|e| allocation_error(e.to_string(), function, pc))?;
            let bytes = logical_value_bytes(stats.nodes.saturating_mul(4))
                .map_err(|e| allocation_error(e.message, function, pc))?
                .saturating_add(stats.payloads_bytes as u64);
            charge_allocation(account, bytes, function, pc)?;
            plan.set_location(location);
            let value = crate::json::materialize_data_plan(
                &plan,
                current,
                Some(crate::json::SemanticDataTarget {
                    background: Some(background),
                    type_id: crate::TypeId::solved(target),
                }),
            )
            .value;
            finish_codec_payload(
                BuiltinAtom::Ok,
                CodecNode::Existing(value),
                input,
                return_target,
                function,
                pc,
                current,
                background,
                account,
            )
        }
        Err(diagnostics) => {
            let message = diagnostics
                .iter()
                .map(|d| sources.render(d))
                .collect::<Vec<_>>()
                .join("\n");
            charge_allocation(
                account,
                logical_value_bytes(2).map_err(|e| allocation_error(e.message, function, pc))?,
                function,
                pc,
            )?;
            let original = crate::json::semantic_tag(
                current,
                crate::json::SemanticDataTarget {
                    background: Some(background),
                    type_id: crate::TypeId::solved(target),
                },
                "String",
                input,
                location,
            );
            finish_decode_result(
                Err(CodecFailure {
                    message,
                    data: input,
                    rule: input,
                    input: Some(original),
                }),
                input,
                return_target,
                function,
                pc,
                current,
                background,
                account,
            )
        }
    }
}
