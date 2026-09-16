use super::build::Build;
use crate::{
    json::DataScalar,
    source::{Diagnostic, Location},
};
use alloc::{string::String, vec::Vec};

fn looks_integer(text: &str) -> bool {
    let unsigned = text.trim_start_matches(['+', '-']);
    !unsigned.is_empty()
        && (unsigned.bytes().all(|b| b.is_ascii_digit() || b == b'_')
            || unsigned.starts_with("0x")
            || unsigned.starts_with("0o"))
}
fn looks_float(text: &str) -> bool {
    text.contains(['.', 'e', 'E']) && text.chars().any(|ch| ch.is_ascii_digit())
}
fn parse_yaml_int(text: &str) -> Result<i64, &'static str> {
    let normalized = text.replace('_', "");
    let (negative, unsigned) = normalized
        .strip_prefix('-')
        .map_or((false, normalized.as_str()), |v| (true, v));
    let unsigned = unsigned.strip_prefix('+').unwrap_or(unsigned);
    let (radix, digits) = unsigned.strip_prefix("0x").map_or_else(
        || {
            unsigned
                .strip_prefix("0o")
                .map_or((10, unsigned), |v| (8, v))
        },
        |v| (16, v),
    );
    let magnitude = i128::from_str_radix(digits, radix).map_err(|_| "invalid YAML integer")?;
    i64::try_from(if negative { -magnitude } else { magnitude })
        .map_err(|_| "YAML integer is outside the i64 range")
}
fn core_non_string(text: &str) -> bool {
    matches!(
        text,
        "~" | "null"
            | "Null"
            | "NULL"
            | "true"
            | "True"
            | "TRUE"
            | "false"
            | "False"
            | "FALSE"
            | ".inf"
            | ".Inf"
            | ".INF"
            | "-.inf"
            | "-.Inf"
            | "-.INF"
            | ".nan"
            | ".NaN"
            | ".NAN"
    ) || looks_integer(text)
        || text.parse::<f64>().is_ok()
}

pub(super) fn key(build: &mut Build, text: &str, loc: Location) -> Result<String, Diagnostic> {
    if text == "<<" {
        return Err(Diagnostic::error("YAML merge keys are not supported", loc));
    }
    build.unsupported(text, loc)?;
    if text.is_empty() || text.starts_with(['[', '{', '?', '!']) {
        return Err(Diagnostic::error("YAML mapping keys must be Strings", loc));
    }
    if text.starts_with(['\'', '"']) {
        return quoted(build, text, loc);
    }
    if core_non_string(text) {
        return Err(Diagnostic::error("YAML mapping keys must be Strings", loc));
    }
    let mut value = String::new();
    build.append(&mut value, text, loc)?;
    Ok(value)
}

pub(super) fn value(
    build: &mut Build,
    text: &str,
    loc: Location,
) -> Result<DataScalar, Diagnostic> {
    build.unsupported(text, loc)?;
    if let Some(encoded) = text.strip_prefix("!!binary") {
        return binary(build, encoded.trim(), loc).map(DataScalar::Bytes);
    }
    if text.starts_with('!') {
        return Err(Diagnostic::error("custom YAML tags are not supported", loc));
    }
    if text.starts_with(['\'', '"']) {
        return quoted(build, text, loc).map(DataScalar::String);
    }
    Ok(match text {
        "" | "~" | "null" | "Null" | "NULL" => DataScalar::Null,
        "true" | "True" | "TRUE" => DataScalar::Bool(true),
        "false" | "False" | "FALSE" => DataScalar::Bool(false),
        ".inf" | ".Inf" | ".INF" | "-.inf" | "-.Inf" | "-.INF" | ".nan" | ".NaN" | ".NAN" => {
            return Err(Diagnostic::error("YAML Float must be finite", loc));
        }
        _ if looks_integer(text) => {
            DataScalar::Int(parse_yaml_int(text).map_err(|m| Diagnostic::error(m, loc))?)
        }
        _ if looks_float(text) => {
            let number = text
                .replace('_', "")
                .parse::<f64>()
                .map_err(|_| Diagnostic::error("invalid YAML Float", loc))?;
            if !number.is_finite() {
                return Err(Diagnostic::error("YAML Float must be finite", loc));
            }
            DataScalar::Float(number)
        }
        _ => {
            let mut value = String::new();
            build.append(&mut value, text, loc)?;
            DataScalar::String(value)
        }
    })
}

fn quoted(build: &mut Build, text: &str, loc: Location) -> Result<String, Diagnostic> {
    use super::lexer::{Kind, Scanner};
    let mut scanner = Scanner::new(text);
    let opening = scanner.next();
    debug_assert_eq!(opening.map(|t| t.kind), Some(Kind::Start));
    let mut output = String::new();
    while let Some(token) = scanner.next() {
        let start = token.span.start;
        let local = build.loc(loc.start as usize + start..loc.start as usize + scanner.pos);
        let ch = match token.kind {
            Kind::Text => {
                build.append(&mut output, &text[token.span], local)?;
                continue;
            }
            Kind::End if scanner.pos == text.len() => return Ok(output),
            Kind::End => {
                return Err(Diagnostic::error(
                    "unexpected content after quoted YAML String",
                    local,
                ));
            }
            Kind::Quote => '\'',
            Kind::Escape(None) => return Err(Diagnostic::error("unterminated YAML escape", loc)),
            Kind::Escape(Some(escaped)) => match escaped {
                '0' => '\0',
                'a' => '\u{7}',
                'b' => '\u{8}',
                't' | '\t' => '\t',
                'n' => '\n',
                'v' => '\u{b}',
                'f' => '\u{c}',
                'r' => '\r',
                'e' => '\u{1b}',
                '"' => '"',
                '/' => '/',
                '\\' => '\\',
                'x' | 'u' | 'U' => {
                    let digits = match escaped {
                        'x' => 2,
                        'u' => 4,
                        _ => 8,
                    };
                    let mut value = 0u32;
                    for _ in 0..digits {
                        let c = scanner.character().ok_or_else(|| {
                            Diagnostic::error("incomplete YAML Unicode escape", loc)
                        })?;
                        let digit = c.to_digit(16).ok_or_else(|| {
                            build.error(
                                loc.start as usize + start..loc.start as usize + scanner.pos,
                                "invalid YAML Unicode escape",
                            )
                        })?;
                        value = value * 16 + digit;
                    }
                    char::from_u32(value).ok_or_else(|| {
                        build.error(
                            loc.start as usize + start..loc.start as usize + scanner.pos,
                            "invalid YAML Unicode scalar",
                        )
                    })?
                }
                _ => return Err(Diagnostic::error("invalid YAML escape", local)),
            },
            _ => unreachable!("quoted scanner remains in string mode until End"),
        };
        build.append(
            &mut output,
            ch.encode_utf8(&mut [0; 4]),
            build.loc(loc.start as usize + start..loc.start as usize + scanner.pos),
        )?;
    }
    Err(Diagnostic::error("unclosed YAML string", loc))
}

fn binary(build: &mut Build, text: &str, loc: Location) -> Result<Vec<u8>, Diagnostic> {
    let digit = |b: u8| match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let error = || {
        Diagnostic::error(
            "YAML !!binary contains invalid or non-canonical base64 data",
            loc,
        )
    };
    let mut input = text.bytes().filter(|b| !b.is_ascii_whitespace());
    let mut output = Vec::new();
    let mut any = false;
    while let Some(a) = input.next() {
        any = true;
        let b = input.next().ok_or_else(error)?;
        let c = input.next().ok_or_else(error)?;
        let d = input.next().ok_or_else(error)?;
        let padding = usize::from(d == b'=') + usize::from(c == b'=');
        if c == b'=' && d != b'=' || padding > 0 && input.clone().next().is_some() {
            return Err(error());
        }
        let a = digit(a).ok_or_else(error)?;
        let b = digit(b).ok_or_else(error)?;
        let c = if c == b'=' {
            0
        } else {
            digit(c).ok_or_else(error)?
        };
        let d = if d == b'=' {
            0
        } else {
            digit(d).ok_or_else(error)?
        };
        if padding == 2 && b & 15 != 0 || padding == 1 && c & 3 != 0 {
            return Err(error());
        }
        let bytes = [(a << 2) | (b >> 4), (b << 4) | (c >> 2), (c << 6) | d];
        build.bytes(&mut output, &bytes[..3 - padding], loc)?;
    }
    if !any {
        return Err(error());
    }
    Ok(output)
}
