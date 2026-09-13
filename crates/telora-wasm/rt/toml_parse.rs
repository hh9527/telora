//! TOML parsing has no language types; raw spans preserve temporal precision.
use crate::json_parse::{Node, Plan, TemporalKind};
use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use toml::de::{DeTable, DeValue};

pub(crate) fn parse(input: &str) -> Result<Plan, String> {
    let table = DeTable::parse(input).map_err(|error| error.to_string())?;
    let mut plan = Plan {
        nodes: Vec::new(),
        root: 0,
    };
    plan.root = append(
        &mut plan,
        DeValue::Table(table.into_inner()),
        input,
        input,
        0,
    )?;
    Ok(plan)
}

fn append(
    plan: &mut Plan,
    value: DeValue<'_>,
    input: &str,
    raw: &str,
    depth: usize,
) -> Result<u32, String> {
    if depth > 512 {
        return Err("TOML nesting limit".into());
    }
    let node = match value {
        DeValue::String(value) => Node::String(value.into_owned()),
        DeValue::Boolean(value) => Node::Bool(value),
        DeValue::Integer(value) => Node::Int(
            i64::from_str_radix(value.as_str(), value.radix())
                .map_err(|_| "TOML Int must fit in i64")?,
        ),
        DeValue::Float(value) => {
            let value = value
                .as_str()
                .parse::<f64>()
                .map_err(|_| "invalid TOML Float")?;
            if !value.is_finite() {
                return Err("TOML Float must be finite".into());
            }
            Node::Float(value)
        }
        DeValue::Datetime(value) => {
            if let Some(date) = value.date {
                let leap = date.year % 4 == 0 && (date.year % 100 != 0 || date.year % 400 == 0);
                let days = match date.month {
                    1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
                    4 | 6 | 9 | 11 => 30,
                    2 if leap => 29,
                    2 => 28,
                    _ => return Err("invalid TOML month".into()),
                };
                if date.day == 0 || date.day > days {
                    return Err("invalid TOML day".into());
                }
            }
            if let Some(time) = value.time {
                let clock = if value.date.is_some() {
                    &raw[11..]
                } else {
                    raw
                };
                if clock.len() < 8
                    || clock.as_bytes().get(2) != Some(&b':')
                    || clock.as_bytes().get(5) != Some(&b':')
                {
                    return Err("invalid TOML time".into());
                }
                if time.hour > 23 || time.minute > 59 || time.second > 59 {
                    return Err("TOML time component is outside its valid range".into());
                }
            }
            let mut text = raw.to_string();
            let kind = match (value.date, value.time, value.offset) {
                (Some(_), None, None) => TemporalKind::LocalDate,
                (None, Some(_), None) => TemporalKind::LocalTime,
                (Some(_), Some(_), offset) => {
                    text.replace_range(10..11, "T");
                    if offset.is_some() {
                        if text.ends_with('z') {
                            text.pop();
                            text.push('Z');
                        }
                        if text.ends_with("+00:00") || text.ends_with("-00:00") {
                            text.truncate(text.len() - 6);
                            text.push('Z');
                        }
                        TemporalKind::OffsetDateTime
                    } else {
                        TemporalKind::LocalDateTime
                    }
                }
                _ => return Err("invalid TOML temporal value".into()),
            };
            Node::Temporal(kind, text)
        }
        DeValue::Array(values) => {
            let mut children = Vec::new();
            for value in values {
                let raw = input.get(value.span()).ok_or("invalid TOML value span")?;
                children.push(append(plan, value.into_inner(), input, raw, depth + 1)?);
            }
            Node::Array(children)
        }
        DeValue::Table(values) => {
            let mut fields = Vec::new();
            for (key, value) in values {
                let raw = input.get(value.span()).ok_or("invalid TOML value span")?;
                fields.push((
                    key.into_inner().into_owned(),
                    append(plan, value.into_inner(), input, raw, depth + 1)?,
                ));
            }
            fields.sort_by(|a, b| a.0.cmp(&b.0));
            Node::Object(fields)
        }
    };
    let id = u32::try_from(plan.nodes.len()).map_err(|_| "TOML node count exceeds wasm32")?;
    plan.nodes.push(node);
    Ok(id)
}
