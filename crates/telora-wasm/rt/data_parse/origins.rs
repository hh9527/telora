//! Initialization registers exact spans; ordinary parsing only inherits an ID.
use telora_data::source::{LineIndex, Location};
use telora_wasm_shared::locations::LocationRecord;

pub(super) enum Origins {
    Inherit(u32),
    Source { id: u32, lines: LineIndex },
}

impl Origins {
    pub fn new(source: u32, input: &str) -> Self {
        if source == 0 {
            Self::Inherit(0)
        } else {
            Self::Source { id: source, lines: LineIndex::new(input).unwrap() }
        }
    }

    pub unsafe fn at(&self, location: Location) -> u32 {
        match self {
            Self::Inherit(id) => *id,
            Self::Source { id, lines } => {
                let start = lines.point(location.start);
                let end = lines.point(location.end);
                unsafe { crate::locations::append(LocationRecord {
                    source: *id,
                    start_line: (start >> 32) as u32,
                    start_offset: start as u32,
                    end_line: (end >> 32) as u32,
                    end_offset: end as u32,
                }) }
            }
        }
    }
}
