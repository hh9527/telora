//! Runtime input slots follow the exact source roots retained by Wasm collection.
use std::collections::BTreeSet;
use telora_core::{SourceDatabase, SourceId};
use telora_wasm::session::Session;

#[derive(Default)]
pub(super) struct RuntimeSources {
    active: BTreeSet<SourceId>,
    free: Vec<SourceId>,
}

impl RuntimeSources {
    pub(super) fn add(
        &mut self,
        sources: &mut SourceDatabase,
        name: String,
        text: &str,
    ) -> Result<SourceId, String> {
        let id = if let Some(id) = self.free.pop() {
            sources
                .replace_unreferenced(id, name, text)
                .map_err(|e| e.to_string())?;
            id
        } else {
            sources.try_add(name, text).map_err(|e| e.to_string())?
        };
        self.active.insert(id);
        Ok(id)
    }

    /// Only call after successful collection and consumption of diagnostics.
    pub(super) fn collect(
        &mut self,
        sources: &mut SourceDatabase,
        session: &Session,
    ) -> Result<(), String> {
        let live = session
            .manifest
            .sources
            .iter()
            .map(|source| source.id)
            .collect::<BTreeSet<_>>();
        let dead = self
            .active
            .iter()
            .copied()
            .filter(|id| !live.contains(&id.get()))
            .collect::<Vec<_>>();
        for id in dead {
            sources
                .replace_unreferenced(id, "", "")
                .map_err(|e| e.to_string())?;
            self.active.remove(&id);
            self.free.push(id);
        }
        Ok(())
    }
}
