//! Public byte ABI only: no type descriptors or language-value decoding.
use crate::session::Session;

pub(super) fn alloc(session: &mut Session, cap: u32, align: u32) -> Result<u32, String> {
    session
        .exports
        .alloc
        .call(&mut session.store, (cap, align))
        .map_err(|e| e.to_string())
}

pub(super) fn free(
    session: &mut Session,
    pointer: u32,
    cap: u32,
    align: u32,
) -> Result<(), String> {
    session
        .exports
        .free
        .call(&mut session.store, (pointer, cap, align))
        .map_err(|e| e.to_string())
}

pub(super) fn words(session: &Session, pointer: u32) -> Result<[u32; 3], String> {
    let mut bytes = [0; 12];
    session
        .memory
        .read(&session.store, pointer as usize, &mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(core::array::from_fn(|index| {
        u32::from_le_bytes(bytes[index * 4..index * 4 + 4].try_into().unwrap())
    }))
}

pub(super) fn bytes(session: &Session, pointer: u32, length: u32) -> Result<Vec<u8>, String> {
    let start = pointer as usize;
    let end = start
        .checked_add(length as usize)
        .ok_or("service output range overflow")?;
    session
        .memory
        .data(&session.store)
        .get(start..end)
        .map(|bytes| bytes.to_vec())
        .ok_or_else(|| "invalid service output range".into())
}

/// Consume returned ownership before any reset can reclaim the allocation.
pub(super) fn response(session: &mut Session, result: u32) -> Result<Vec<u8>, String> {
    let [pointer, length, cap] = words(session, result)?;
    if length > cap {
        return Err("service output length exceeds capacity".into());
    }
    let bytes = bytes(session, pointer, length)?;
    free(session, pointer, cap, 1)?;
    free(session, result, 12, 4)?;
    Ok(bytes)
}
