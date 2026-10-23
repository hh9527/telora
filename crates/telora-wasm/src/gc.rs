//! Execution collection policy and explicit feedback ownership.
use crate::session::Session;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GcConfig {
    /// Minimum allocation interval. After collection the threshold grows with
    /// the live graph and is capped by remaining managed budget. One forces every
    /// safe point; zero disables automatic collection. Explicit roots still apply.
    pub allocation_interval: u32,
    /// Aligned object storage (including RawString/Bytes) and registry capacity.
    /// Excludes hidden default-allocator overhead, bounded by Guest memory.
    /// Other Guest allocations are additionally bounded by the Session memory limit.
    pub heap_limit: u32,
}

/// Guest allocation telemetry; retained linear memory is bounded by the engine.
#[derive(Clone, Copy, Debug)]
pub struct GcStats {
    pub managed_bytes: u32,
    /// Aligned storage plus registry capacity; name retained for API compatibility.
    pub words_committed: u32,
    pub allocated_total: u64,
    pub collections: u32,
    pub memory_bytes: usize,
}

impl Default for GcConfig {
    fn default() -> Self {
        Self {
            allocation_interval: 64 * 1024,
            heap_limit: 32 * 1024 * 1024,
        }
    }
}

impl Session {
    /// Applies before initialization or between invocations, and survives service
    /// snapshot replacement. A rejected smaller budget leaves the policy intact.
    pub fn configure_gc(&mut self, config: GcConfig) -> Result<(), String> {
        let limit = self
            .instance
            .get_typed_func::<u32, u32>(&self.store, "telora_gc_heap_limit")
            .map_err(|error| error.to_string())?;
        if self.guest_call(limit, config.heap_limit)? != 0 {
            return Err("Wasm: GC budget is below retained heap usage".into());
        }
        let interval = self
            .instance
            .get_typed_func::<u32, u32>(&self.store, "telora_gc_interval")
            .map_err(|error| error.to_string())?;
        self.guest_call(interval, config.allocation_interval)?;
        self.gc_config = config;
        Ok(())
    }

    pub fn gc_stats(&mut self) -> Result<GcStats, String> {
        let mut word = |name| -> Result<u32, String> {
            let metric = self
                .instance
                .get_typed_func::<(), u32>(&self.store, name)
                .map_err(|error| error.to_string())?;
            self.guest_call(metric, ())
        };
        let managed_bytes = word("telora_heap_bytes")?;
        let words_committed = word("telora_heap_committed")?;
        let collections = word("telora_gc_collections")?;
        let total = self
            .instance
            .get_typed_func::<(), u64>(&self.store, "telora_gc_allocated_total")
            .map_err(|error| error.to_string())?;
        let allocated_total = self.guest_call(total, ())?;
        Ok(GcStats {
            managed_bytes,
            words_committed,
            collections,
            allocated_total,
            memory_bytes: self.memory.data_size(&self.store),
        })
    }

    /// Collect at a Host boundary. Every borrowed value kept for a later call
    /// must have a pin; references remain stable during this nonmoving collection.
    pub fn collect_garbage(&mut self) -> Result<(), String> {
        let collect = self
            .instance
            .get_typed_func::<(), u32>(&self.store, "telora_gc_collect")
            .map_err(|error| error.to_string())?;
        self.guest_call(collect, ())?;
        self.prune_sources()
    }

    /// Discard Guest diagnostic/debug history after the Host has copied it.
    /// The active failure remains visible; Failed demands retain their own identity.
    /// Read-only history APIs retain events until this method is called.
    pub fn release_feedback(&mut self) -> Result<(), String> {
        let release = self
            .instance
            .get_typed_func::<u32, u32>(&self.store, "telora_gc_release_feedback")
            .map_err(|error| error.to_string())?;
        let active = self
            .instance
            .get_global(&self.store, "telora_error")
            .ok_or("Wasm: missing failure global")?
            .get(&self.store)
            .i32()
            .ok_or("Wasm: invalid failure global")? as u32;
        self.guest_call(release, active)?;
        self.emitted_debug.set(0);
        Ok(())
    }
}
