use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=rt");
    println!("cargo:rerun-if-changed=../telora-sha256");
    println!("cargo:rerun-if-changed=../telora-wasm-shared");
    println!("cargo:rerun-if-changed=../telora-data");
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let target_dir = output_dir.join("rt-target");
    let status = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args([
            "build",
            "--manifest-path",
            "rt/Cargo.toml",
            "--locked",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
            "--target-dir",
        ])
        .arg(&target_dir)
        .status()
        .expect("launch Cargo for Wasm RT");
    assert!(
        status.success(),
        "Wasm RT build failed; install rustup target add wasm32-unknown-unknown"
    );
    std::fs::copy(
        target_dir.join("wasm32-unknown-unknown/release/libtelora_wasm_rt.a"),
        output_dir.join("telora-rt.a"),
    )
    .expect("copy linked Wasm RT archive");
    let sysroot = Command::new(&rustc)
        .args(["--print", "sysroot"])
        .output()
        .expect("read Rust sysroot");
    assert!(sysroot.status.success());
    let linker = PathBuf::from(String::from_utf8(sysroot.stdout).unwrap().trim())
        .join("lib/rustlib")
        .join(env::var("HOST").unwrap())
        .join("bin/gcc-ld/wasm-ld");
    let help = Command::new(&linker)
        .arg("--help")
        .output()
        .unwrap_or_else(|error| panic!("Failed to probe {} --help: {error}", linker.display()));
    assert!(
        help.status.success(),
        "Failed to probe {} --help ({}):\n{}\n{}",
        linker.display(),
        help.status,
        String::from_utf8_lossy(&help.stdout),
        String::from_utf8_lossy(&help.stderr),
    );
    // Older LLD defaults to placing the stack after static data and has no
    // inverse flag. Newer LLD supports the flag and defaults to stack-first.
    let no_stack_first = String::from_utf8_lossy(&help.stdout)
        .split_whitespace()
        .any(|word| word == "--no-stack-first");
    let exports = "get-data-source-count get-data-source-name set-data-source create-service run-service get-service-diagnostics telora_service_bootstrap mem-alloc mem-free mem-realloc telora_alloc telora_invoke telora_table_push telora_table_get telora_freeze
        telora_string_compare telora_source_name telora_subject_label telora_sort_pairs
        telora_duplicate_key_message telora_text_query telora_text_build telora_text_split
        telora_path telora_format_render telora_format_message telora_format_join
        telora_template_prepare telora_member_message telora_regex telora_hash
        telora_service_context telora_service_sources_prepare telora_service_source_count telora_service_source_name
        telora_service_source_parse telora_service_source_store telora_service_sources_seal
        telora_parse_data telora_json_write telora_json_parse telora_toml_parse telora_yaml_parse
        telora_float_remainder telora_register_source telora_source_retained telora_collect telora_heap_end
        telora_reserve_static telora_locations_bootstrap telora_location_get telora_location_add __heap_base __indirect_function_table";
    let status = Command::new(&linker)
        .args([
            "--no-entry",
            "--export-memory",
            "--global-base=512",
            "--strip-debug",
        ])
        .args(no_stack_first.then_some("--no-stack-first"))
        .args(
            exports
                .split_whitespace()
                .map(|name| format!("--export={name}")),
        )
        .arg(output_dir.join("telora-rt.a"))
        .arg("-o")
        .arg(output_dir.join("telora-rt.wasm"))
        .status()
        .expect("prelink Wasm runtime template");
    assert!(
        status.success(),
        "Wasm runtime template linking failed: {} ({status})",
        linker.display(),
    );
}
