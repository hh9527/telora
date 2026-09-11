use crate::heap::Handle;
use crate::type_store::{InternType, TypeId, TypeShape, TypeStore};
use crate::value::Atom;
use crate::{ValueKind, ValueRef};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use hashbrown::raw::RawTable;
use std::hash::{BuildHasher, Hash, Hasher};

include!("types/graph.rs");
include!("types/descriptor.rs");
include!("types/traits.rs");
include!("types/prelude.rs");
include!("types/relations.rs");
