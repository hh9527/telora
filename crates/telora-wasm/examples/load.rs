//! Execute an artifact in a fresh process; no source or MIR is loaded.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("expected .wasm filename")?;
    let bytes = std::fs::read(path)?;
    let mut session = telora_wasm::session::Session::load(&bytes, 10_000_000)?;
    session.initialize()?;
    println!("{}", session.eval()?);
    Ok(())
}
