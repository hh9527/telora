use crate::{
    data_parse::telora_parse_data,
    values::{telora_invoke, word},
};
use alloc::{string::String, vec::Vec};

fn failure(diagnostics: &str, output: &mut Vec<u8>) {
    let mut json = String::from(
        "{\"schema\":\"telora.service/v1\",\"ok\":null,\"error\":true,\"diagnostics\":[",
    );
    json.push_str(diagnostics);
    json.push_str("]}");
    output.extend_from_slice(json.as_bytes());
}

#[unsafe(export_name = "run-service")]
pub unsafe extern "C" fn run(input: u32, length: u32, output: u32, cap: u32, result: u32) {
    unsafe { run_input(input, length, output, cap, result, false) }
}

#[unsafe(export_name = "run-service-located")]
pub unsafe extern "C" fn run_located(input: u32, length: u32, output: u32, cap: u32, result: u32) {
    unsafe { run_input(input, length, output, cap, result, true) }
}

unsafe fn run_input(input: u32, length: u32, output: u32, cap: u32, result: u32, located: bool) {
    unsafe {
        assert!(
            super::service().phase == super::Phase::Ready,
            "service not ready"
        );
        let mut output = super::buffers::take(input, length, output, cap, result);
        let packet = if located {
            crate::data_parse::telora_import_located(input, length)
        } else {
            telora_parse_data(input, length, 1, 0)
        };
        let error = word(packet, 12);
        if error != 0 {
            let message = crate::data_parse::error_text(error + 8);
            failure(message, &mut output);
        } else {
            let materialize: unsafe extern "C" fn(u32, u32) -> u32 =
                core::mem::transmute(super::service().contract.materialize);
            let value = materialize(packet, 0);
            assert_ne!(value, 0, "validated request failed materialization");
            let args = crate::telora_alloc(8);
            crate::heap::write(args, value);
            let reply_output = crate::telora_alloc(crate::abi::STRING_BYTES);
            let reply = telora_invoke(super::service().handler, args, reply_output);
            assert_ne!(
                reply, 0,
                "closed service entry failed to serialize diagnostics"
            );
            output.extend_from_slice(crate::text::text(reply).as_bytes());
        }
        super::buffers::give(output, result);
    }
}
