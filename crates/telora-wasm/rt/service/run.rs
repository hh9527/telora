use alloc::{string::String, vec::Vec};
use crate::{data_parse::telora_parse_data, values::{word, telora_invoke}};

fn failure(message: &str, output: &mut Vec<u8>) {
    let mut json = String::from("{\"schema\":\"telora.service/v1\",\"ok\":null,\"error\":true,\"diagnostics\":[{\"severity\":\"Error\",\"message\":");
    crate::json_text::quoted(&mut json, message).unwrap();
    json.push_str(",\"labels\":[],\"notes\":[]}]}");
    output.extend_from_slice(json.as_bytes());
}

#[unsafe(export_name = "run-service")]
pub unsafe extern "C" fn run(input: u32, length: u32, output: u32, cap: u32, result: u32) {
    unsafe {
        assert!(super::service().phase == super::Phase::Ready, "service not ready");
        let mut output = super::buffers::take(input, length, output, cap, result);
        let packet = telora_parse_data(input, length, 1, 0);
        let error = word(packet, 12);
        if error != 0 {
            let message = core::str::from_utf8(core::slice::from_raw_parts(
                word(error, 0) as *const u8, word(error, 4) as usize)).unwrap();
            failure(message, &mut output);
        } else {
            let materialize: unsafe extern "C" fn(u32, u32) -> u32 =
                core::mem::transmute(super::service().contract.materialize);
            let value = materialize(packet, 0);
            assert_ne!(value, 0, "validated request failed materialization");
            let args = [value];
            let reply = telora_invoke(super::service().handler, args.as_ptr() as u32);
            assert_ne!(reply, 0, "closed service entry failed to serialize diagnostics");
            output.extend_from_slice(crate::text::text(reply).as_bytes());
        }
        super::buffers::give(output, result);
    }
}
