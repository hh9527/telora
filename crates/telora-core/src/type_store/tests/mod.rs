use super::*;
use crate::{ModuleId, TypeConstructorId};

fn constructor(local: u32) -> TypeConstructorId {
    TypeConstructorId {
        module: ModuleId::from_index(0),
        local,
    }
}

include!("part-01.rs");
