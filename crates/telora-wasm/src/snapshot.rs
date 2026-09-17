//! Preinitialize with our existing wasmi host; Wizer only rewrites the module.
use crate::session::Session;
pub use wasmtime_wizer::ModuleContext;
use wasmtime_wizer::{InstanceState, SnapshotVal, ValType, Wizer};

pub fn instrument(bytes: &[u8]) -> Result<(ModuleContext<'_>, Vec<u8>), String> {
    Wizer::new().instrument(bytes).map_err(|e| e.to_string())
}

pub(crate) fn capture(
    context: &ModuleContext<'_>,
    session: &mut Session,
) -> Result<Vec<u8>, String> {
    futures_executor::block_on(Wizer::new().snapshot(context, session)).map_err(|e| e.to_string())
}

impl InstanceState for Session {
    async fn global_get(&mut self, name: &str, _: ValType) -> SnapshotVal {
        match self
            .instance
            .get_global(&self.store, name)
            .expect("Wizer exported global")
            .get(&self.store)
        {
            wasmi::Val::I32(v) => SnapshotVal::I32(v),
            wasmi::Val::I64(v) => SnapshotVal::I64(v),
            wasmi::Val::F32(v) => SnapshotVal::F32(v.to_bits()),
            wasmi::Val::F64(v) => SnapshotVal::F64(v.to_bits()),
            _ => unreachable!(
                "Wizer validation rejects reference globals; current RT uses scalar globals"
            ),
        }
    }

    async fn memory_contents(&mut self, name: &str, contents: impl FnOnce(&[u8]) + Send) {
        let memory = self
            .instance
            .get_memory(&self.store, name)
            .expect("Wizer exported memory");
        contents(memory.data(&self.store));
    }
}
