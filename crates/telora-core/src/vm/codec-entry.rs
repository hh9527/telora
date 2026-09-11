#[allow(clippy::too_many_arguments)]
fn run_core_codec(
    operation: CoreCodecFunction,
    arguments: &[Val],
    signature: Option<Val>,
    return_target: ReturnTarget,
    rule_boundary: Option<crate::Loc>,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    match operation {
        CoreCodecFunction::Encode => run_solved_codec_encode(
            arguments,
            signature,
            return_target,
            rule_boundary,
            function,
            pc,
            current,
            background,
            account,
        ),
        CoreCodecFunction::Decode => run_solved_codec_decode(
            arguments,
            signature,
            return_target,
            function,
            pc,
            current,
            background,
            account,
        ),
    }
}
