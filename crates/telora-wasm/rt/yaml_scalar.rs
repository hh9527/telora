//! Telora scalar policy, independent of language TypeIds.
use crate::json_parse::Node;
use alloc::{string::String, vec::Vec};

pub(crate) fn scalar(text: &str) -> Result<Node, String> {
    Ok(match text {
        "" | "~" | "null" | "Null" | "NULL" => Node::Null,
        "true" | "True" | "TRUE" => Node::Bool(true),
        "false" | "False" | "FALSE" => Node::Bool(false),
        ".inf" | ".Inf" | ".INF" | "-.inf" | "-.Inf" | "-.INF" | ".nan" | ".NaN" | ".NAN" => {
            return Err("YAML Float must be finite".into());
        }
        _ => {
            let unsigned = text.trim_start_matches(['+', '-']);
            if !unsigned.is_empty()
                && (unsigned.bytes().all(|b| b.is_ascii_digit() || b == b'_')
                    || unsigned.starts_with("0x")
                    || unsigned.starts_with("0o"))
            {
                let normalized = text.replace('_', "");
                let (negative, unsigned) = normalized
                    .strip_prefix('-')
                    .map_or((false, normalized.as_str()), |v| (true, v));
                let unsigned = unsigned.strip_prefix('+').unwrap_or(unsigned);
                let (radix, digits) = if let Some(v) = unsigned.strip_prefix("0x") {
                    (16, v)
                } else if let Some(v) = unsigned.strip_prefix("0o") {
                    (8, v)
                } else {
                    (10, unsigned)
                };
                let magnitude =
                    i128::from_str_radix(digits, radix).map_err(|_| "invalid YAML integer")?;
                Node::Int(
                    i64::try_from(if negative { -magnitude } else { magnitude })
                        .map_err(|_| "YAML integer is outside the i64 range")?,
                )
            } else if text.contains(['.', 'e', 'E']) && text.chars().any(|c| c.is_ascii_digit()) {
                let value = text
                    .replace('_', "")
                    .parse::<f64>()
                    .map_err(|_| "invalid YAML Float")?;
                if !value.is_finite() {
                    return Err("YAML Float must be finite".into());
                }
                Node::Float(value)
            } else {
                Node::String(text.into())
            }
        }
    })
}

pub(crate) fn binary(text: &str) -> Result<Vec<u8>, String> {
    fn digit(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let input: Vec<_> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if input.is_empty() || input.len() % 4 != 0 {
        return Err("YAML !!binary contains invalid base64 data".into());
    }
    let mut output = Vec::new();
    for (index, chunk) in input.chunks_exact(4).enumerate() {
        let last = (index + 1) * 4 == input.len();
        let padding = usize::from(chunk[3] == b'=') + usize::from(chunk[2] == b'=');
        if padding > 0 && !last || chunk[2] == b'=' && chunk[3] != b'=' {
            return Err("YAML !!binary contains invalid base64 padding".into());
        }
        let a = digit(chunk[0]).ok_or("YAML !!binary contains invalid base64 data")?;
        let b = digit(chunk[1]).ok_or("YAML !!binary contains invalid base64 data")?;
        let c = if chunk[2] == b'=' {
            0
        } else {
            digit(chunk[2]).ok_or("YAML !!binary contains invalid base64 data")?
        };
        let d = if chunk[3] == b'=' {
            0
        } else {
            digit(chunk[3]).ok_or("YAML !!binary contains invalid base64 data")?
        };
        if padding == 2 && b & 15 != 0 || padding == 1 && c & 3 != 0 {
            return Err("YAML !!binary contains invalid base64 padding".into());
        }
        output.push((a << 2) | (b >> 4));
        if padding < 2 {
            output.push((b << 4) | (c >> 2));
        }
        if padding == 0 {
            output.push((c << 6) | d);
        }
    }
    Ok(output)
}
