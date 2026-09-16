//! Source-backed parsing stores byte spans; ordinary parsing inherits its input.
use telora_data::source::{LineIndex, Location};
use telora_wasm_shared::source_range::SourceRange;

pub(super) enum Origins {
    Inherit(SourceRange),
    Source { id: u32 },
}

impl Origins {
    pub fn new(source: u32, input: &str) -> Self {
        if source == 0 {
            Self::Inherit(SourceRange::NONE)
        } else {
            let lines = LineIndex::new(input).unwrap();
            let ranges = lines.ranges().collect::<alloc::vec::Vec<_>>();
            unsafe { crate::sources::telora_source_index(source, ranges.as_ptr() as u32, ranges.len() as u32); }
            Self::Source { id: source }
        }
    }

    pub unsafe fn write(&self, pointer: u32, location: Location) {
        let range = match self {
            Self::Inherit(range) => *range,
            Self::Source { id } => SourceRange { source: *id, start: location.start, end: location.end },
        };
        unsafe { core::ptr::copy_nonoverlapping(range.encode().as_ptr(), pointer as *mut u8, 12); }
    }

    pub unsafe fn inherit(pointer: u32) -> Self {
        unsafe { Self::Inherit(SourceRange {
            source: crate::values::word(pointer, 0),
            start: crate::values::word(pointer, 4),
            end: crate::values::word(pointer, 8),
        }) }
    }
}
