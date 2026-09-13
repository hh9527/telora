//! JSON text primitives, independent of Telora types and heap layout.
use alloc::string::String;
use core::fmt::{self, Write};

/// Per-call output state. Generated code owns traversal and collection identity.
pub(crate) struct Writer {
    pub output: String,
    indent: Option<u32>,
    depth: u32,
}

impl Writer {
    pub fn new(indent: Option<u32>) -> Self {
        assert!(indent.is_none_or(|width| width <= 16));
        Self {
            output: String::new(),
            indent,
            depth: 0,
        }
    }
    pub fn quoted(&mut self, input: &str) {
        quoted(&mut self.output, input).unwrap();
    }
    pub fn integer(&mut self, input: i64) {
        write!(self.output, "{input}").unwrap();
    }
    pub fn float(&mut self, input: f64) -> bool {
        if !input.is_finite() {
            return false;
        }
        write!(self.output, "{input}").unwrap();
        true
    }
    pub fn open(&mut self, object: bool) {
        self.output.push(if object { '{' } else { '[' });
        self.depth = self.depth.checked_add(1).unwrap();
    }
    pub fn item(&mut self, index: u32) {
        if index != 0 {
            self.output.push(',');
        }
        self.newline();
    }
    pub fn colon(&mut self) {
        self.output.push(':');
        if self.indent.is_some() {
            self.output.push(' ');
        }
    }
    pub fn close(&mut self, object: bool, nonempty: bool) {
        self.depth = self.depth.checked_sub(1).unwrap();
        if nonempty {
            self.newline();
        }
        self.output.push(if object { '}' } else { ']' });
    }
    fn newline(&mut self) {
        if let Some(width) = self.indent {
            self.output.push('\n');
            for _ in 0..self.depth.checked_mul(width).unwrap() {
                self.output.push(' ');
            }
        }
    }
}

pub(crate) fn quoted(output: &mut dyn Write, input: &str) -> fmt::Result {
    output.write_char('"')?;
    let mut start = 0;
    for (index, byte) in input.bytes().enumerate() {
        let escape = match byte {
            b'"' => "\\\"",
            b'\\' => "\\\\",
            b'\n' => "\\n",
            b'\r' => "\\r",
            b'\t' => "\\t",
            8 => "\\b",
            12 => "\\f",
            0..=31 => "",
            _ => continue,
        };
        output.write_str(&input[start..index])?;
        if escape.is_empty() {
            write!(output, "\\u{byte:04x}")?;
        } else {
            output.write_str(escape)?;
        }
        start = index + 1;
    }
    output.write_str(&input[start..])?;
    output.write_char('"')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_and_pretty_collections() {
        for indent in [None, Some(0), Some(2), Some(16)] {
            let mut writer = Writer::new(indent);
            writer.open(true);
            writer.item(0);
            writer.quoted("key");
            writer.colon();
            writer.open(false);
            writer.item(0);
            writer.integer(-7);
            writer.item(1);
            writer.quoted("中\n");
            writer.close(false, true);
            writer.item(1);
            writer.quoted("z");
            writer.colon();
            writer.open(true);
            writer.close(true, false);
            writer.close(true, true);
            let value = serde_json::json!({"key": [-7, "中\n"], "z": {}});
            let mut expected = Vec::new();
            if let Some(width) = indent {
                use serde::Serialize;
                let spaces = vec![b' '; width as usize];
                let formatter = serde_json::ser::PrettyFormatter::with_indent(&spaces);
                value
                    .serialize(&mut serde_json::Serializer::with_formatter(
                        &mut expected,
                        formatter,
                    ))
                    .unwrap();
            } else {
                serde_json::to_writer(&mut expected, &value).unwrap();
            }
            assert_eq!(writer.output.as_bytes(), expected);
        }
    }

    #[test]
    fn float_matches_language_display_and_rejects_nonfinite() {
        let mut writer = Writer::new(None);
        assert!(writer.float(-0.0));
        assert_eq!(writer.output, "-0");
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(!writer.float(value));
            assert_eq!(writer.output, "-0");
        }
    }

    #[test]
    fn json_escaping_matches_reference() {
        let controls: String = (0..=127).map(char::from).collect();
        for input in [
            "",
            "普通文本😀",
            "\"\\/\n\r\t",
            "é\0中\u{2028}\u{2029}",
            &controls,
        ] {
            let mut output = String::new();
            quoted(&mut output, input).unwrap();
            assert_eq!(output, serde_json::to_string(input).unwrap());
        }
    }

    #[test]
    fn writer_failure_propagates() {
        struct Fails;
        impl Write for Fails {
            fn write_str(&mut self, _: &str) -> fmt::Result {
                Err(fmt::Error)
            }
        }
        assert!(quoted(&mut Fails, "text").is_err());
    }
}
