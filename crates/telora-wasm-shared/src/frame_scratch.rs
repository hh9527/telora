//! Reusable function-local storage, separate from the managed object arena.
use alloc::vec::Vec;

pub const BASE: u32 = 0x8000_0000;
pub const TRANSFER: u32 = 0xc000_0000;

#[derive(Default)]
pub struct Scratch {
    words: Vec<u64>,
    frames: Vec<usize>,
    transfer: Vec<u64>,
    pending: Option<u32>,
}

impl Scratch {
    pub const fn new() -> Self {
        Self {
            words: Vec::new(),
            frames: Vec::new(),
            transfer: Vec::new(),
            pending: None,
        }
    }

    /// The word immediately preceding the returned local region owns args.
    pub fn enter(&mut self, bytes: u32, args: u32) -> Result<u32, &'static str> {
        let adopted = args == TRANSFER;
        if adopted != self.pending.is_some() {
            return Err("invalid tail adoption");
        }
        let prefix = if adopted { self.transfer.len() } else { 0 };
        if let Some(count) = self.pending {
            for i in 0..count as usize {
                let reference = (self.transfer[i / 2] >> ((i % 2) * 32)) as u32;
                if reference >= TRANSFER && (reference - TRANSFER) as usize >= prefix * 8 {
                    return Err("invalid tail value");
                }
            }
        }
        let base = self.words.len();
        let locals = base
            .checked_add(prefix)
            .and_then(|n| n.checked_add(1))
            .ok_or("scratch overflow")?;
        let end = locals
            .checked_add((bytes as usize).div_ceil(8))
            .ok_or("scratch overflow")?;
        if end >= ((TRANSFER - BASE) / 8) as usize {
            return Err("scratch namespace exhausted");
        }
        self.frames
            .try_reserve(1)
            .map_err(|_| "scratch frames exhausted")?;
        self.words
            .try_reserve(end - base)
            .map_err(|_| "scratch exhausted")?;
        self.words.resize(end, 0);
        let arguments = if let Some(count) = self.pending.take() {
            self.words[base..base + prefix].copy_from_slice(&self.transfer);
            let packet = &mut self.words[base..base + prefix];
            for i in 0..count as usize {
                let shift = (i % 2) * 32;
                let reference = (packet[i / 2] >> shift) as u32;
                if reference >= TRANSFER {
                    let offset = reference - TRANSFER;
                    let relocated = BASE + (base * 8) as u32 + offset;
                    let mask = u64::from(u32::MAX) << shift;
                    packet[i / 2] = (packet[i / 2] & !mask) | (u64::from(relocated) << shift);
                }
            }
            self.transfer.clear();
            BASE + (base * 8) as u32
        } else {
            args
        };
        self.words[locals - 1] = u64::from(arguments);
        self.frames.push(base);
        Ok(BASE + (locals * 8) as u32)
    }

    pub fn leave(&mut self) -> Result<(), &'static str> {
        let base = self.frames.pop().ok_or("scratch frame underflow")?;
        self.words.truncate(base);
        Ok(())
    }

    /// A tail packet contains argument pointers followed by owned plain values.
    /// Its embedded owned pointers are relative to TRANSFER, ready for adoption.
    pub fn tail(&mut self, packet: u32, bytes: u32, count: u32) -> Result<u32, &'static str> {
        if self.pending.is_some() || !(BASE..TRANSFER).contains(&packet) || !bytes.is_multiple_of(8)
        {
            return Err("invalid tail packet");
        }
        let start = ((packet - BASE) / 8) as usize;
        let length = (bytes / 8) as usize;
        let end = start.checked_add(length).ok_or("tail packet overflow")?;
        let frame = *self.frames.last().ok_or("tail outside frame")?;
        if !packet.is_multiple_of(8) || start < frame || end > self.words.len() || count > bytes / 4
        {
            return Err("invalid tail extent");
        }
        self.transfer
            .try_reserve(length.saturating_sub(self.transfer.len()))
            .map_err(|_| "tail storage exhausted")?;
        self.transfer.resize(length, 0);
        self.transfer.copy_from_slice(&self.words[start..end]);
        self.leave()?;
        self.pending = Some(count);
        Ok(TRANSFER)
    }

    pub fn address(&mut self, reference: u32) -> Result<*mut u8, &'static str> {
        let offset = reference.checked_sub(BASE).ok_or("not scratch")? as usize;
        if reference >= TRANSFER || offset > self.words.len() * 8 {
            return Err("invalid scratch reference");
        }
        Ok(unsafe { self.words.as_mut_ptr().cast::<u8>().add(offset) })
    }

    pub fn reset(&mut self) {
        self.words.clear();
        self.frames.clear();
        self.transfer.clear();
        self.pending = None;
    }

    pub fn depth(&self) -> usize {
        self.frames.len()
    }
    pub fn capacity_bytes(&self) -> usize {
        (self.words.capacity() + self.transfer.capacity()) * 8
            + self.frames.capacity() * core::mem::size_of::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(scratch: &mut Scratch, reference: u32) -> u64 {
        unsafe {
            scratch
                .address(reference)
                .unwrap()
                .cast::<u64>()
                .read_unaligned()
        }
    }

    fn write(scratch: &mut Scratch, reference: u32, value: u64) {
        unsafe {
            scratch
                .address(reference)
                .unwrap()
                .cast::<u64>()
                .write_unaligned(value);
        }
    }

    fn transfer(scratch: &mut Scratch, frame: u32, value: u64) {
        write(scratch, frame, u64::from(TRANSFER + 8) | (77u64 << 32));
        write(scratch, frame + 8, value);
        assert_eq!(scratch.tail(frame, 16, 2).unwrap(), TRANSFER);
    }

    #[test]
    fn repeated_tail_handoff_reuses_storage_and_owns_arguments() {
        let mut scratch = Scratch::new();
        let mut frame = scratch.enter(16, 0).unwrap();
        let mut capacity = 0;
        for value in 0..50_000 {
            transfer(&mut scratch, frame, value);
            assert_eq!(scratch.depth(), 0);
            frame = scratch.enter(16, TRANSFER).unwrap();
            let args = word(&mut scratch, frame - 8) as u32;
            let refs = word(&mut scratch, args);
            assert_eq!(refs >> 32, 77);
            assert_eq!(word(&mut scratch, refs as u32), value);
            assert_eq!(scratch.depth(), 1);
            if value == 10 {
                capacity = scratch.capacity_bytes();
            }
            if value > 10 {
                assert_eq!(scratch.capacity_bytes(), capacity);
            }
        }
        scratch.leave().unwrap();
        assert!(scratch.leave().is_err());
    }

    #[test]
    fn nested_tail_adoption_preserves_ancestor_arguments() {
        let mut scratch = Scratch::new();
        let frame = scratch.enter(16, 0).unwrap();
        transfer(&mut scratch, frame, 42);
        let frame = scratch.enter(16, TRANSFER).unwrap();
        let args = word(&mut scratch, frame - 8) as u32;
        let owned = word(&mut scratch, args) as u32;
        for value in 0..100 {
            let nested = scratch.enter(16, args).unwrap();
            transfer(&mut scratch, nested, value);
            scratch.enter(16, TRANSFER).unwrap();
            scratch.leave().unwrap();
            assert_eq!(word(&mut scratch, owned), 42);
            assert_eq!(scratch.depth(), 1);
        }
        scratch.leave().unwrap();
    }

    #[test]
    fn reset_discards_interrupted_tail_and_rejects_invalid_extents() {
        let mut scratch = Scratch::new();
        let frame = scratch.enter(16, 0).unwrap();
        assert!(scratch.tail(frame, 24, 1).is_err());
        assert_eq!(scratch.depth(), 1);
        transfer(&mut scratch, frame, 3);
        assert!(scratch.enter(16, 0).is_err());
        scratch.reset();
        assert!(scratch.enter(16, TRANSFER).is_err());
        scratch.enter(16, 0).unwrap();
        scratch.leave().unwrap();
    }
}
