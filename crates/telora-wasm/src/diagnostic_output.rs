//! Read-only diagnostic transport, after Wasm has recorded the event.
use crate::{abi::*, artifact::Manifest, output::Output, session::Session};

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: telora_core::source::Severity,
    pub message: String,
    pub locs: Vec<[u32; 5]>,
    pub initialization: Option<crate::artifact::InitializationRoot>,
}

impl Diagnostic {
    pub fn render(&self, manifest: &Manifest) -> String {
        let mut rendered = self.message.clone();
        if let Some(&primary) = self.locs.first() {
            let loc = telora_core::source::SourceCoordinates(primary);
            let (source, start, end) = (loc.source(), loc.start(), loc.end());
            let file = manifest.sources.iter().find(|s| s.id == source);
            rendered = match file {
                None if source == 0 => format!("<input>:{start}..{end}: {}", self.message),
                Some(file) => {
                    let (line, offset) = telora_core::source::SourceCoordinates::position(start);
                    format!("{}:{line}:{}: {}", file.name, offset + 1, self.message)
                }
                None => format!("<unknown>:{start}..{end}: {}", self.message),
            };
        }
        for (index, &words) in self.locs.iter().enumerate().skip(1) {
            let loc = telora_core::source::SourceCoordinates(words);
            let file = manifest.sources.iter().find(|s| s.id == loc.source());
            match file {
                None if loc.source() == 0 => rendered.push_str(&format!(
                    "\n  locs[{index}] <input>:{}..{}",
                    loc.start(),
                    loc.end()
                )),
                Some(file) => {
                    let (line, offset) =
                        telora_core::source::SourceCoordinates::position(loc.start());
                    rendered.push_str(&format!(
                        "\n  locs[{index}] {}:{line}:{}",
                        file.name,
                        offset + 1
                    ));
                }
                None => rendered.push_str(&format!(
                    "\n  locs[{index}] <unknown>:{}..{}",
                    loc.start(),
                    loc.end()
                )),
            }
        }
        rendered
    }
}

pub(crate) use telora_wasm_shared::diagnostics::error_message;

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
            let origin = output.location_words(pointer)?;
            let code = output.word(pointer + DIAG_CODE)?;
            let message = if code == ERROR_USER {
                output.text(output.word(pointer + DIAG_MESSAGE)? as u64)?
            } else if code == ERROR_CYCLE {
                let mut affected = vec![];
                for global in &self.manifest.globals {
                    if output.word(global.demand as u64)? == 3
                        && output.word(global.demand as u64 + 4)? as u64 == pointer
                    {
                        affected.push(global.name.as_str());
                    }
                }
                format!(
                    "{}; affected globals: {}",
                    error_message(code),
                    affected.join(", ")
                )
            } else {
                error_message(code).into()
            };
            let mut locs = if origin == [0; 5] {
                vec![]
            } else {
                vec![origin]
            };
            let base = output.word(pointer + DIAG_SUBJECTS)? as u64;
            let count = output.word(pointer + DIAG_COUNT)? as u64;
            output.bytes(base, count * u64::from(LOC_BYTES))?;
            for index in 0..count {
                let offset = base + index * u64::from(LOC_BYTES);
                let subject = output.location_words(offset)?;
                if subject != [0; 5] {
                    locs.push(subject);
                }
            }
            diagnostics.push(Diagnostic {
                severity: match output.word(pointer + DIAG_SEVERITY)? {
                    0 => telora_core::source::Severity::Error,
                    1 => telora_core::source::Severity::Warning,
                    2 => telora_core::source::Severity::Info,
                    other => return Err(format!("Wasm: invalid diagnostic severity {other}")),
                },
                message,
                locs,
                initialization: match output.word(pointer + DIAG_ROOT)? {
                    0 => None,
                    index => Some(
                        self.manifest
                            .initialization_roots
                            .get(index as usize - 1)
                            .ok_or("Wasm: invalid initialization root identity")?
                            .clone(),
                    ),
                },
            });
        }
        Ok(diagnostics)
    }
}
