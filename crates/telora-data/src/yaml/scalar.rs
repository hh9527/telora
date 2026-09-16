use super::build::Build;
use super::structure::{Scalar, Text};
use crate::source::{Diagnostic, Location};
use alloc::borrow::Cow;

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
pub(super) fn parse_yaml_int(text: &str) -> Result<i64, &'static str> {
    let normalized = normalize_number(text);
    let (negative, unsigned) = normalized
        .strip_prefix('-')
        .map_or((false, normalized.as_ref()), |v| (true, v));
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

pub(super) fn normalize_number(text: &str) -> Cow<'_, str> {
    if text.contains('_') {
        Cow::Owned(text.replace('_', ""))
    } else {
        Cow::Borrowed(text)
    }
}

fn string(build: &mut Build, text: &str, loc: Location) -> Result<Text, Diagnostic> {
    let mut length = 0;
    if text.starts_with(['\'', '"']) {
        let escaped = quoted(text, loc, |piece, local| {
            build.admit(&mut length, piece.len(), false, local)
        })?;
        Ok(if escaped {
            Text::Quoted(loc.range())
        } else {
            Text::Source(loc.start as usize + 1..loc.end as usize - 1)
        })
    } else {
        build.admit(&mut length, text.len(), false, loc)?;
        Ok(Text::Source(loc.range()))
    }
}

pub(super) fn key(build: &mut Build, text: &str, loc: Location) -> Result<Text, Diagnostic> {
    if text == "<<" {
        return Err(Diagnostic::error("YAML merge keys are not supported", loc));
    }
    build.unsupported(text, loc)?;
    if text.is_empty()
        || text.starts_with(['[', '{', '?', '!'])
        || (!text.starts_with(['\'', '"']) && core_non_string(text))
    {
        return Err(Diagnostic::error("YAML mapping keys must be Strings", loc));
    }
    string(build, text, loc)
}

pub(super) fn value(build: &mut Build, text: &str, loc: Location) -> Result<Scalar, Diagnostic> {
    build.unsupported(text, loc)?;
    if let Some(encoded) = text.strip_prefix("!!binary") {
        let encoded = encoded.trim();
        let mut length = 0;
        binary(encoded, loc, |bytes| {
            build.admit(&mut length, bytes.len(), true, loc)
        })?;
        return Ok(Scalar::Bytes(
            loc.end as usize - encoded.len()..loc.end as usize,
        ));
    }
    if text.starts_with('!') {
        return Err(Diagnostic::error("custom YAML tags are not supported", loc));
    }
    if text.starts_with(['\'', '"']) {
        return string(build, text, loc).map(Scalar::String);
    }
    Ok(match text {
        "" | "~" | "null" | "Null" | "NULL" => Scalar::Null,
        "true" | "True" | "TRUE" => Scalar::Bool(true),
        "false" | "False" | "FALSE" => Scalar::Bool(false),
        _ if looks_integer(text) => Scalar::Number {
            range: loc.range(),
            float: false,
        },
        _ if looks_float(text)
            || matches!(
                text,
                ".inf" | ".Inf" | ".INF" | "-.inf" | "-.Inf" | "-.INF" | ".nan" | ".NaN" | ".NAN"
            ) =>
        {
            Scalar::Number {
                range: loc.range(),
                float: true,
            }
        }
        _ => Scalar::String(string(build, text, loc)?),
    })
}

pub(super) fn quoted(
    text: &str,
    loc: Location,
    mut emit: impl FnMut(&str, Location) -> Result<(), Diagnostic>,
) -> Result<bool, Diagnostic> {
    use super::lexer::{Kind, Scanner};
    let mut scanner = Scanner::new(text);
    let opening = scanner.next();
    debug_assert_eq!(opening.map(|t| t.kind), Some(Kind::Start));
    let mut escaped = false;
    while let Some(token) = scanner.next() {
        let start = token.span.start;
        let local = Location::from_usize(
            loc.source,
            loc.start as usize + start..loc.start as usize + scanner.pos,
        )
        .unwrap();
        let ch = match token.kind {
            Kind::Text => {
                emit(&text[token.span], local)?;
                continue;
            }
            Kind::End if scanner.pos == text.len() => return Ok(escaped),
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
                            Diagnostic::error(
                                "invalid YAML Unicode escape",
                                Location::from_usize(
                                    loc.source,
                                    loc.start as usize + start..loc.start as usize + scanner.pos,
                                )
                                .unwrap(),
                            )
                        })?;
                        value = value * 16 + digit;
                    }
                    char::from_u32(value).ok_or_else(|| {
                        Diagnostic::error(
                            "invalid YAML Unicode scalar",
                            Location::from_usize(
                                loc.source,
                                loc.start as usize + start..loc.start as usize + scanner.pos,
                            )
                            .unwrap(),
                        )
                    })?
                }
                _ => return Err(Diagnostic::error("invalid YAML escape", local)),
            },
            _ => unreachable!("quoted scanner remains in string mode until End"),
        };
        escaped = true;
        emit(
            ch.encode_utf8(&mut [0; 4]),
            Location::from_usize(
                loc.source,
                loc.start as usize + start..loc.start as usize + scanner.pos,
            )
            .unwrap(),
        )?;
    }
    Err(Diagnostic::error("unclosed YAML string", loc))
}

pub(super) fn binary(
    text: &str,
    loc: Location,
    mut emit: impl FnMut(&[u8]) -> Result<(), Diagnostic>,
) -> Result<(), Diagnostic> {
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
        emit(&bytes[..3 - padding])?;
    }
    if !any {
        return Err(error());
    }
    Ok(())
}
