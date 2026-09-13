use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=rt");
    println!("cargo:rerun-if-env-changed=TELORA_WASM_LD");
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
    let linker = env::var_os("TELORA_WASM_LD")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(String::from_utf8(sysroot.stdout).unwrap().trim())
                .join("lib/rustlib")
                .join(env::var("HOST").unwrap())
                .join("bin/gcc-ld/wasm-ld")
        });
    println!("cargo:rustc-env=TELORA_BUILT_WASM_LD={}", linker.display());
}
