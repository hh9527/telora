//! Read-only diagnostic transport, after Wasm has recorded the event.
use crate::{abi::*, artifact::Manifest, output::Output, session::Session};

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub warning: bool,
    pub origin: [u32; 3],
    pub message: String,
    pub subjects: Vec<[u32; 3]>,
}

impl Diagnostic {
    pub fn render(&self, manifest: &Manifest) -> String {
        let [source, start, end] = self.origin;
        let file = manifest.sources.iter().find(|s| s.id == source);
        match file {
            Some(file) => {
                let (line, column) = file.position(start);
                format!("{}:{line}:{column}: {}", file.name, self.message)
            }
            None => format!("<unknown>:{start}..{end}: {}", self.message),
        }
    }
}

pub(crate) fn error_message(code: u32) -> &'static str {
    match code {
        ERROR_OVERFLOW => "integer arithmetic overflowed",
        ERROR_DIVISION => "integer division by zero",
        ERROR_CYCLE => "initialization dependency cycle (cyclic demand)",
        ERROR_INDEX => "OutOfRange: array index out of bounds",
        ERROR_KEY => "dictionary key is absent",
        ERROR_PROPERTY => "property type does not support this decorator target",
        ERROR_MATCH => "no match arm accepted the value",
        ERROR_DATA => "data module has not been injected before initialization",
        _ => "Wasm execution failed",
    }
}

impl Session {
    /// Demand failures can propagate an earlier event without reporting it again.
    pub(crate) fn active_failure(&self) -> Result<Option<Diagnostic>, String> {
        let pointer = self
            .instance
            .get_global(&self.store, "telora_error")
            .ok_or("Wasm: missing failure global")?
            .get(&self.store)
            .i32()
            .ok_or("Wasm: invalid failure global")? as u32;
        if pointer == 0 {
            return Ok(None);
        }
        let output = self.output();
        let events = self.diagnostics()?;
        for (index, event) in events.into_iter().enumerate() {
            if output.payload(DIAGNOSTICS, index as u32)?.0 == pointer as u64 {
                return Ok(Some(event));
            }
        }
        Err("Wasm: failure does not reference a diagnostic event".into())
    }

    pub fn diagnostics(&self) -> Result<Vec<Diagnostic>, String> {
        let output = Output {
            memory: self.memory.data(&self.store),
            manifest: &self.manifest,
        };
        let count = output.word(table_address(DIAGNOSTICS) as u64 + 4)?;
        let mut diagnostics = vec![];
        for index in 0..count {
            let (pointer, bytes) = output.payload(DIAGNOSTICS, index)?;
            if bytes != DIAGNOSTIC_BYTES as u64 {
                return Err("Wasm: invalid diagnostic record size".into());
            }
            output.bytes(pointer, bytes)?;
            let origin = [
                output.word(pointer)?,
                output.word(pointer + 4)?,
                output.word(pointer + 8)?,
            ];
            let code = output.word(pointer + 12)?;
            let message = if code == ERROR_USER {
                output.text(output.word(pointer + 16)? as u64)?
            } else if code == ERROR_CYCLE {
                let mut affected = vec![];
                for global in &self.manifest.globals {
                    if output.word(global.demand as u64)? == 3
                        && output.word(global.demand as u64 + 4)? as u64 == pointer {
                        affected.push(global.name.as_str());
                    }
                }
                format!("{}; affected globals: {}", error_message(code), affected.join(", "))
            } else {
                error_message(code).into()
            };
            let mut subjects = vec![];
            let base = output.word(pointer + 20)? as u64;
            let count = output.word(pointer + 24)? as u64;
            output.bytes(base, count * 12)?;
            for index in 0..count {
                let offset = base + index * 12;
                let subject = [
                    output.word(offset)?,
                    output.word(offset + 4)?,
                    output.word(offset + 8)?,
                ];
                if subject[0] != 0 && !subjects.contains(&subject) {
                    subjects.push(subject);
                }
            }
            diagnostics.push(Diagnostic {
                warning: output.word(pointer + 28)? != 0,
                origin,
                message,
                subjects,
            });
        }
        Ok(diagnostics)
    }
}
