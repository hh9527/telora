/// Report entered phases on success or error without changing the operation result.
pub(super) fn observe<T>(
    phase: &'static str,
    observer: &mut dyn FnMut(&'static str, std::time::Duration),
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let started = std::time::Instant::now();
    let result = operation();
    observer(phase, started.elapsed());
    result
}
