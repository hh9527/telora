//! Bounded observational formatting. Never invokes Display, codecs or user code.
use super::*;

struct Formatter { output: String, truncated: bool }
impl Formatter {
    fn push(&mut self, text: &str) {
        if self.truncated { return; }
        for ch in text.chars() {
            if self.output.len() + ch.len_utf8() > 4093 { self.truncated = true; break; }
            self.output.push(ch);
        }
    }
    fn quoted(&mut self, text: &str) {
        self.push("\"");
        for ch in text.chars() {
            if self.truncated { break; }
            if ch == '\'' { self.push("'"); } else { self.push(&ch.escape_debug().to_string()); }
        }
        self.push("\"");
    }
    fn value(&mut self, rt: &Runtime, value: &Value, depth: usize) -> Result<()> {
        if self.truncated { return Ok(()); }
        let shape = rt.layout(value.type_id())?;
        match shape.kind {
            Kind::Scalar => {
                let bits = rt.scalar_bits(value.as_ref())?;
                self.push(&match shape.dynamic_kind {
                    Some("Float") => format!("{:?}", f64::from_bits(bits)),
                    Some("Int") => (bits as i64).to_string(),
                    _ => if bits == 0 { "'False" } else { "'True" }.into(),
                });
            }
            Kind::String => self.quoted(rt.text(value.as_ref())?.as_str()),
            Kind::Bytes => {
                self.push("b\"");
                let bytes = rt.bytes_data(value)?;
                for byte in bytes.iter().take(32) { self.push(&format!("\\x{byte:02x}")); }
                if bytes.len() > 32 { self.push("..."); }
                self.push("\"");
            }
            Kind::Metadata => self.push(&format!("<TypeId:{}>", value.words[2])),
            Kind::Dyn => self.push("<dyn>"),
            Kind::Function => self.push("<fn>"),
            Kind::Array | Kind::Tuple | Kind::Newtype | Kind::Record | Kind::Dict | Kind::Enum => {
                if depth >= 8 { self.push("..."); return Ok(()); }
                if shape.kind == Kind::Enum {
                    let variant = &shape.variants[rt.enum_tag(value)? as usize];
                    self.push("'"); self.push(&variant.name);
                    if let Some(payload) = rt.enum_payload(value)? {
                        self.push("("); self.value(rt, &payload.to_owned(), depth + 1)?; self.push(")");
                    }
                    return Ok(());
                }
                let (open, close, count) = match shape.kind {
                    Kind::Array => ("[", "]", rt.array_len(value)?),
                    Kind::Dict => ("{", "}", rt.dict_len(value)?),
                    Kind::Record => ("{", "}", shape.fields.len()),
                    _ => ("(", ")", shape.fields.len()),
                };
                self.push(open);
                for index in 0..count.min(32) {
                    if self.truncated { break; }
                    if index > 0 { self.push(", "); }
                    let child = match shape.kind {
                        Kind::Array => rt.array_get(value, index)?,
                        Kind::Dict => {
                            let (key, child) = rt.dict_entry(value, index)?;
                            self.push(rt.text(key)?.as_str()); self.push(": "); child
                        }
                        Kind::Record => {
                            self.push(&shape.field_names[index]); self.push(": "); rt.field(value, index)?
                        }
                        _ => rt.field(value, index)?,
                    };
                    self.value(rt, &child.to_owned(), depth + 1)?;
                }
                if count > 32 { self.push(", ..."); }
                self.push(close);
            }
            _ => self.push(&format!("<{:?}>", shape.kind)),
        }
        Ok(())
    }
}
impl Runtime {
    pub(crate) fn debug_repr(&self, value: &Value) -> Result<String> {
        let mut formatter = Formatter { output: String::new(), truncated: false };
        formatter.value(self, value, 0)?;
        if formatter.truncated { formatter.output.push_str("..."); }
        Ok(formatter.output)
    }
}
