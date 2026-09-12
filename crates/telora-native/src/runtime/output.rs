use super::*;
use std::io::Write;

/// Output is a logical allocation too: a small shared graph can render to a
/// large document. Charge before appending, including escaped JSON fragments.
pub(super) struct Output<'a> {
    runtime: &'a Runtime,
    bytes: Vec<u8>,
}

impl<'a> Output<'a> {
    pub(super) fn new(runtime: &'a Runtime) -> Self { Self { runtime, bytes: Vec::new() } }
    pub(super) fn push_str(&mut self, text: &str) -> Result<()> {
        self.write_all(text.as_bytes()).map_err(|error| error.to_string())
    }
    pub(super) fn push(&mut self, character: char) -> Result<()> {
        self.push_str(character.encode_utf8(&mut [0; 4]))
    }
    pub(super) fn finish(self) -> Result<String> {
        String::from_utf8(self.bytes).map_err(|_| "native output is not UTF-8".into())
    }
}

impl Write for Output<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.runtime.charge_allocation(bytes.len(), 1, 0).map_err(std::io::Error::other)?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

impl Runtime {
    /// The output backing was charged while writing; transferring it to the
    /// String table only adds the slot, without copying or charging it twice.
    pub(super) fn output_string(&mut self, ty: TypeId, loc: Location, text: String) -> Result<Value> {
        if text.len() > 14 {
            self.charge_allocation(0, 1, std::mem::size_of::<RawStringItem>())?;
        }
        self.precharged_string(ty, loc, text)
    }
}
