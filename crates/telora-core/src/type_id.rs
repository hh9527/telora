#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TypeId(u32);

impl TypeId {
    const UNCHECKED: u32 = 1 << 31;
    const SOLVED: u32 = 1 << 30;

    /// Disjoint encoding for IDs owned by the sealed Main-world type image.
    /// This is an identity encoding, not interning or type reconstruction.
    pub(crate) fn solved(id: crate::mir::TypeId) -> Self {
        let raw = id.index() as u32 + 1;
        assert!(raw < Self::SOLVED, "solved type image exceeds ID space");
        Self(Self::SOLVED | raw)
    }

    pub(crate) fn solved_id(self) -> Option<crate::mir::TypeId> {
        if self.0 & Self::SOLVED == 0 { return None; }
        (self.0 & !(Self::SOLVED | Self::UNCHECKED)).checked_sub(1).map(crate::mir::TypeId)
    }

    pub(crate) const fn unchecked(self) -> Self {
        Self(self.0 | Self::UNCHECKED)
    }

    pub(crate) const fn checked(self) -> Self {
        Self(self.0 & !Self::UNCHECKED)
    }

    pub(crate) const fn is_unchecked(self) -> bool {
        self.0 & Self::UNCHECKED != 0
    }

    pub(crate) const fn from_raw(raw: u32) -> Option<Self> {
        if raw & Self::SOLVED == 0 || raw & !(Self::SOLVED | Self::UNCHECKED) == 0 { None } else { Some(Self(raw)) }
    }

    pub const fn raw(self) -> u32 {
        self.0
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solved_stamps_round_trip_and_reject_legacy_ids() {
        for raw in [0, 1, 1024, (1 << 30) - 2] {
            let source = crate::mir::TypeId(raw);
            let id = TypeId::solved(source);
            assert_eq!(id.solved_id(), Some(source));
            assert_eq!(id.unchecked().solved_id(), Some(source));
            assert_eq!(id.unchecked().checked(), id);
            assert_eq!(TypeId::from_raw(id.raw()), Some(id));
            assert_eq!(TypeId::from_raw(id.unchecked().raw()), Some(id.unchecked()));
        }
        for raw in [0, 7, 1024, 1 << 30, 1 << 31, (1 << 31) | (1 << 30)] {
            assert_eq!(TypeId::from_raw(raw), None);
        }
    }
}
