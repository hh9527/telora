use super::*;

fn origin() -> Origin {
    Origin::Synthetic { derived_from: None }
}

include!("part-01.rs");
