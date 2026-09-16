//! Byte transport only: parsers, source spans and Value construction stay in Guest.
use crate::{session::Session, transport::Value};
use telora_core::data_plan::Format;

impl Session {
    pub fn parse_data_source(&mut self, file: &telora_core::source::SourceFile, format: Format) -> Result<Value, String> {
        let source = crate::artifact::Source::from_file(file);
        if let Some(existing) = self.manifest.sources.iter().find(|item| item.id == source.id) {
            if existing.name != source.name {
                return Err("Wasm: input source identity conflict".into());
            }
        } else {
            self.manifest.sources.push(source);
            self.register_sources()?;
        }
        let input = file.text().contiguous().ok_or("Wasm: data source requires contiguous text")?;
        self.parse_data_text(input, format, file.id().get())
            .map_err(|error| format!("{}: {error}", file.name))
    }

    /// source=0 is a request/temporary input, never an independently registered source.
    pub fn parse_data_text(&mut self, input: &str, format: Format, source: u32) -> Result<Value, String> {
        let ty = self.manifest.value_type.ok_or("Wasm: missing sealed Value contract")?;
        if source != 0 && !self.manifest.sources.iter().any(|entry| entry.id == source) {
            return Err("Wasm: input source is not registered".into());
        }
        let length = u32::try_from(input.len()).map_err(|_| "Wasm: input size exceeds wasm32")?;
        let cap = length.checked_add(7).ok_or("Wasm: input capacity overflow")? & !7;
        let alloc = self.instance.get_typed_func::<u32, u32>(&self.store, "mem-alloc")
            .map_err(|e| e.to_string())?;
        let free = self.instance.get_typed_func::<(u32, u32), ()>(&self.store, "mem-free")
            .map_err(|e| e.to_string())?;
        let parse = self.instance.get_typed_func::<(u32, u32, u32, u32), u32>(&self.store, "telora_parse_data")
            .map_err(|e| e.to_string())?;
        let materialize = self.instance.get_typed_func::<(u32, u32), u32>(&self.store, "telora_materialize_data")
            .map_err(|e| e.to_string())?;
        let pointer = alloc.call(&mut self.store, cap).map_err(|e| e.to_string())?;
        self.write(pointer as usize, input.as_bytes())?;
        let format = match format { Format::Json => 1, Format::Yaml => 2, Format::Toml => 3 };
        let packet = parse.call(&mut self.store, (pointer, length, format, source))
            .map_err(|e| e.to_string())?;
        // The parser now owns its text spans. It cannot retain the transfer buffer.
        free.call(&mut self.store, (pointer, cap)).map_err(|e| e.to_string())?;
        let output = self.output();
        let error = output.word(packet as u64 + 12)?;
        if error != 0 {
            let text = output.word(error as u64)?;
            let bytes = output.word(error as u64 + 4)?;
            return Err(core::str::from_utf8(output.bytes(text as u64, bytes as u64)?)
                .map_err(|e| e.to_string())?.to_owned());
        }
        let pointer = materialize.call(&mut self.store, (packet, 0)).map_err(|e| e.to_string())?;
        Ok(Value { pointer, ty })
    }
}
