//! Fixed parse-plan ABI. Node IDs are postorder indices, never TypeIds.
//! Result: {rows, count, root, error_span}, four u32 words.
//! Row (16 bytes): {kind:u32, reserved:u32, payload:u64}.
//! Payload is scalar bits or {pointer:u32,count:u32}. Object entries are
//! {key_pointer,key_length,child_id}; Array entries are child IDs.
use alloc::{boxed::Box, format, string::{String, ToString}};
use telora_data::{
    data_plan::{
        self, DataPlanNodeKind as Node, DataScalar as Scalar, Format, TemporalKind,
        ValidatedDataPlan as Plan,
    },
    source::SourceDatabase,
};

fn parse(input: &str, format: Format) -> Result<Plan, String> {
    let mut sources = SourceDatabase::default();
    let source = sources.try_add("<string>", input).map_err(|error| error.to_string())?;
    let plan = data_plan::parse_registered(&sources, source, format).map_err(|diagnostics| {
        diagnostics
            .into_iter()
            .next()
            .expect("parse diagnostic")
            .message
    })?;
    Ok(plan)
}

unsafe fn put(pointer: u32, offset: u32, value: u32) {
    unsafe {
        ((pointer + offset) as *mut u32).write_unaligned(value);
    }
}

fn string_bytes(value: String) -> (u32, u32) {
    let bytes = Box::leak(value.into_bytes().into_boxed_slice());
    (bytes.as_mut_ptr() as u32, bytes.len() as u32)
}

/// Parse text without constructing any language values. The generated caller
/// supplies all final type identities, headers, and container layouts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_json_parse(input: u32) -> u32 {
    unsafe {
        export_plan(
            parse(crate::text::text(input), Format::Json)
                .map_err(|error| format!("<json string>: {error}")),
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_toml_parse(input: u32) -> u32 {
    unsafe {
        export_plan(
            parse(crate::text::text(input), Format::Toml)
                .map_err(|error| format!("<toml string>: {error}")),
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_yaml_parse(input: u32) -> u32 {
    unsafe {
        export_plan(
            parse(crate::text::text(input), Format::Yaml)
                .map_err(|error| format!("<yaml string>: {error}")),
        )
    }
}

unsafe fn export_plan(parsed: Result<Plan, String>) -> u32 {
    unsafe {
        let result = crate::telora_alloc(16);
        for offset in [0, 4, 8, 12] {
            put(result, offset, 0);
        }
        let plan = match parsed {
            Ok(plan) => plan,
            Err(message) => {
                let span = crate::format::render_with(|out| out.write_str(&message));
                put(result, 12, span);
                return result;
            }
        };
        let plan = plan.into_postorder();
        let root = plan.root_node().expect("parsed root").index() as u32;
        let nodes = plan.into_nodes();
        let count = u32::try_from(nodes.len()).unwrap();
        let rows = crate::telora_alloc(count.checked_mul(16).unwrap());
        put(result, 0, rows);
        put(result, 4, count);
        put(result, 8, root);
        for (index, node) in nodes.into_iter().enumerate() {
            let row = rows + index as u32 * 16;
            put(row, 4, 0);
            let (kind, payload) = match node.kind {
                Node::Scalar(Scalar::Null) => (0, 0),
                Node::Scalar(Scalar::Bool(value)) => (if value { 1 } else { 2 }, 0),
                Node::Scalar(Scalar::Int(value)) => (3, value as u64),
                Node::Scalar(Scalar::Float(value)) => (4, value.to_bits()),
                Node::Scalar(Scalar::String(value)) => {
                    let (pointer, length) = string_bytes(value);
                    (5, u64::from(pointer) | (u64::from(length) << 32))
                }
                Node::Scalar(Scalar::Temporal { kind, value }) => {
                    let kind = match kind {
                        TemporalKind::LocalDate => 8,
                        TemporalKind::LocalTime => 9,
                        TemporalKind::LocalDateTime => 10,
                        TemporalKind::OffsetDateTime => 11,
                    };
                    let (pointer, length) = string_bytes(value);
                    (kind, u64::from(pointer) | (u64::from(length) << 32))
                }
                Node::Scalar(Scalar::Bytes(bytes)) => {
                    let bytes = Box::leak(bytes.into_boxed_slice());
                    (12, bytes.as_mut_ptr() as u64 | ((bytes.len() as u64) << 32))
                }
                Node::Array(children) => {
                    let children = Box::leak(
                        children
                            .into_iter()
                            .map(|id| id.index() as u32)
                            .collect::<alloc::vec::Vec<_>>()
                            .into_boxed_slice(),
                    );
                    (
                        6,
                        children.as_mut_ptr() as u64 | ((children.len() as u64) << 32),
                    )
                }
                Node::Object(fields) => {
                    let count = u32::try_from(fields.len()).unwrap();
                    let entries = crate::telora_alloc(count.checked_mul(12).unwrap());
                    for (index, (key, child)) in fields.into_iter().enumerate() {
                        let entry = entries + index as u32 * 12;
                        let (pointer, length) = string_bytes(key);
                        put(entry, 0, pointer);
                        put(entry, 4, length);
                        put(entry, 8, child.value.index() as u32);
                    }
                    (7, u64::from(entries) | (u64::from(count) << 32))
                }
            };
            put(row, 0, kind);
            ((row + 8) as *mut u64).write_unaligned(payload);
        }
        result
    }
}
