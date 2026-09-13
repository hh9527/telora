use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=rt");
    println!("cargo:rerun-if-env-changed=TELORA_WASM_LD");
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("telora-rt.a");
    let status = Command::new(&rustc)
        .args([
            "--edition=2024",
            "--target",
            "wasm32-unknown-unknown",
            "--crate-type",
            "staticlib",
            "-C",
            "opt-level=2",
            "-C",
            "panic=abort",
            "rt/lib.rs",
            "-o",
        ])
        .arg(output)
        .status()
        .expect("launch rustc for Wasm RT");
    assert!(
        status.success(),
        "Wasm RT build failed; install rustup target add wasm32-unknown-unknown"
    );
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
