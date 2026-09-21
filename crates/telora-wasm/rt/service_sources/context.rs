//! Assemble only the fixed Context ABI supplied by sealed codegen.
use crate::{abi::*, tables::telora_table_push, telora_alloc};

unsafe fn put(pointer: u32, offset: u64, value: u32) {
    unsafe { crate::heap::write(pointer + offset as u32, value) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_service_context(ctx_ty: u32, dict_ty: u32, stride: u32, field_offset: u32) -> u32 {
    unsafe {
        if super::telora_service_sources_seal() != 0 { return 0; }
        assert!(stride >= HEADER_BYTES);
        let slots = &super::sources().slots;
        let length = u32::try_from(slots.len()).unwrap();
        let key_bytes = length.checked_mul(STRING_BYTES).unwrap();
        let value_bytes = length.checked_mul(stride).unwrap();
        let keys = telora_alloc(key_bytes);
        let values = telora_alloc(value_bytes);
        for (index, slot) in slots.iter().enumerate() {
            crate::heap::telora_heap_copy(keys + index as u32 * STRING_BYTES, slot.key, STRING_BYTES);
            crate::heap::telora_heap_copy(values + index as u32 * stride, slot.value, stride);
        }
        let keys = telora_table_push(table_address(ARRAYS), keys, key_bytes);
        let values = telora_table_push(table_address(ARRAYS), values, value_bytes);
        let bytes = field_offset.checked_add(STRING_BYTES).unwrap();
        let fields = telora_alloc(bytes);
        let dict = fields + field_offset;
        put(dict, DATA, keys);
        put(dict, DATA + 4, length);
        put(dict, DATA + 8, values);
        let object = telora_table_push(table_address(RECORDS), fields, bytes);
        let ctx = telora_alloc(SCALAR_BYTES);
        put(ctx, DATA, object);
        ctx
    }
}
