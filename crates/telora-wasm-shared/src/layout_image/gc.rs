//! Static reference classification; no heap contents or source names are inspected.
use super::{DETAIL_BOXED, Detail, Entry, Kind};
use alloc::{vec, vec::Vec};

/// Ordinary values contain String/Bytes/Array/Dict references, transitively.
pub const GC_REFERENCES: u32 = 1;
/// Representation owns storage (closure environments, resources or boxes).
pub const INTERNAL_REFERENCES: u32 = 2;
/// The concrete environment/payload determines the references at runtime.
pub const DYNAMIC_REFERENCES: u32 = 4;
pub const REFERENCE_FLAGS: u32 = GC_REFERENCES | INTERNAL_REFERENCES | DYNAMIC_REFERENCES;

/// Compute the least fixed point over closed layouts, including recursive types.
/// Signature arguments are not environment fields: Function and Dyn are dynamic.
pub fn classify(image: &[u8], count: usize) -> Option<Vec<u32>> {
    let entries = (0..count)
        .map(|id| Entry::decode(image, u32::try_from(id).ok()?))
        .collect::<Option<Vec<_>>>()?;
    let mut flags = vec![0; count];
    let mut consumers = vec![Vec::new(); count];
    for (id, entry) in entries.iter().enumerate() {
        flags[id] = match entry.kind {
            Kind::String | Kind::Bytes | Kind::Array | Kind::Dict => GC_REFERENCES,
            Kind::Dyn | Kind::Function => INTERNAL_REFERENCES | DYNAMIC_REFERENCES,
            Kind::Resource => INTERNAL_REFERENCES,
            _ => 0,
        };
        for index in 0..entry.detail_count {
            let detail = Detail::decode(image, *entry, index)?;
            if detail.flags & DETAIL_BOXED != 0 {
                flags[id] |= INTERNAL_REFERENCES;
            }
            if let Some(ty) = detail.ty {
                let child = usize::try_from(ty).ok()?;
                consumers.get_mut(child)?.push(id);
            }
        }
    }
    let mut pending = (0..count).filter(|&id| flags[id] != 0).collect::<Vec<_>>();
    let mut queued = flags.iter().map(|&flags| flags != 0).collect::<Vec<_>>();
    while let Some(child) = pending.pop() {
        queued[child] = false;
        for &parent in &consumers[child] {
            let next = flags[parent] | flags[child];
            if next != flags[parent] {
                flags[parent] = next;
                if !queued[parent] {
                    queued[parent] = true;
                    pending.push(parent);
                }
            }
        }
    }
    Some(flags)
}

#[cfg(test)]
mod tests;
