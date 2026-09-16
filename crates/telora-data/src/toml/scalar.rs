use crate::json::{DataScalar, TemporalKind};
use alloc::{borrow::ToOwned, string::String};

pub(super) fn parse_number(text: &str) -> Result<DataScalar, &'static str> {
    if text
        .strip_prefix(['+', '-'])
        .is_some_and(|rest| rest.starts_with(['+', '-']))
    {
        return Err("invalid sign in TOML number");
    }
    validate_numeric_underscores(text)?;
    let normalized = text.replace('_', "");
    match normalized.as_str() {
        "inf" | "+inf" | "-inf" | "nan" | "+nan" | "-nan" => {
            return Err("TOML Float must be finite");
        }
        _ => {}
    }
    let unsigned_prefix = normalized.trim_start_matches(['+', '-']);
    let radix_prefixed = unsigned_prefix.starts_with("0x")
        || unsigned_prefix.starts_with("0o")
        || unsigned_prefix.starts_with("0b");
    if radix_prefixed && text.starts_with(['+', '-']) {
        return Err("TOML radix integers cannot have a sign");
    }
    if !radix_prefixed && normalized.contains(['.', 'e', 'E']) {
        if invalid_leading_zero(&normalized) {
            return Err("invalid leading zero in TOML Float");
        }
        validate_float_syntax(&normalized)?;
        let value = normalized.parse::<f64>().map_err(|_| "invalid TOML Float");
        return value.and_then(|value| {
            value
                .is_finite()
                .then_some(DataScalar::Float(value))
                .ok_or("TOML Float must be finite")
        });
    }
    let (negative, unsigned) = normalized
        .strip_prefix('-')
        .map_or((false, normalized.as_str()), |value| (true, value));
    let unsigned = unsigned.strip_prefix('+').unwrap_or(unsigned);
    let (radix, digits) = if let Some(digits) = unsigned.strip_prefix("0x") {
        (16, digits)
    } else if let Some(digits) = unsigned.strip_prefix("0o") {
        (8, digits)
    } else if let Some(digits) = unsigned.strip_prefix("0b") {
        (2, digits)
    } else {
        if unsigned.len() > 1 && unsigned.starts_with('0') {
            return Err("invalid leading zero in TOML integer");
        }
        (10, unsigned)
    };
    if digits.is_empty() {
        return Err("invalid TOML integer");
    }
    let magnitude = i128::from_str_radix(digits, radix).map_err(|_| "invalid TOML integer")?;
    let signed = if negative { -magnitude } else { magnitude };
    i64::try_from(signed)
        .map(DataScalar::Int)
        .map_err(|_| "TOML integer is outside the i64 range")
}

fn validate_float_syntax(value: &str) -> Result<(), &'static str> {
    let unsigned = value.trim_start_matches(['+', '-']);
    let (mantissa, exponent) = unsigned
        .split_once(['e', 'E'])
        .map_or((unsigned, None), |(mantissa, exponent)| {
            (mantissa, Some(exponent))
        });
    if unsigned.matches(['e', 'E']).count() > 1 {
        return Err("invalid TOML Float");
    }
    if let Some((whole, fraction)) = mantissa.split_once('.') {
        if whole.is_empty()
            || fraction.is_empty()
            || !whole.bytes().all(|byte| byte.is_ascii_digit())
            || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err("invalid TOML Float");
        }
    } else if mantissa.is_empty() || !mantissa.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("invalid TOML Float");
    }
    if let Some(exponent) = exponent {
        let digits = exponent.trim_start_matches(['+', '-']);
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("invalid TOML Float exponent");
        }
    }
    Ok(())
}

fn validate_numeric_underscores(value: &str) -> Result<(), &'static str> {
    let unsigned = value.trim_start_matches(['+', '-']);
    let radix = if unsigned.starts_with("0x") {
        16
    } else if unsigned.starts_with("0o") {
        8
    } else if unsigned.starts_with("0b") {
        2
    } else {
        10
    };
    let bytes = value.as_bytes();
    for (index, byte) in bytes.iter().copied().enumerate() {
        if byte != b'_' {
            continue;
        }
        let valid = |byte: u8| char::from(byte).is_digit(radix);
        if index == 0
            || index + 1 == bytes.len()
            || !valid(bytes[index - 1])
            || !valid(bytes[index + 1])
        {
            return Err("TOML numeric underscores must occur between digits");
        }
    }
    Ok(())
}

fn invalid_leading_zero(value: &str) -> bool {
    let value = value.trim_start_matches(['+', '-']);
    value.len() > 1
        && value.starts_with('0')
        && value.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
}

pub(super) fn parse_temporal(text: &str) -> Option<Result<(TemporalKind, String), &'static str>> {
    if text.len() >= 10
        && text.as_bytes().get(4) == Some(&b'-')
        && text.as_bytes().get(7) == Some(&b'-')
    {
        return Some(parse_date_time(text));
    }
    if text.len() >= 8
        && text.as_bytes().get(2) == Some(&b':')
        && text.as_bytes().get(5) == Some(&b':')
    {
        return Some(parse_time(text).map(|time| (TemporalKind::LocalTime, time)));
    }
    None
}

fn parse_date_time(text: &str) -> Result<(TemporalKind, String), &'static str> {
    let date = &text[..10];
    validate_date(date)?;
    if text.len() == 10 {
        return Ok((TemporalKind::LocalDate, date.to_owned()));
    }
    let separator = text.as_bytes()[10];
    if !matches!(separator, b'T' | b't' | b' ') {
        return Err("invalid TOML date-time separator");
    }
    let remainder = &text[11..];
    let (time, offset) = split_offset(remainder)?;
    let time = parse_time(time)?;
    if let Some(offset) = offset {
        let offset = canonical_offset(offset)?;
        Ok((
            TemporalKind::OffsetDateTime,
            format!("{date}T{time}{offset}"),
        ))
    } else {
        Ok((TemporalKind::LocalDateTime, format!("{date}T{time}")))
    }
}

fn split_offset(time: &str) -> Result<(&str, Option<&str>), &'static str> {
    if let Some(value) = time.strip_suffix(['Z', 'z']) {
        return Ok((value, Some("Z")));
    }
    if time.len() > 8
        && let Some(index) = time[8..].find(['+', '-']).map(|index| index + 8)
    {
        return Ok((&time[..index], Some(&time[index..])));
    }
    Ok((time, None))
}

fn validate_date(date: &str) -> Result<(), &'static str> {
    if date.len() != 10 {
        return Err("invalid TOML date");
    }
    let year = decimal(&date[0..4])?;
    let month = decimal(&date[5..7])?;
    let day = decimal(&date[8..10])?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return Err("invalid TOML month"),
    };
    if day == 0 || day > days {
        return Err("invalid TOML day");
    }
    Ok(())
}

fn parse_time(time: &str) -> Result<String, &'static str> {
    if time.len() < 8
        || time.as_bytes().get(2) != Some(&b':')
        || time.as_bytes().get(5) != Some(&b':')
    {
        return Err("invalid TOML time");
    }
    let hour = decimal(&time[0..2])?;
    let minute = decimal(&time[3..5])?;
    let second = decimal(&time[6..8])?;
    if hour > 23 || minute > 59 || second > 59 {
        return Err("TOML time component is outside its valid range");
    }
    if time.len() > 8 {
        let fraction = time.strip_prefix(&time[..8]).expect("prefix exists");
        if !fraction.starts_with('.')
            || fraction.len() == 1
            || !fraction[1..].bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err("invalid TOML fractional second");
        }
    }
    Ok(time.to_owned())
}

fn canonical_offset(offset: &str) -> Result<String, &'static str> {
    if matches!(offset, "Z" | "z") {
        return Ok("Z".into());
    }
    if offset.len() != 6 || offset.as_bytes().get(3) != Some(&b':') {
        return Err("invalid TOML date-time offset");
    }
    let hour = decimal(&offset[1..3])?;
    let minute = decimal(&offset[4..6])?;
    if hour > 23 || minute > 59 || !matches!(offset.as_bytes()[0], b'+' | b'-') {
        return Err("TOML date-time offset is outside its valid range");
    }
    if hour == 0 && minute == 0 {
        Ok("Z".into())
    } else {
        Ok(offset.to_owned())
    }
}

fn decimal(value: &str) -> Result<u32, &'static str> {
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("invalid decimal digits in TOML temporal value");
    }
    value.parse().map_err(|_| "invalid TOML temporal value")
}
