//! Opt-in wall-clock observations; never part of the command result stream.
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
