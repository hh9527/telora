//! Two location tables: immutable artifact bytes and initialization records.
use crate::locations::{LocId, LocationIndex, LocationRecord, RECORD_BYTES};
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocationTableError {
    InvalidStaticTable,
    Capacity,
    Frozen,
    Missing,
}

pub struct LocationTables<'a> {
    static_bytes: &'a [u8],
    initialization: Vec<[u8; RECORD_BYTES as usize]>,
    frozen: bool,
}

impl<'a> LocationTables<'a> {
    pub fn new(static_bytes: &'a [u8]) -> Result<Self, LocationTableError> {
        let size = RECORD_BYTES as usize;
        if static_bytes.len() % size != 0
            || static_bytes.len() / size > crate::locations::STATIC_BIT as usize
        {
            return Err(LocationTableError::InvalidStaticTable);
        }
        Ok(Self {
            static_bytes,
            initialization: Vec::new(),
            frozen: false,
        })
    }

    pub fn append(&mut self, record: LocationRecord) -> Result<LocId, LocationTableError> {
        if self.frozen {
            return Err(LocationTableError::Frozen);
        }
        let index =
            u32::try_from(self.initialization.len()).map_err(|_| LocationTableError::Capacity)?;
        let id = LocId::initialization_index(index).ok_or(LocationTableError::Capacity)?;
        self.initialization
            .try_reserve(1)
            .map_err(|_| LocationTableError::Capacity)?;
        self.initialization.push(record.encode());
        Ok(id)
    }

    pub fn initialization_storage(&self) -> &[[u8; RECORD_BYTES as usize]] {
        &self.initialization
    }

    pub fn freeze(&mut self) {
        self.frozen = true;
    }

    pub fn get(&self, id: LocId) -> Result<Option<LocationRecord>, LocationTableError> {
        Ok(self.bytes(id)?.and_then(LocationRecord::decode))
    }

    pub fn bytes(&self, id: LocId) -> Result<Option<&[u8]>, LocationTableError> {
        let bytes = match id.index() {
            LocationIndex::None => return Ok(None),
            LocationIndex::Static(index) => {
                let offset = (index as usize)
                    .checked_mul(RECORD_BYTES as usize)
                    .ok_or(LocationTableError::Missing)?;
                let end = offset
                    .checked_add(RECORD_BYTES as usize)
                    .ok_or(LocationTableError::Missing)?;
                self.static_bytes.get(offset..end)
            }
            LocationIndex::Initialization(index) => {
                self.initialization.get(index as usize).map(|r| &r[..])
            }
        };
        bytes.map(Some).ok_or(LocationTableError::Missing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialization_growth_and_freeze_preserve_both_tables() {
        let static_record = LocationRecord {
            source: 5,
            start_line: 70_000,
            ..Default::default()
        };
        let bytes = static_record.encode();
        let mut tables = LocationTables::new(&bytes).unwrap();
        let initial_record = LocationRecord {
            source: 9,
            end_offset: 55,
            ..Default::default()
        };
        let initial = tables.append(initial_record).unwrap();
        for offset in 0..1024 {
            tables
                .append(LocationRecord {
                    end_offset: offset,
                    ..initial_record
                })
                .unwrap();
        }
        tables.freeze();
        assert_eq!(
            tables.get(LocId::static_index(0).unwrap()),
            Ok(Some(static_record))
        );
        assert_eq!(tables.get(initial), Ok(Some(initial_record)));
        assert_eq!(tables.get(LocId::NONE), Ok(None));
        assert_eq!(
            tables.append(initial_record),
            Err(LocationTableError::Frozen)
        );
        assert_eq!(
            tables.get(LocId::static_index(1).unwrap()),
            Err(LocationTableError::Missing)
        );
        assert_eq!(
            tables.get(LocId::initialization_index(1025).unwrap()),
            Err(LocationTableError::Missing)
        );
        assert!(matches!(
            LocationTables::new(&bytes[..19]),
            Err(LocationTableError::InvalidStaticTable)
        ));
    }
}
