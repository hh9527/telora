//! Observational event transport. No user code, inference or value copies.
use crate::{abi::*, emit::Emitter, output::Output, session::Session};
use telora_core::mir::HirId;
use wasm_encoder::{BlockType, Instruction as I};

#[derive(Clone, Debug, serde::Serialize)]
pub struct DebugEvent {
    pub name: String,
    pub repr: String,
    pub module: String,
    pub line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Emitter<'_> {
    pub fn debug_value(&mut self, node: HirId, input: u32) -> Result<u32, String> {
        self.extend([
            I::I32Const(DEBUG_ENABLED as i32),
            I::I32Load(memory(0, 2)),
            I::If(BlockType::Empty),
        ]);
        let event = self.alloc(8);
        self.store32(event, 0, node.index() as u32);
        self.extend([
            I::LocalGet(event),
            I::LocalGet(input),
            I::I32Store(memory(4, 2)),
        ]);
        self.table_push(DEBUG_EVENTS, event, 8);
        self.emit(I::End);
        Ok(input)
    }
}

impl Session {
    pub fn set_debug_enabled(&mut self, enabled: bool) -> Result<(), String> {
        self.write(DEBUG_ENABLED as usize, &u32::from(enabled).to_le_bytes())
    }
    pub fn debug_events(&self) -> Result<Vec<DebugEvent>, String> {
        self.debug_events_from(0)
    }
    pub fn take_debug_events(&self) -> Result<Vec<DebugEvent>, String> {
        let events = self.debug_events_from(self.emitted_debug.get())?;
        self.emitted_debug
            .set(self.emitted_debug.get() + events.len() as u32);
        Ok(events)
    }
    fn debug_events_from(&self, start: u32) -> Result<Vec<DebugEvent>, String> {
        let output = Output {
            memory: self.memory.data(&self.store),
            manifest: &self.manifest,
        };
        let count = output.word(table_address(DEBUG_EVENTS) as u64 + 4)?;
        let mut events = Vec::new();
        for index in start..count {
            let (pointer, bytes) = output.payload(DEBUG_EVENTS, index)?;
            if bytes != 8 {
                return Err("Wasm: invalid debug event size".into());
            }
            let node = output.word(pointer)?;
            let site = self
                .manifest
                .debug_sites
                .iter()
                .find(|site| site.node == node)
                .ok_or("Wasm: unknown debug site")?;
            let value = output.word(pointer + 4)? as u64;
            let loc = output.location(site.origin)?.ok_or("Wasm: debug site has no location")?;
            let source = self.manifest.sources.iter().find(|source| source.id == loc[0])
                .ok_or("Wasm: debug site has no source")?;
            events.push(DebugEvent {
                name: site.name.clone(),
                repr: output.debug_repr(value)?,
                module: source.name.clone(),
                line: loc[1].checked_add(1).ok_or("Wasm: debug line overflow")?,
                message: site.message.clone(),
            });
        }
        Ok(events)
    }
}
