//! Versioned, pointer-free structured inputs with temporary UTF-8 byte offsets.
use crate::source_range::OFFSET_LIMIT;
use alloc::{collections::BTreeSet, string::String, vec::Vec};

pub const MAGIC: &[u8; 4] = b"TLI1";
pub const MAX_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_NODES: usize = 100_000;
pub const MAX_DEPTH: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct LocatedValue {
    pub span: Option<Span>,
    pub value: InputValue,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: String,
    pub span: Option<Span>,
    pub value: LocatedValue,
}
#[derive(Clone, Debug, PartialEq)]
pub enum InputValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Array(Vec<LocatedValue>),
    Object(Vec<Field>),
}

fn span_check(s: Option<Span>) -> Result<(), &'static str> {
    if s.is_some_and(|s| s.start > s.end || s.end >= OFFSET_LIMIT) {
        return Err("invalid located input offset range");
    }
    Ok(())
}

impl LocatedValue {
    pub fn encode(&self) -> Result<Vec<u8>, &'static str> {
        fn append(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), &'static str> {
            if bytes.len() > MAX_BYTES.saturating_sub(out.len()) {
                return Err("located input byte limit");
            }
            out.extend_from_slice(bytes);
            Ok(())
        }
        fn span(out: &mut Vec<u8>, s: Option<Span>) -> Result<(), &'static str> {
            span_check(s)?;
            let s = s.unwrap_or(Span { start: 0, end: 0 });
            append(out, &s.start.to_le_bytes())?;
            append(out, &s.end.to_le_bytes())
        }
        fn text(out: &mut Vec<u8>, s: &str) -> Result<(), &'static str> {
            let len = u32::try_from(s.len()).map_err(|_| "located input string too long")?;
            append(out, &len.to_le_bytes())?;
            append(out, s.as_bytes())
        }
        fn node(
            out: &mut Vec<u8>,
            n: &LocatedValue,
            depth: usize,
            nodes: &mut usize,
        ) -> Result<(), &'static str> {
            if depth > MAX_DEPTH || *nodes >= MAX_NODES {
                return Err("located input node/depth limit");
            }
            *nodes += 1;
            let tag = match &n.value {
                InputValue::Null => 0,
                InputValue::Bool(false) => 1,
                InputValue::Bool(true) => 2,
                InputValue::Int(_) => 3,
                InputValue::Float(_) => 4,
                InputValue::String(_) => 5,
                InputValue::Array(_) => 6,
                InputValue::Object(_) => 7,
            };
            append(out, &[tag])?;
            span(out, n.span)?;
            match &n.value {
                InputValue::Int(v) => append(out, &v.to_le_bytes())?,
                InputValue::Float(v) => {
                    if !v.is_finite() {
                        return Err("nonfinite located input");
                    }
                    append(out, &v.to_le_bytes())?;
                }
                InputValue::String(v) => text(out, v)?,
                InputValue::Array(items) => {
                    let len = u32::try_from(items.len()).map_err(|_| "located input node limit")?;
                    append(out, &len.to_le_bytes())?;
                    for item in items {
                        node(out, item, depth + 1, nodes)?;
                    }
                }
                InputValue::Object(fields) => {
                    let len =
                        u32::try_from(fields.len()).map_err(|_| "located input node limit")?;
                    append(out, &len.to_le_bytes())?;
                    let mut keys = BTreeSet::new();
                    for field in fields {
                        if !keys.insert(&field.name) {
                            return Err("duplicate located input key");
                        }
                        text(out, &field.name)?;
                        span(out, field.span)?;
                        node(out, &field.value, depth + 1, nodes)?;
                    }
                }
                _ => (),
            }
            Ok(())
        }
        let mut out = MAGIC.to_vec();
        node(&mut out, self, 0, &mut 0)?;
        Ok(out)
    }
}

pub fn decode(bytes: &[u8]) -> Result<LocatedValue, &'static str> {
    if bytes.len() > MAX_BYTES {
        return Err("located input byte limit");
    }
    if bytes.get(..4) != Some(MAGIC) {
        return Err("unsupported located input version");
    }
    struct Reader<'a> {
        bytes: &'a [u8],
        at: usize,
        nodes: usize,
    }
    impl Reader<'_> {
        fn take(&mut self, n: usize) -> Result<&[u8], &'static str> {
            let end = self
                .at
                .checked_add(n)
                .ok_or("located input size overflow")?;
            let b = self
                .bytes
                .get(self.at..end)
                .ok_or("truncated located input")?;
            self.at = end;
            Ok(b)
        }
        fn word(&mut self) -> Result<u32, &'static str> {
            Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
        }
        fn span(&mut self) -> Result<Option<Span>, &'static str> {
            let s = Span {
                start: self.word()?,
                end: self.word()?,
            };
            span_check(Some(s))?;
            Ok((s.start != 0 || s.end != 0).then_some(s))
        }
        fn text(&mut self) -> Result<String, &'static str> {
            let len = self.word()? as usize;
            Ok(String::from(
                core::str::from_utf8(self.take(len)?).map_err(|_| "invalid located input UTF-8")?,
            ))
        }
        fn node(&mut self, depth: usize) -> Result<LocatedValue, &'static str> {
            if depth > MAX_DEPTH || self.nodes >= MAX_NODES {
                return Err("located input node/depth limit");
            }
            self.nodes += 1;
            let tag = self.take(1)?[0];
            let span = self.span()?;
            let value = match tag {
                0 => InputValue::Null,
                1 => InputValue::Bool(false),
                2 => InputValue::Bool(true),
                3 => InputValue::Int(i64::from_le_bytes(self.take(8)?.try_into().unwrap())),
                4 => {
                    let f = f64::from_le_bytes(self.take(8)?.try_into().unwrap());
                    if !f.is_finite() {
                        return Err("nonfinite located input");
                    }
                    InputValue::Float(f)
                }
                5 => InputValue::String(self.text()?),
                6 | 7 => {
                    let count = self.word()? as usize;
                    if count > MAX_NODES - self.nodes {
                        return Err("located input node limit");
                    }
                    if tag == 6 {
                        let mut items = Vec::new();
                        for _ in 0..count {
                            items.push(self.node(depth + 1)?);
                        }
                        InputValue::Array(items)
                    } else {
                        let mut fields = Vec::new();
                        let mut keys = BTreeSet::new();
                        for _ in 0..count {
                            let name = self.text()?;
                            if !keys.insert(name.clone()) {
                                return Err("duplicate located input key");
                            }
                            let span = self.span()?;
                            let value = self.node(depth + 1)?;
                            fields.push(Field { name, span, value });
                        }
                        InputValue::Object(fields)
                    }
                }
                _ => return Err("unknown located input value kind"),
            };
            Ok(LocatedValue { span, value })
        }
    }
    let mut r = Reader {
        bytes,
        at: 4,
        nodes: 0,
    };
    let value = r.node(0)?;
    if r.at != bytes.len() {
        return Err("trailing located input bytes");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_roundtrip_and_rejects_bad_packets() {
        let value = LocatedValue {
            span: Some(Span { start: 0, end: 7 }),
            value: InputValue::Object(alloc::vec![Field {
                name: String::from("a"),
                span: Some(Span { start: 9, end: 10 }),
                value: LocatedValue {
                    span: None,
                    value: InputValue::Array(alloc::vec![
                        LocatedValue {
                            span: None,
                            value: InputValue::Int(i64::MAX)
                        },
                        LocatedValue {
                            span: None,
                            value: InputValue::String(String::from("中文🙂\r\n"))
                        },
                    ])
                }
            }]),
        };
        let bytes = value.encode().unwrap();
        assert_eq!(decode(&bytes).unwrap(), value);
        for n in 0..bytes.len() {
            assert!(decode(&bytes[..n]).is_err());
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(decode(&trailing).is_err());
        let mut wrong = bytes.clone();
        wrong[3] = b'2';
        assert!(decode(&wrong).is_err());
        let bad = LocatedValue {
            span: Some(Span { start: 8, end: 7 }),
            value: InputValue::Null,
        };
        assert!(bad.encode().is_err());
        assert!(
            LocatedValue {
                span: None,
                value: InputValue::Float(f64::NAN)
            }
            .encode()
            .is_err()
        );
        let field = Field {
            name: String::from("same"),
            span: None,
            value: LocatedValue {
                span: None,
                value: InputValue::Null,
            },
        };
        assert!(
            LocatedValue {
                span: None,
                value: InputValue::Object(alloc::vec![field.clone(), field])
            }
            .encode()
            .is_err()
        );
    }
}
