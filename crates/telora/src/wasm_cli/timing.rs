//! Opt-in wall-clock observations; never part of the command result stream.
pub(super) fn artifact_size(bytes: usize) {
    if std::env::var_os("TELORA_WASM_TIMINGS").as_deref() == Some(std::ffi::OsStr::new("1")) {
        eprintln!("{}", serde_json::json!({"wasm_phase": "artifact", "bytes": bytes}));
    }
}

pub(super) fn initialization_heap(session: &mut telora_wasm::session::Session) -> Result<(), String> {
    if std::env::var_os("TELORA_WASM_TIMINGS").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return Ok(());
    }
    let stats = session.initialization_stats()?;
    if stats.demand_roots != 0 {
        eprintln!("{}", serde_json::json!({"wasm_phase": "initialization_heap",
            "before_bytes": stats.heap_before, "after_bytes": stats.heap_after,
            "demand_roots": stats.demand_roots,
            "linear_memory_before_gc_bytes": stats.memory_before,
            "linear_memory_high_water_bytes": stats.memory_high_water}));
    }
    Ok(())
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
