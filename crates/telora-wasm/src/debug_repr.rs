//! Bounded read-only formatting at the external debug boundary.
use crate::{abi::*, artifact::Kind, output::Output};

struct Formatter {
    text: String,
    truncated: bool,
}
impl Formatter {
    fn push(&mut self, value: &str) {
        if self.truncated {
            return;
        }
        for ch in value.chars() {
            if self.text.len() + ch.len_utf8() > 4093 {
                self.truncated = true;
                break;
            }
            self.text.push(ch);
        }
    }
    fn quoted(&mut self, value: &str) {
        self.push("\"");
        for ch in value.chars() {
            if self.truncated {
                break;
            }
            if ch == '\'' {
                self.push("'");
            } else {
                self.push(&ch.escape_debug().to_string());
            }
        }
        self.push("\"");
    }
    fn value(&mut self, output: &Output<'_>, pointer: u64, depth: usize) -> Result<(), String> {
        if self.truncated {
            return Ok(());
        }
        let ty = output.word(pointer + TYPE)? as usize;
        let desc = output
            .manifest
            .types
            .get(ty)
            .ok_or("Wasm: invalid debug TypeId")?;
        output.bytes(pointer, desc.bytes as u64)?;
        match desc.kind {
            Kind::Int => self.push(
                &i64::from_le_bytes(output.bytes(pointer + DATA, 8)?.try_into().unwrap())
                    .to_string(),
            ),
            Kind::Float => self.push(&format!(
                "{:?}",
                f64::from_le_bytes(output.bytes(pointer + DATA, 8)?.try_into().unwrap())
            )),
            Kind::Bool => self.push(if output.word(pointer + DATA)? == 0 {
                "'False"
            } else {
                "'True"
            }),
            Kind::Unit => self.push("()"),
            Kind::String => self.quoted(output.text_str(pointer)?),
            Kind::Function => self.push("<fn>"),
            Kind::Dyn => self.push("<dyn>"),
            Kind::Metadata => self.push(&format!("<TypeId:{}>", output.word(pointer + DATA)?)),
            Kind::Bytes => {
                let (base, bytes) = output.payload(BYTES, output.word(pointer + DATA)?)?;
                let start = output.word(pointer + 20)? as u64;
                let end = output.word(pointer + 24)? as u64;
                if start > end || end > bytes {
                    return Err("Wasm: invalid debug Bytes slice".into());
                }
                self.push("b\"");
                for byte in output.bytes(base + start, (end - start).min(32))? {
                    self.push(&format!("\\x{byte:02x}"));
                }
                if end - start > 32 {
                    self.push("...");
                }
                self.push("\"");
            }
            Kind::Unsupported => self.push("<opaque>"),
            _ if depth >= 8 => self.push("..."),
            Kind::Enum | Kind::Option | Kind::Value => {
                let branch = desc
                    .variants
                    .get(output.word(pointer + DATA)? as usize)
                    .ok_or("Wasm: invalid debug variant")?;
                self.push("'");
                self.push(&branch.name);
                if branch.ty.is_some() {
                    let value = if branch.boxed {
                        output.payload(VALUES, output.word(pointer + 24)?)?.0
                    } else {
                        pointer + 24
                    };
                    self.push("(");
                    self.value(output, value, depth + 1)?;
                    self.push(")");
                }
            }
            Kind::Newtype => {
                let (value, _) = output.payload(NEWTYPES, output.word(pointer + DATA)?)?;
                self.push("(");
                self.value(output, value, depth + 1)?;
                self.push(")");
            }
            Kind::Array | Kind::Dict => {
                let dict = desc.kind == Kind::Dict;
                let (base, bytes) =
                    output.payload(ARRAYS, output.word(pointer + if dict { 24 } else { DATA })?)?;
                let start = if dict {
                    0
                } else {
                    output.word(pointer + 20)? as u64
                };
                let end = output.word(pointer + if dict { 20 } else { 24 })? as u64;
                let element = *desc
                    .arguments
                    .first()
                    .ok_or("Wasm: missing debug element type")?;
                let stride = output
                    .manifest
                    .types
                    .get(element as usize)
                    .ok_or("Wasm: invalid debug element type")?
                    .bytes as u64;
                if start > end || end * stride > bytes || (stride == 0 && end != 0) {
                    return Err("Wasm: invalid debug sequence".into());
                }
                let keys = if dict {
                    let (keys, bytes) = output.payload(ARRAYS, output.word(pointer + DATA)?)?;
                    if end * 32 > bytes {
                        return Err("Wasm: invalid debug Dict keys".into());
                    }
                    keys
                } else {
                    0
                };
                self.push(if dict { "{" } else { "[" });
                for index in start..end.min(start + 32) {
                    if self.truncated {
                        break;
                    }
                    if index != start {
                        self.push(", ");
                    }
                    if dict {
                        self.push(output.text_str(keys + index * 32)?);
                        self.push(": ");
                    }
                    self.value(output, base + index * stride, depth + 1)?;
                }
                if end - start > 32 {
                    self.push(", ...");
                }
                self.push(if dict { "}" } else { "]" });
            }
            Kind::Tuple | Kind::Record => {
                let (base, bytes) = output.payload(RECORDS, output.word(pointer + DATA)?)?;
                let record = desc.kind == Kind::Record;
                self.push(if record { "{" } else { "(" });
                for (index, field) in desc.fields.iter().take(32).enumerate() {
                    if self.truncated {
                        break;
                    }
                    let width = output
                        .manifest
                        .types
                        .get(field.ty as usize)
                        .ok_or("Wasm: invalid debug field type")?
                        .bytes as u64;
                    if field.offset as u64 + width > bytes {
                        return Err("Wasm: invalid debug field".into());
                    }
                    if index != 0 {
                        self.push(", ");
                    }
                    if record {
                        self.push(&field.name);
                        self.push(": ");
                    }
                    self.value(output, base + field.offset as u64, depth + 1)?;
                }
                if desc.fields.len() > 32 {
                    self.push(", ...");
                }
                self.push(if record { "}" } else { ")" });
            }
        }
        Ok(())
    }
}

impl Output<'_> {
    pub(crate) fn debug_repr(&self, pointer: u64) -> Result<String, String> {
        let mut formatter = Formatter {
            text: String::new(),
            truncated: false,
        };
        formatter.value(self, pointer, 0)?;
        if formatter.truncated {
            formatter.text.push_str("...");
        }
        Ok(formatter.text)
    }
}
