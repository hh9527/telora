//! Explicit frame and Host roots. Scalar bits are never interpreted as references.
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An owned raw allocation; its bytes are not interpreted as references.
    Storage,
    /// Postorder parser packet with explicitly typed row/span ownership.
    ParsePlan,
    /// Fixed parser/codec context; fields have explicitly prescribed roles.
    Context {
        string_type: u32,
        codec: bool,
    },
    /// A prefix of pointers to full values of a closed type.
    References {
        type_id: u32,
        initialized: u32,
    },
    Value {
        type_id: u32,
    },
    /// A typed object under construction; only the initialized prefix is scanned.
    Object {
        type_id: u32,
        tag: u32,
        initialized: u32,
    },
    /// A packed origin owner, independent of whether the value contains GC fields.
    Source,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Root {
    pub reference: u32,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Limit,
    Allocation,
    InvalidFrame,
    InvalidSlot,
    StaleHandle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    generation: u64,
    base: usize,
    count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle {
    generation: u64,
    slot: usize,
}

impl Handle {
    pub fn generation(self) -> u64 {
        self.generation
    }
}

struct Pin {
    generation: u64,
    root: Option<Root>,
}

pub struct Roots {
    frames: Vec<Frame>,
    slots: Vec<Option<Root>>,
    pins: Vec<Pin>,
    free_pins: Vec<usize>,
    generation: u64,
    limit: usize,
}

impl Roots {
    pub const fn new(limit: usize) -> Self {
        Self {
            frames: Vec::new(),
            slots: Vec::new(),
            pins: Vec::new(),
            free_pins: Vec::new(),
            generation: 0,
            limit,
        }
    }

    fn generation(&mut self) -> Result<u64, Error> {
        self.generation = self.generation.checked_add(1).ok_or(Error::Limit)?;
        Ok(self.generation)
    }

    pub fn enter(&mut self, count: usize) -> Result<Frame, Error> {
        let end = self.slots.len().checked_add(count).ok_or(Error::Limit)?;
        // Empty frames also consume bounded bookkeeping space.
        if end
            .checked_add(self.pins.len())
            .and_then(|n| n.checked_add(self.frames.len() + 1))
            .ok_or(Error::Limit)?
            > self.limit
        {
            return Err(Error::Limit);
        }
        self.slots
            .try_reserve(count)
            .map_err(|_| Error::Allocation)?;
        self.frames.try_reserve(1).map_err(|_| Error::Allocation)?;
        let frame = Frame {
            generation: self.generation()?,
            base: self.slots.len(),
            count,
        };
        self.slots.resize(end, None);
        self.frames.push(frame);
        Ok(frame)
    }

    fn current(&self, frame: Frame) -> Result<(), Error> {
        if self.frames.last() == Some(&frame) {
            Ok(())
        } else {
            Err(Error::InvalidFrame)
        }
    }

    pub fn set(&mut self, frame: Frame, slot: usize, root: Option<Root>) -> Result<(), Error> {
        self.current(frame)?;
        if slot >= frame.count {
            return Err(Error::InvalidSlot);
        }
        self.slots[frame.base + slot] = root.filter(|root| root.reference != 0);
        Ok(())
    }

    /// All normal and recoverable failure exits pop through the same boundary.
    /// No GC may occur between returning a borrowed value and the caller setting its slot.
    pub fn leave(&mut self, frame: Frame) -> Result<(), Error> {
        self.current(frame)?;
        self.slots.truncate(frame.base);
        self.frames.pop();
        Ok(())
    }

    /// Tail calls replace their frame with a protected transfer packet. Allocation
    /// failure leaves the old frame intact. No user code/GC runs in the transition.
    pub fn tail(&mut self, frame: Frame, transfer: &[Root]) -> Result<Frame, Error> {
        self.current(frame)?;
        let count = transfer.len();
        let end = frame.base.checked_add(count).ok_or(Error::Limit)?;
        if end
            .checked_add(self.pins.len())
            .and_then(|n| n.checked_add(self.frames.len()))
            .ok_or(Error::Limit)?
            > self.limit
        {
            return Err(Error::Limit);
        }
        self.slots
            .try_reserve(end.saturating_sub(self.slots.len()))
            .map_err(|_| Error::Allocation)?;
        let next = Frame {
            generation: self.generation()?,
            base: frame.base,
            count,
        };
        self.slots.truncate(frame.base);
        self.slots.extend(transfer.iter().copied().map(Some));
        *self.frames.last_mut().unwrap() = next;
        Ok(next)
    }

    pub fn pin(&mut self, root: Root) -> Result<Handle, Error> {
        let generation = self.generation()?;
        if let Some(slot) = self.free_pins.pop() {
            self.pins[slot] = Pin {
                generation,
                root: Some(root),
            };
            return Ok(Handle { generation, slot });
        }
        if self
            .slots
            .len()
            .checked_add(self.frames.len())
            .and_then(|n| n.checked_add(self.pins.len() + 1))
            .ok_or(Error::Limit)?
            > self.limit
        {
            return Err(Error::Limit);
        }
        self.pins.try_reserve(1).map_err(|_| Error::Allocation)?;
        self.free_pins
            .try_reserve(self.pins.len() + 1 - self.free_pins.len())
            .map_err(|_| Error::Allocation)?;
        let slot = self.pins.len();
        self.pins.push(Pin {
            generation,
            root: Some(root),
        });
        Ok(Handle { generation, slot })
    }

    pub fn get(&self, handle: Handle) -> Result<Root, Error> {
        self.pins
            .get(handle.slot)
            .filter(|pin| pin.generation == handle.generation)
            .and_then(|pin| pin.root)
            .ok_or(Error::StaleHandle)
    }

    pub fn unpin(&mut self, handle: Handle) -> Result<Root, Error> {
        let root = self.get(handle)?;
        self.pins[handle.slot].root = None;
        self.free_pins.push(handle.slot);
        Ok(root)
    }

    /// Boundary copying may relocate pins; their identity and generation remain stable.
    pub fn rewrite_pins(&mut self, mut relocate: impl FnMut(Root) -> Root) {
        for pin in &mut self.pins {
            if let Some(root) = pin.root {
                pin.root = Some(relocate(root));
            }
        }
    }

    pub fn invalidate_pins(&mut self) {
        self.free_pins.clear();
        for (slot, pin) in self.pins.iter_mut().enumerate() {
            pin.root = None;
            self.free_pins.push(slot);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = Root> + '_ {
        self.slots
            .iter()
            .filter_map(|root| *root)
            .chain(self.pins.iter().filter_map(|pin| pin.root))
            .filter(|root| root.reference != 0)
    }

    pub fn depth(&self) -> usize {
        self.frames.len()
    }
    pub fn active_slots(&self) -> usize {
        self.slots.len()
    }
}

#[cfg(test)]
mod tests;
