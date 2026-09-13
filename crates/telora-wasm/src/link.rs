//! Static linking belongs to compilation, never artifact loading or execution.
use std::process::Command;

pub(crate) fn link(object: &[u8], reserved_bytes: u32) -> Result<Vec<u8>, String> {
    let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
    let input = directory.path().join("program.o");
    let runtime = directory.path().join("runtime.a");
    let output = directory.path().join("program.wasm");
    std::fs::write(&input, object).map_err(|e| e.to_string())?;
    std::fs::write(
        &runtime,
        include_bytes!(concat!(env!("OUT_DIR"), "/telora-rt.a")),
    )
    .map_err(|e| e.to_string())?;
    // The ABI's table descriptors and graph demand slots own this low-memory
    // prefix. Rust static data, stack and heap must all follow it.
    let base = reserved_bytes
        .checked_add(15)
        .ok_or("Wasm: static size overflow")?
        & !15;
    let linker =
        std::env::var_os("TELORA_WASM_LD").unwrap_or_else(|| env!("TELORA_BUILT_WASM_LD").into());
    let result = Command::new(linker)
        .args([
            "--no-entry",
            "--no-stack-first",
            "--export-memory",
            "--export=telora_alloc",
            "--export=telora_invoke",
            "--export=telora_table_push",
            "--export=telora_initialize",
            "--export=telora_entry",
            "--export=telora_inject_data",
            "--export=telora_error",
            "--export=telora_register_source",
            "--export=telora_source_name",
        ])
        .arg(format!("--global-base={base}"))
        .arg(&input)
        .arg(&runtime)
        .arg("-o")
        .arg(&output)
        .output()
        .map_err(|e| format!("Wasm linker: {e}"))?;
    if !result.status.success() {
        let saved = directory.keep();
        return Err(format!(
            "Wasm linker (inputs preserved at {}): {}",
            saved.display(),
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    std::fs::read(output).map_err(|e| e.to_string())
}
