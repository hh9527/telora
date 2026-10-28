//! Exact layout traversal, independent of marking, allocation and relocation.
//! Readers lend no physical address across an edge callback. Inline enum/Dyn
//! payloads are followed iteratively; object edges are queued by the visitor.
use crate::{abi::*, layout_image as layout};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Overflow,
    Layout,
    Memory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// A full value embedded in its owner's storage. Queue traversal without
    /// allocating or marking a separate object for the field.
    Inline {
        old: u32,
        at: u32,
        ty: u32,
    },
    /// Address of a packed source range, not an object reference.
    Source(u32),
    Content {
        old: u32,
        at: u32,
    },
    Object {
        table: u32,
        id: u32,
        ty: u32,
        at: u32,
    },
    Value {
        reference: u32,
        ty: u32,
        at: u32,
    },
    /// Owned, pointer-free storage (diagnostic subject ranges).
    Storage {
        reference: u32,
        bytes: u32,
        at: u32,
    },
    Function(u32),
}

pub trait Visitor {
    fn types(&self) -> u32;
    fn read(&self, reference: u32) -> Result<u32, Error>;
    fn edge(&mut self, edge: Edge) -> Result<(), Error>;
}

fn add(a: u32, b: u32) -> Result<u32, Error> {
    a.checked_add(b).ok_or(Error::Overflow)
}
fn mul(a: u32, b: u32) -> Result<u32, Error> {
    a.checked_mul(b).ok_or(Error::Overflow)
}
fn word(v: &impl Visitor, at: u32, offset: u32) -> Result<u32, Error> {
    v.read(add(at, offset)?)
}
fn desc(v: &impl Visitor, ty: u32) -> Result<u32, Error> {
    add(v.types(), mul(ty, layout::ENTRY_BYTES)?)
}
fn detail(v: &impl Visitor, ty: u32, index: u32) -> Result<u32, Error> {
    let desc = desc(v, ty)?;
    if index >= word(v, desc, layout::DETAIL_COUNT)? {
        return Err(Error::Layout);
    }
    add(
        add(v.types(), word(v, desc, layout::DETAIL_OFFSET)?)?,
        mul(index, layout::DETAIL_BYTES)?,
    )
}
fn detail_type(v: &impl Visitor, ty: u32, index: u32) -> Result<u32, Error> {
    let ty = word(v, detail(v, ty, index)?, layout::DETAIL_TYPE)?;
    (ty != layout::NO_TYPE).then_some(ty).ok_or(Error::Layout)
}
fn handle(v: &mut impl Visitor, table: u32, old: u32, at: u32, ty: u32) -> Result<(), Error> {
    let id = v.read(old)?;
    v.edge(Edge::Object { table, id, ty, at })
}
fn indirect(v: &mut impl Visitor, old: u32, at: u32, ty: u32) -> Result<(), Error> {
    let reference = v.read(old)?;
    v.edge(Edge::Value { reference, ty, at })
}

pub fn value(v: &mut impl Visitor, ty: u32, old: u32, at: u32) -> Result<(), Error> {
    v.edge(Edge::Source(old))?;
    data(v, ty, add(old, HEADER_BYTES)?, add(at, HEADER_BYTES)?)
}

fn data(v: &mut impl Visitor, mut ty: u32, mut old: u32, mut at: u32) -> Result<(), Error> {
    loop {
        let desc = desc(v, ty)?;
        match layout::Kind::decode(word(v, desc, layout::KIND)?).ok_or(Error::Layout)? {
            layout::Kind::Scalar => return Ok(()),
            layout::Kind::String | layout::Kind::Bytes => return v.edge(Edge::Content { old, at }),
            layout::Kind::Record => return handle(v, RECORDS, old, at, ty),
            layout::Kind::InlineRecord | layout::Kind::InlineNewtype => {
                if v.read(old)? != INLINE_FIELDS {
                    return Err(Error::Layout);
                }
                for index in 0..word(v, desc, layout::DETAIL_COUNT)? {
                    let field = detail(v, ty, index)?;
                    let offset = add(8, word(v, field, layout::DETAIL_OFFSET_OR_TAG)?)?;
                    v.edge(Edge::Inline {
                        old: add(old, offset)?,
                        at: add(at, offset)?,
                        ty: detail_type(v, ty, index)?,
                    })?;
                }
                return Ok(());
            }
            layout::Kind::Array => {
                return match word(v, old, 12)? {
                    ARRAY_EMPTY
                        if v.read(old)? == 0 && word(v, old, 4)? == 0 && word(v, old, 8)? == 0 =>
                    {
                        Ok(())
                    }
                    ARRAY_SINGLE
                        if v.read(old)? != 0 && word(v, old, 4)? == 0 && word(v, old, 8)? == 1 =>
                    {
                        indirect(v, old, at, detail_type(v, ty, 0)?)
                    }
                    ARRAY_SLICE => handle(v, ARRAYS, old, at, detail_type(v, ty, 0)?),
                    _ => Err(Error::Layout),
                };
            }
            layout::Kind::Dict => {
                handle(v, ARRAYS, old, at, detail_type(v, ty, 0)?)?;
                return handle(v, ARRAYS, add(old, 8)?, add(at, 8)?, detail_type(v, ty, 1)?);
            }
            layout::Kind::Enum => {
                let variant = detail(v, ty, v.read(old)?)?;
                let payload = word(v, variant, layout::DETAIL_TYPE)?;
                if payload == layout::NO_TYPE {
                    return Ok(());
                }
                old = add(old, 8)?;
                at = add(at, 8)?;
                if word(v, variant, layout::DETAIL_FLAGS)? & layout::DETAIL_BOXED != 0 {
                    return handle(v, VALUES, old, at, payload);
                }
                v.edge(Edge::Source(old))?;
                old = add(old, HEADER_BYTES)?;
                at = add(at, HEADER_BYTES)?;
                ty = payload;
            }
            layout::Kind::Dyn => {
                let payload = v.read(old)?;
                let boxed = word(v, old, 4)?;
                if boxed > 1 {
                    return Err(Error::Layout);
                }
                old = add(old, 8)?;
                at = add(at, 8)?;
                if boxed == 1 {
                    return handle(v, VALUES, old, at, payload);
                }
                ty = payload;
            }
            layout::Kind::Function => {
                v.edge(Edge::Function(v.read(old)?))?;
                let id = word(v, old, 4)?;
                return if id == 0 {
                    Ok(())
                } else {
                    v.edge(Edge::Object {
                        table: ENVIRONMENTS,
                        id,
                        ty,
                        at: add(at, 4)?,
                    })
                };
            }
            layout::Kind::Resource => {
                return handle(v, word(v, desc, layout::RESOURCE_TABLE)?, old, at, ty);
            }
            layout::Kind::Newtype => return handle(v, NEWTYPES, old, at, detail_type(v, ty, 0)?),
            layout::Kind::CompileTime => return Err(Error::Layout),
        }
    }
}

/// Scan a published object payload. This entry requires a valid initialized layout.
pub fn object(
    v: &mut impl Visitor,
    table: u32,
    old: u32,
    at: u32,
    bytes: u32,
    ty: u32,
) -> Result<(), Error> {
    match table {
        HASHES | REGEXES => {}
        ARRAYS => {
            let width = word(v, desc(v, ty)?, layout::VALUE_BYTES)?;
            if width == 0 {
                return if bytes == 0 {
                    Ok(())
                } else {
                    Err(Error::Layout)
                };
            }
            if width < HEADER_BYTES || !bytes.is_multiple_of(width) {
                return Err(Error::Layout);
            }
            for index in 0..bytes / width {
                let offset = mul(index, width)?;
                value(v, ty, add(old, offset)?, add(at, offset)?)?;
            }
        }
        VALUES => value(v, ty, old, at)?,
        NEWTYPES => value(v, detail_type(v, ty, 0)?, old, at)?,
        RECORDS => {
            for index in 0..word(v, desc(v, ty)?, layout::DETAIL_COUNT)? {
                let field = detail(v, ty, index)?;
                let offset = word(v, field, layout::DETAIL_OFFSET_OR_TAG)?;
                value(
                    v,
                    detail_type(v, ty, index)?,
                    add(old, offset)?,
                    add(at, offset)?,
                )?;
            }
        }
        ENVIRONMENTS | TESTS => {
            let count = word(v, old, if table == ENVIRONMENTS { 0 } else { 4 })?;
            if bytes != add(8, mul(count, 8)?)? {
                return Err(Error::Layout);
            }
            for index in 0..count {
                let offset = add(8, mul(index, 4)?)?;
                let ty = word(v, old, add(offset, mul(count, 4)?)?)?;
                indirect(v, add(old, offset)?, add(at, offset)?, ty)?;
            }
        }
        BLAMES => {
            v.edge(Edge::Source(old))?;
            for index in 0..word(v, old, BLAME_COUNT)? {
                v.edge(Edge::Source(add(
                    old,
                    add(BLAME_SUBJECTS, mul(index, LOC_BYTES)?)?,
                )?))?;
            }
            v.edge(Edge::Content {
                old: add(old, HEADER_BYTES)?,
                at: add(at, HEADER_BYTES)?,
            })?;
        }
        FORMATS => {
            let count = match v.read(old)? {
                0 => 0,
                1..=3 => 1,
                4 | 6..=11 => 2,
                _ => return Err(Error::Layout),
            };
            for index in 0..count {
                let ty = word(v, old, 12 + index * 4)?;
                indirect(v, add(old, 4 + index * 4)?, add(at, 4 + index * 4)?, ty)?;
            }
        }
        DEBUG_EVENTS => indirect(v, add(old, 4)?, add(at, 4)?, word(v, old, 8)?)?,
        DIAGNOSTICS => {
            v.edge(Edge::Source(old))?;
            indirect(
                v,
                add(old, DIAG_MESSAGE as u32)?,
                add(at, DIAG_MESSAGE as u32)?,
                word(v, old, DIAG_MESSAGE_TYPE as u32)?,
            )?;
            let count = word(v, old, DIAG_COUNT as u32)?;
            let reference = word(v, old, DIAG_SUBJECTS as u32)?;
            v.edge(Edge::Storage {
                reference,
                bytes: mul(count, LOC_BYTES)?,
                at: add(at, DIAG_SUBJECTS as u32)?,
            })?;
            for index in 0..count {
                v.edge(Edge::Source(add(reference, mul(index, LOC_BYTES)?)?))?;
            }
        }
        _ => return Err(Error::Layout),
    }
    Ok(())
}

#[cfg(test)]
mod tests;
