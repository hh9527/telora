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
        let name = manifest
            .sources
            .iter()
            .find(|s| s.id == source)
            .map(|s| s.name.as_str())
            .unwrap_or("<unknown>");
        match manifest
            .locations
            .iter()
            .find(|l| l.source == source && l.start == start && l.end == end)
        {
            Some(loc) => format!("{name}:{}:{}: {}", loc.line, loc.column, self.message),
            None => format!("{name}:{start}..{end}: {}", self.message),
        }
    }
}

pub(crate) fn error_message(code: u32) -> &'static str {
    match code {
        ERROR_OVERFLOW => "integer arithmetic overflowed",
        ERROR_DIVISION => "integer division by zero",
        ERROR_CYCLE => "initialization dependency cycle",
        ERROR_INDEX => "array index out of bounds",
        ERROR_KEY => "dictionary key is absent",
        ERROR_PROPERTY => "property type does not support this decorator target",
        ERROR_MATCH => "no match arm accepted the value",
        ERROR_DATA => "data module has not been injected before initialization",
        _ => "Wasm execution failed",
    }
}

impl Session {
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
