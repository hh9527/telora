pub(super) struct Timer {
    phase: &'static str,
    start: Option<std::time::Instant>,
}
impl Timer {
    pub fn new(phase: &'static str) -> Self {
        Self {
            phase,
            start: std::env::var_os("TELORA_WASM_TIMINGS").map(|_| std::time::Instant::now()),
        }
    }
}
impl Drop for Timer {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            eprintln!(
                "{{\"elapsed_ns\":{},\"wasm_phase\":\"{}\"}}",
                start.elapsed().as_nanos(),
                self.phase
            );
        }
    }
}
