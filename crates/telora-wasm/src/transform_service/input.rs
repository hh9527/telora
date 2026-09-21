//! One Host-owned Guest allocation is reused across all source injections.
use crate::session::Session;
use std::io::{ErrorKind, Read};

pub(super) struct TransferBuffer {
    pub pointer: u32,
    cap: u32,
}

impl TransferBuffer {
    pub fn new() -> Self {
        Self { pointer: 1, cap: 0 }
    }

    pub fn read(
        &mut self,
        session: &mut Session,
        reader: &mut dyn Read,
        limit: usize,
    ) -> Result<u32, String> {
        let limit = limit.min(u32::MAX as usize) as u32;
        let mut length = 0u32;
        loop {
            if length == self.cap || length == limit {
                // Probe before growing: exact-capacity files need no extra allocation.
                let mut byte = [0];
                match reader.read(&mut byte) {
                    Ok(0) => return Ok(length),
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error.to_string()),
                    Ok(_) => {}
                }
                if length == limit {
                    return Err("input exceeds file_size limit".into());
                }
                let cap = self.cap.saturating_mul(2).max(8192).min(limit);
                self.pointer = if self.cap == 0 {
                    super::buffers::alloc(session, cap, 1)?
                } else {
                    session
                        .exports
                        .realloc
                        .call(&mut session.store, (self.pointer, self.cap, cap, 1))
                        .map_err(|e| e.to_string())?
                };
                self.cap = cap;
                session
                    .memory
                    .write(
                        &mut session.store,
                        self.pointer as usize + length as usize,
                        &byte,
                    )
                    .map_err(|e| e.to_string())?;
                length += 1;
            }
            // Reacquire the view after every Guest call; memory may have grown.
            let start = self.pointer as usize + length as usize;
            let end = self.pointer as usize + self.cap.min(limit) as usize;
            let target = session
                .memory
                .data_mut(&mut session.store)
                .get_mut(start..end)
                .ok_or("invalid Guest transfer buffer")?;
            if target.is_empty() {
                continue;
            }
            match reader.read(target) {
                Ok(0) => return Ok(length),
                Ok(count) => length += count as u32,
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error.to_string()),
            }
        }
    }

    pub fn free(self, session: &mut Session) -> Result<(), String> {
        super::buffers::free(session, self.pointer, self.cap, 1)
    }
}
