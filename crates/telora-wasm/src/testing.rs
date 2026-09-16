//! Deferred test protocol. Only language failures are recoverable between cases.
use crate::{abi, diagnostic_output::Diagnostic, session::Session, transport::Value};

#[derive(Debug)]
pub enum Description {
    ShouldOk(Value),
    ShouldFail(Value),
    ShouldFailWith(Value, String),
    Fixtures {
        sources: Vec<String>,
        factory: Value,
    },
}

#[derive(Debug)]
pub struct TestDescription {
    pub origin: [u32; 5],
    pub kind: Description,
}

#[derive(Debug)]
pub struct Invocation {
    pub value: Option<Value>,
    pub diagnostics: Vec<Diagnostic>,
    /// Present only for traps or host/ABI failures. These cannot satisfy should_fail.
    pub terminal: Option<String>,
}

pub struct TestSession {
    session: Session,
    failed: bool,
}

impl TestSession {
    /// Requires a successfully initialized graph. It is never reinitialized per case.
    pub fn new(session: Session) -> Result<Self, String> {
        let phase = session
            .instance
            .get_global(&session.store, "telora_phase")
            .ok_or("Wasm: test session lacks phase")?;
        if phase.get(&session.store).i32() != Some(2) {
            return Err("Wasm: test graph is not initialized".into());
        }
        Ok(Self {
            session,
            failed: false,
        })
    }

    pub fn session(&self) -> &Session {
        &self.session
    }
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    pub fn entry(&mut self) -> Result<Value, String> {
        if self.failed {
            return Err("Wasm: test session has terminated".into());
        }
        Ok(Value {
            pointer: self.session.entry()?,
            ty: self.session.manifest.entry_type,
        })
    }

    pub fn describe(&self, value: Value) -> Result<TestDescription, String> {
        let session = &self.session;
        session.expect_value(value, value.ty)?;
        if session.manifest.types[value.ty as usize].resource_table != Some(abi::TESTS) {
            return Err("Wasm: test requires sealed Test identity".into());
        }
        let output = session.output();
        let (base, bytes) =
            output.payload(abi::TESTS, output.word(value.pointer as u64 + abi::DATA)?)?;
        let operation = output.word(base)?;
        let count = output.word(base + 4)?;
        if operation > 3
            || count != if operation < 2 { 1 } else { 2 }
            || bytes != 8 + count as u64 * 4
        {
            return Err("Wasm: invalid Test description".into());
        }
        let input = |index: u64| -> Result<Value, String> {
            let pointer = output.word(base + 8 + index * 4)?;
            let ty = output.word(pointer as u64 + abi::TYPE)?;
            let value = Value { pointer, ty };
            session.expect_value(value, ty)?;
            Ok(value)
        };
        let kind = match operation {
            0 => Description::ShouldOk(input(0)?),
            1 => Description::ShouldFail(input(0)?),
            2 => Description::ShouldFailWith(input(0)?, session.text_value(input(1)?)?),
            3 => Description::Fixtures {
                sources: session
                    .array_items(input(0)?)?
                    .into_iter()
                    .map(|v| session.text_value(v))
                    .collect::<Result<_, _>>()?,
                factory: input(1)?,
            },
            _ => unreachable!(),
        };
        Ok(TestDescription {
            origin: output.location_words(value.pointer as u64)?,
            kind,
        })
    }

    pub fn invoke(&mut self, callable: Value, arguments: &[Value]) -> Result<Invocation, String> {
        if self.failed {
            return Err("Wasm: test session has terminated".into());
        }
        // Do not reset fuel, initialized demands, caches, or the language heap.
        let before = self.session.diagnostics()?.len();
        self.failed = true;
        let execution = self.session.invoke_testable(callable, arguments);
        let mut diagnostics = self
            .session
            .diagnostics()?
            .into_iter()
            .skip(before)
            .collect::<Vec<_>>();
        match execution {
            Ok(value) => {
                if value.is_none() {
                    if !diagnostics.iter().any(|d| !d.warning)
                        && let Some(event) = self.session.active_failure()?
                    {
                        diagnostics.push(event);
                    }
                    if !diagnostics.iter().any(|d| !d.warning) {
                        self.failed = true;
                        return Ok(Invocation {
                            value,
                            diagnostics,
                            terminal: Some(
                                "Wasm: test failed without a language diagnostic".into(),
                            ),
                        });
                    }
                    for (name, value) in [("telora_error", 0), ("telora_phase", 2)] {
                        self.session
                            .instance
                            .get_global(&self.session.store, name)
                            .ok_or("Wasm: missing test boundary global")?
                            .set(&mut self.session.store, wasmi::Val::I32(value))
                            .map_err(|e| e.to_string())?;
                    }
                }
                self.failed = false;
                Ok(Invocation {
                    value,
                    diagnostics,
                    terminal: None,
                })
            }
            Err(message) => {
                self.failed = true;
                Ok(Invocation {
                    value: None,
                    diagnostics,
                    terminal: Some(message),
                })
            }
        }
    }
}
