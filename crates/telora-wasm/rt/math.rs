//! Fixed scalar ABI for operations without a direct Wasm instruction.
#[unsafe(no_mangle)]
pub extern "C" fn telora_float_remainder(left: f64, right: f64) -> f64 {
    libm::fmod(left, right)
}
