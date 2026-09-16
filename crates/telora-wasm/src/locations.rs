//! Deterministic source-span identities shared by every generated instance.
use std::collections::BTreeMap;
use telora_core::{mir::Mir, source::Location};
use telora_wasm_shared::locations::{LocId, LocationRecord};

pub(crate) struct StaticLocations {
    ids: BTreeMap<Location, LocId>,
    pub bytes: Vec<u8>,
}

impl StaticLocations {
    pub fn new(mir: &Mir) -> Result<Self, String> {
        let mut locations = Self {
            ids: BTreeMap::new(),
            bytes: Vec::new(),
        };
        // HIR order is deterministic and does not multiply with generic instances.
        for node in &mir.hir {
            if locations.ids.contains_key(&node.location) {
                continue;
            }
            let id = LocId::static_index(
                u32::try_from(locations.ids.len())
                    .map_err(|_| "Wasm: too many static locations")?,
            )
            .ok_or("Wasm: too many static locations")?;
            let source = mir.sources.get(node.location.source);
            let start = source.utf8_position(node.location.start);
            let end = source.utf8_position(node.location.end);
            let record = LocationRecord {
                source: node.location.source.get(),
                start_line: start.line,
                start_offset: start.character,
                end_line: end.line,
                end_offset: end.character,
            };
            locations.bytes.extend_from_slice(&record.encode());
            locations.ids.insert(node.location, id);
        }
        Ok(locations)
    }

    pub fn id(&self, location: Location) -> LocId {
        self.ids[&location]
    }
}
