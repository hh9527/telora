//! Opt-in wall-clock observations; never part of the command result stream.
pub(super) fn collection(
    stats: &telora_wasm::collection::CollectionStats,
    source_slots: usize,
    live_sources: usize,
    output_bytes: usize,
) {
    if std::env::var_os("TELORA_WASM_TIMINGS").as_deref() == Some(std::ffi::OsStr::new("1")) {
        eprintln!(
            "{}",
            serde_json::json!({
                "wasm_observation": "service_memory",
                "heap_before": stats.heap_before,
                "heap_after": stats.heap_after,
                "memory_bytes": stats.memory_bytes,
                "source_slots": source_slots,
                "live_sources": live_sources,
                "buffered_output_bytes": output_bytes,
            })
        );
    }
}

pub(super) struct PhaseTimer {
    name: &'static str,
    start: Option<std::time::Instant>,
}

impl PhaseTimer {
    pub(super) fn new(name: &'static str) -> Self {
        Self {
            name,
            start: (std::env::var_os("TELORA_WASM_TIMINGS").as_deref()
                == Some(std::ffi::OsStr::new("1")))
            .then(std::time::Instant::now),
        }
    }
}

impl Drop for PhaseTimer {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            eprintln!(
                "{}",
                serde_json::json!({
                    "wasm_phase": self.name,
                    "elapsed_ns": start.elapsed().as_nanos(),
                })
            );
        }
    }
}
