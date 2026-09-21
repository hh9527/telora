//! Assemble only the fixed Context ABI supplied by sealed codegen.
use crate::{abi::*, telora_alloc};

unsafe fn put(pointer: u32, offset: u64, value: u32) {
    unsafe { crate::heap::write(pointer + offset as u32, value) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn telora_service_context(ctx_ty: u32, _dict_ty: u32, stride: u32, field_offset: u32, string_ty: u32, value_ty: u32) -> u32 {
    unsafe {
        if super::telora_service_sources_seal() != 0 { return 0; }
        assert!(stride >= HEADER_BYTES);
        let slots = &super::sources().slots;
        let length = u32::try_from(slots.len()).unwrap();
        let key_bytes = length.checked_mul(STRING_BYTES).unwrap();
        let value_bytes = length.checked_mul(stride).unwrap();
        let keys = telora_alloc(16 + key_bytes);
        let values = telora_alloc(16 + value_bytes);
        put(keys, 0, string_ty);
        put(keys, 4, keys + 16);
        put(keys, 8, length);
        put(keys, 12, length);
        put(values, 0, value_ty);
        put(values, 4, values + 16);
        put(values, 8, length);
        put(values, 12, length);
        for (index, slot) in slots.iter().enumerate() {
            crate::heap::telora_heap_copy(keys + 16 + index as u32 * STRING_BYTES, slot.key, STRING_BYTES);
            crate::heap::telora_heap_copy(values + 16 + index as u32 * stride, slot.value, stride);
        }
        let bytes = field_offset.checked_add(STRING_BYTES).unwrap();
        let object = telora_alloc(8 + bytes);
        put(object, 0, ctx_ty);
        put(object, 4, bytes);
        let fields = object + 8;
        let dict = fields + field_offset;
        put(dict, DATA, keys);
        put(dict, DATA + 4, length);
        put(dict, DATA + 8, values);
        let ctx = telora_alloc(SCALAR_BYTES);
        put(ctx, DATA, object);
        ctx
    }
}
