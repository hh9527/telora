//! Closed physical layout metadata shared by codegen, Guest tracing and Host sealing.

pub const ENTRY_BYTES: u32 = 32;
pub const DETAIL_BYTES: u32 = 12;

pub const KIND: u32 = 0;
pub const VALUE_BYTES: u32 = 4;
pub const DATA_BYTES: u32 = 8;
pub const ALIGNMENT: u32 = 12;
pub const RESOURCE_TABLE: u32 = 16;
pub const DETAIL_OFFSET: u32 = 20;
pub const DETAIL_COUNT: u32 = 24;
pub const FLAGS: u32 = 28;

pub const DETAIL_TYPE: u32 = 0;
pub const DETAIL_OFFSET_OR_TAG: u32 = 4;
pub const DETAIL_FLAGS: u32 = 8;
pub const DETAIL_BOXED: u32 = 1;

pub const NO_TYPE: u32 = u32::MAX;
pub const NO_RESOURCE: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Kind {
    Scalar = 0,
    String = 1,
    Bytes = 2,
    Record = 3,
    Array = 4,
    Dict = 5,
    Enum = 6,
    Dyn = 7,
    Function = 8,
    Resource = 9,
    Newtype = 10,
    CompileTime = 11,
}

impl Kind {
    pub fn decode(value: u32) -> Option<Self> {
        Some(match value {
            0 => Self::Scalar,
            1 => Self::String,
            2 => Self::Bytes,
            3 => Self::Record,
            4 => Self::Array,
            5 => Self::Dict,
            6 => Self::Enum,
            7 => Self::Dyn,
            8 => Self::Function,
            9 => Self::Resource,
            10 => Self::Newtype,
            11 => Self::CompileTime,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub kind: Kind,
    pub value_bytes: u32,
    pub data_bytes: u32,
    pub alignment: u32,
    pub resource_table: Option<u32>,
    pub detail_offset: u32,
    pub detail_count: u32,
    pub flags: u32,
}

impl Entry {
    pub fn decode(image: &[u8], type_id: u32) -> Option<Self> {
        let base = usize::try_from(type_id.checked_mul(ENTRY_BYTES)?).ok()?;
        let word = |offset: u32| {
            let at = base.checked_add(offset as usize)?;
            Some(u32::from_le_bytes(image.get(at..at + 4)?.try_into().ok()?))
        };
        let kind = Kind::decode(word(KIND)?)?;
        let resource = word(RESOURCE_TABLE)?;
        let result = Self {
            kind,
            value_bytes: word(VALUE_BYTES)?,
            data_bytes: word(DATA_BYTES)?,
            alignment: word(ALIGNMENT)?,
            resource_table: (resource != NO_RESOURCE).then_some(resource),
            detail_offset: word(DETAIL_OFFSET)?,
            detail_count: word(DETAIL_COUNT)?,
            flags: word(FLAGS)?,
        };
        let details = result
            .detail_offset
            .checked_add(result.detail_count.checked_mul(DETAIL_BYTES)?)?;
        if (result.value_bytes == 0) != (result.kind == Kind::CompileTime)
            || !result.alignment.is_power_of_two()
            || details as usize > image.len()
        {
            return None;
        }
        Some(result)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Detail {
    pub ty: Option<u32>,
    pub offset_or_tag: u32,
    pub flags: u32,
}

impl Detail {
    pub fn decode(image: &[u8], entry: Entry, index: u32) -> Option<Self> {
        if index >= entry.detail_count {
            return None;
        }
        let base = entry
            .detail_offset
            .checked_add(index.checked_mul(DETAIL_BYTES)?)? as usize;
        let word = |offset: u32| {
            let at = base.checked_add(offset as usize)?;
            Some(u32::from_le_bytes(image.get(at..at + 4)?.try_into().ok()?))
        };
        let ty = word(DETAIL_TYPE)?;
        Some(Self {
            ty: (ty != NO_TYPE).then_some(ty),
            offset_or_tag: word(DETAIL_OFFSET_OR_TAG)?,
            flags: word(DETAIL_FLAGS)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn rejects_truncated_entries_and_details() {
        assert_eq!(Entry::decode(&[], 0), None);
        let mut image = vec![0; ENTRY_BYTES as usize];
        image[VALUE_BYTES as usize..VALUE_BYTES as usize + 4].copy_from_slice(&16u32.to_le_bytes());
        image[ALIGNMENT as usize..ALIGNMENT as usize + 4].copy_from_slice(&8u32.to_le_bytes());
        image[RESOURCE_TABLE as usize..RESOURCE_TABLE as usize + 4]
            .copy_from_slice(&NO_RESOURCE.to_le_bytes());
        image[DETAIL_OFFSET as usize..DETAIL_OFFSET as usize + 4]
            .copy_from_slice(&ENTRY_BYTES.to_le_bytes());
        image[DETAIL_COUNT as usize..DETAIL_COUNT as usize + 4]
            .copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(Entry::decode(&image, 0), None);
    }
}
