use crate::{CallContext, NativeError, NativeType, ValueRef};
use std::collections::HashMap;
use std::fmt::Write;
use std::mem::size_of;
use std::sync::Arc;

#[derive(Clone, Debug, Eq, PartialEq)]
enum TemplatePart {
    Text(String),
    Field(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DisplayTemplate(Vec<TemplatePart>);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Fmt(Arc<FmtNode>);

#[derive(Debug, Eq, PartialEq)]
enum FmtNode {
    String(String),
    Int(i64),
    Float(u64),
    Atom(String),
    Concat {
        strings: Vec<String>,
        items: Vec<Fmt>,
    },
}

impl Fmt {
    fn string(value: String) -> Self {
        Self(Arc::new(FmtNode::String(value)))
    }

    fn int(value: i64) -> Self {
        Self(Arc::new(FmtNode::Int(value)))
    }

    fn float(value: f64) -> Self {
        Self(Arc::new(FmtNode::Float(value.to_bits())))
    }

    fn atom(value: String) -> Self {
        Self(Arc::new(FmtNode::Atom(value)))
    }

    fn concat(strings: Vec<String>, items: Vec<Self>) -> Self {
        Self(Arc::new(FmtNode::Concat { strings, items }))
    }
}

fn fmt_type(context: &CallContext<'_, '_>) -> Result<NativeType, NativeError> {
    context
        .value(context.upvalue(0)?)?
        .as_native_type()
        .cloned()
        .ok_or_else(|| NativeError::new("Fmt native type is not linked"))
}

fn fmt_argument(context: &CallContext<'_, '_>, index: usize) -> Result<Fmt, NativeError> {
    fmt_value(context.value(context.argument(index)?)?).cloned()
}

fn fmt_value(value: ValueRef<'_>) -> Result<&Fmt, NativeError> {
    let native_type = value
        .opaque_native_type()
        .ok_or_else(|| NativeError::new("expected std/fmt#Fmt"))?;
    if native_type.qualified_name() != "std/fmt#Fmt" {
        return Err(NativeError::new("expected std/fmt#Fmt"));
    }
    value
        .as_opaque::<Fmt>(native_type)
        .ok_or_else(|| NativeError::new("expected std/fmt#Fmt"))
}

struct ByteCounter(Option<usize>);

impl Write for ByteCounter {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        self.0 = self.0.and_then(|length| length.checked_add(text.len()));
        self.0.map_or(Err(std::fmt::Error), |_| Ok(()))
    }
}

fn formatted_len(value: impl std::fmt::Display) -> Result<usize, NativeError> {
    let mut counter = ByteCounter(Some(0));
    let _ = write!(&mut counter, "{value}");
    counter
        .0
        .ok_or_else(|| NativeError::allocation_limit("Fmt rendered size overflowed"))
}

pub(crate) fn string_with_capacity(length: usize) -> Result<String, NativeError> {
    let mut output = String::new();
    output
        .try_reserve_exact(length)
        .map_err(|_| NativeError::allocation_limit("Fmt String allocation cannot be reserved"))?;
    Ok(output)
}

fn copy_string(value: &str) -> Result<String, NativeError> {
    let mut output = string_with_capacity(value.len())?;
    output.push_str(value);
    Ok(output)
}

#[derive(Clone, Copy)]
struct FmtMeasurement {
    bytes: usize,
    height: usize,
}

fn measure_fmt(value: &Fmt) -> Result<usize, NativeError> {
    fn add(left: usize, right: usize) -> Result<usize, NativeError> {
        left.checked_add(right)
            .ok_or_else(|| NativeError::allocation_limit("Fmt rendered size overflowed"))
    }

    fn visit(
        value: &Fmt,
        memo: &mut HashMap<*const FmtNode, FmtMeasurement>,
    ) -> Result<FmtMeasurement, NativeError> {
        let key = Arc::as_ptr(&value.0);
        if let Some(measurement) = memo.get(&key) {
            return Ok(*measurement);
        }
        let measurement = match value.0.as_ref() {
            FmtNode::String(value) | FmtNode::Atom(value) => FmtMeasurement {
                bytes: value.len(),
                height: 1,
            },
            FmtNode::Int(value) => FmtMeasurement {
                bytes: formatted_len(value)?,
                height: 1,
            },
            FmtNode::Float(value) => FmtMeasurement {
                bytes: formatted_len(f64::from_bits(*value))?,
                height: 1,
            },
            FmtNode::Concat { strings, items } => {
                let mut bytes = strings
                    .iter()
                    .try_fold(0usize, |total, text| add(total, text.len()))?;
                let mut child_height = 0;
                for item in items {
                    let child = visit(item, memo)?;
                    bytes = add(bytes, child.bytes)?;
                    child_height = child_height.max(child.height);
                }
                FmtMeasurement {
                    bytes,
                    height: child_height.checked_add(1).ok_or_else(|| {
                        NativeError::new("std/fmt value exceeds the recursive rendering limit")
                    })?,
                }
            }
        };
        if measurement.height > 128 {
            return Err(NativeError::new(
                "std/fmt value exceeds the recursive rendering limit",
            ));
        }
        memo.insert(key, measurement);
        Ok(measurement)
    }

    Ok(visit(value, &mut HashMap::new())?.bytes)
}

fn write_fmt(value: &Fmt, output: &mut String, depth: usize) -> Result<(), NativeError> {
    if depth >= 128 {
        return Err(NativeError::new(
            "std/fmt value exceeds the recursive rendering limit",
        ));
    }
    match value.0.as_ref() {
        FmtNode::String(value) | FmtNode::Atom(value) => output.push_str(value),
        FmtNode::Int(value) => write!(output, "{value}").expect("writing to String cannot fail"),
        FmtNode::Float(value) => {
            write!(output, "{}", f64::from_bits(*value)).expect("writing to String cannot fail")
        }
        FmtNode::Concat { strings, items } => {
            for (index, item) in items.iter().enumerate() {
                output.push_str(&strings[index]);
                write_fmt(item, output, depth + 1)?;
            }
            output.push_str(&strings[items.len()]);
        }
    }
    Ok(())
}

pub(crate) fn interpolation_value_len(value: ValueRef<'_>) -> Result<usize, NativeError> {
    if let Some(value) = value.as_str() {
        return Ok(value.as_str().len());
    }
    if let Some(value) = value.as_int() {
        return formatted_len(value);
    }
    if let Some(value) = value.as_float() {
        return formatted_len(value);
    }
    if let Some(value) = value.as_atom() {
        return Ok(value.as_str().len());
    }
    let native_type = value
        .opaque_native_type()
        .ok_or_else(|| NativeError::new("string interpolation expected std/fmt#Fmt"))?;
    if native_type.qualified_name() != "std/fmt#Fmt" {
        return Err(NativeError::new(
            "string interpolation expected std/fmt#Fmt",
        ));
    }
    measure_fmt(
        value
            .as_opaque::<Fmt>(native_type)
            .ok_or_else(|| NativeError::new("string interpolation expected std/fmt#Fmt"))?,
    )
}

fn fmt_payload_bytes(nested: usize) -> Result<usize, NativeError> {
    size_of::<Fmt>()
        .checked_add(4 * size_of::<usize>())
        .and_then(|bytes| bytes.checked_add(size_of::<FmtNode>()))
        .and_then(|bytes| bytes.checked_add(nested))
        .ok_or_else(|| NativeError::allocation_limit("Fmt payload size overflowed"))
}

fn fmt_concat_payload_bytes(
    string_count: usize,
    item_count: usize,
    text_bytes: usize,
) -> Result<usize, NativeError> {
    let string_slots = string_count
        .checked_mul(size_of::<String>())
        .ok_or_else(|| NativeError::allocation_limit("Fmt payload size overflowed"))?;
    let item_slots = item_count
        .checked_mul(size_of::<Fmt>())
        .ok_or_else(|| NativeError::allocation_limit("Fmt payload size overflowed"))?;
    let nested = string_slots
        .checked_add(item_slots)
        .and_then(|bytes| bytes.checked_add(text_bytes))
        .ok_or_else(|| NativeError::allocation_limit("Fmt payload size overflowed"))?;
    fmt_payload_bytes(nested)
}

fn commit_fmt(
    context: &mut CallContext<'_, '_>,
    native_type: NativeType,
    value: Fmt,
    reservation: crate::vm::OpaqueAllocationReservation,
) -> Result<(), NativeError> {
    context.set_opaque_reserved(context.result(), native_type, value, reservation)
}

pub(crate) fn write_interpolation_value(
    value: ValueRef<'_>,
    output: &mut String,
) -> Result<(), NativeError> {
    if let Some(value) = value.as_str() {
        output.push_str(value.as_str());
        return Ok(());
    }
    if let Some(value) = value.as_int() {
        write!(output, "{value}").expect("writing to String cannot fail");
        return Ok(());
    }
    if let Some(value) = value.as_float() {
        write!(output, "{value}").expect("writing to String cannot fail");
        return Ok(());
    }
    if let Some(value) = value.as_atom() {
        output.push_str(value.as_str());
        return Ok(());
    }
    let native_type = value
        .opaque_native_type()
        .ok_or_else(|| NativeError::new("string interpolation expected std/fmt#Fmt"))?;
    if native_type.qualified_name() != "std/fmt#Fmt" {
        return Err(NativeError::new(
            "string interpolation expected std/fmt#Fmt",
        ));
    }
    write_fmt(
        value
            .as_opaque::<Fmt>(native_type)
            .ok_or_else(|| NativeError::new("string interpolation expected std/fmt#Fmt"))?,
        output,
        0,
    )
}

#[derive(Clone, Copy)]
struct TemplateMeasurement {
    part_count: usize,
}

fn validate_template_field(field: &str) -> Result<(), NativeError> {
    if field.is_empty()
        || !field
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        || field.starts_with(|ch: char| ch.is_ascii_digit())
    {
        return Err(NativeError::new(format!(
            "invalid Display template field {field:?}"
        )));
    }
    Ok(())
}

fn measure_template(source: &str) -> Result<TemplateMeasurement, NativeError> {
    let mut part_count = 0usize;
    let mut text_bytes = 0usize;
    let mut current_text_bytes = 0usize;
    let mut chars = source.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        match ch {
            '{' if chars.peek().is_some_and(|(_, next)| *next == '{') => {
                chars.next();
                current_text_bytes = current_text_bytes.checked_add(1).ok_or_else(|| {
                    NativeError::allocation_limit("DisplayTemplate payload size overflowed")
                })?;
            }
            '}' if chars.peek().is_some_and(|(_, next)| *next == '}') => {
                chars.next();
                current_text_bytes = current_text_bytes.checked_add(1).ok_or_else(|| {
                    NativeError::allocation_limit("DisplayTemplate payload size overflowed")
                })?;
            }
            '{' => {
                if current_text_bytes != 0 {
                    part_count = part_count.checked_add(1).ok_or_else(|| {
                        NativeError::allocation_limit("DisplayTemplate part count overflowed")
                    })?;
                    text_bytes = text_bytes.checked_add(current_text_bytes).ok_or_else(|| {
                        NativeError::allocation_limit("DisplayTemplate payload size overflowed")
                    })?;
                    current_text_bytes = 0;
                }
                let field_start = chars.peek().map_or(source.len(), |(index, _)| *index);
                let field_end = loop {
                    match chars.next() {
                        Some((index, '}')) => break index,
                        Some((_, '{')) => {
                            return Err(NativeError::new("nested '{' in Display template field"));
                        }
                        Some(_) => {}
                        None => return Err(NativeError::new("unclosed Display template field")),
                    }
                };
                let field = &source[field_start..field_end];
                validate_template_field(field)?;
                part_count = part_count.checked_add(1).ok_or_else(|| {
                    NativeError::allocation_limit("DisplayTemplate part count overflowed")
                })?;
                text_bytes = text_bytes.checked_add(field.len()).ok_or_else(|| {
                    NativeError::allocation_limit("DisplayTemplate payload size overflowed")
                })?;
            }
            '}' => return Err(NativeError::new("unmatched '}' in Display template")),
            ch => {
                current_text_bytes =
                    current_text_bytes
                        .checked_add(ch.len_utf8())
                        .ok_or_else(|| {
                            NativeError::allocation_limit("DisplayTemplate payload size overflowed")
                        })?;
            }
        }
    }
    if current_text_bytes != 0 {
        part_count = part_count.checked_add(1).ok_or_else(|| {
            NativeError::allocation_limit("DisplayTemplate part count overflowed")
        })?;
        text_bytes.checked_add(current_text_bytes).ok_or_else(|| {
            NativeError::allocation_limit("DisplayTemplate payload size overflowed")
        })?;
    }
    Ok(TemplateMeasurement { part_count })
}

fn push_template_part(
    parts: &mut Vec<TemplatePart>,
    part: TemplatePart,
) -> Result<(), NativeError> {
    parts.try_reserve(1).map_err(|_| {
        NativeError::allocation_limit("DisplayTemplate part allocation cannot be reserved")
    })?;
    parts.push(part);
    Ok(())
}

fn push_template_char(text: &mut String, ch: char) -> Result<(), NativeError> {
    text.try_reserve(ch.len_utf8()).map_err(|_| {
        NativeError::allocation_limit("DisplayTemplate text allocation cannot be reserved")
    })?;
    text.push(ch);
    Ok(())
}

fn parse_template(
    source: &str,
    measurement: TemplateMeasurement,
) -> Result<DisplayTemplate, NativeError> {
    let mut parts = Vec::new();
    parts
        .try_reserve_exact(measurement.part_count)
        .map_err(|_| {
            NativeError::allocation_limit("DisplayTemplate part allocation cannot be reserved")
        })?;
    let mut text = String::new();
    let mut chars = source.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        match ch {
            '{' if chars.peek().is_some_and(|(_, next)| *next == '{') => {
                chars.next();
                push_template_char(&mut text, '{')?;
            }
            '}' if chars.peek().is_some_and(|(_, next)| *next == '}') => {
                chars.next();
                push_template_char(&mut text, '}')?;
            }
            '{' => {
                if !text.is_empty() {
                    push_template_part(&mut parts, TemplatePart::Text(std::mem::take(&mut text)))?;
                }
                let mut field = String::new();
                loop {
                    match chars.next() {
                        Some((_, '}')) => break,
                        Some((_, '{')) => {
                            return Err(NativeError::new("nested '{' in Display template field"));
                        }
                        Some((_, ch)) => push_template_char(&mut field, ch)?,
                        None => return Err(NativeError::new("unclosed Display template field")),
                    }
                }
                validate_template_field(&field)?;
                push_template_part(&mut parts, TemplatePart::Field(field))?;
            }
            '}' => return Err(NativeError::new("unmatched '}' in Display template")),
            ch => push_template_char(&mut text, ch)?,
        }
    }
    if !text.is_empty() {
        push_template_part(&mut parts, TemplatePart::Text(text))?;
    }
    debug_assert_eq!(parts.len(), measurement.part_count);
    Ok(DisplayTemplate(parts))
}

pub(crate) fn native_prepare(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let source_argument = context.argument(0)?;
    let source = context
        .value(source_argument)?
        .as_str()
        .ok_or_else(|| NativeError::new("std/fmt.display_by expects String"))?;
    let measurement = measure_template(source.as_str())?;
    let source = context
        .value(source_argument)?
        .as_str()
        .expect("String argument was validated before reservation");
    let template = parse_template(source.as_str(), measurement)?;

    set_template_parts(context, template)
}

fn set_template_parts(
    context: &mut CallContext<'_, '_>,
    template: DisplayTemplate,
) -> Result<(), NativeError> {
    let mut strings = vec![String::new()];
    let mut fields = Vec::new();
    for part in template.0 {
        match part {
            TemplatePart::Text(text) => strings
                .last_mut()
                .expect("Display template always has a trailing String")
                .push_str(&text),
            TemplatePart::Field(field) => {
                fields.push(field);
                strings.push(String::new());
            }
        }
    }

    let mut string_registers = Vec::new();
    string_registers
        .try_reserve_exact(strings.len())
        .map_err(|_| {
            NativeError::allocation_limit("Display template String registers cannot be reserved")
        })?;
    for text in strings {
        let register = context.scratch()?;
        context.set_string(register, text)?;
        string_registers.push(register);
    }
    let strings = context.scratch()?;
    context.make_array(strings, &string_registers)?;

    let mut field_registers = Vec::new();
    field_registers
        .try_reserve_exact(fields.len())
        .map_err(|_| {
            NativeError::allocation_limit("Display template field registers cannot be reserved")
        })?;
    for field in fields {
        let register = context.scratch()?;
        context.set_string(register, field)?;
        field_registers.push(register);
    }
    let fields = context.scratch()?;
    context.make_array(fields, &field_registers)?;
    context.make_tuple(context.result(), &[strings, fields])
}

pub(crate) fn native_from_string(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = fmt_type(context)?;
    let argument = context.argument(0)?;
    let length = context
        .value(argument)?
        .unwrap_declared()
        .and_then(ValueRef::as_str)
        .ok_or_else(|| NativeError::new("std/fmt.from_string expects String"))?
        .as_str()
        .len();
    let reservation = context.reserve_opaque_allocation(fmt_payload_bytes(length)?)?;
    let text = context
        .value(argument)?
        .unwrap_declared()
        .and_then(ValueRef::as_str)
        .expect("String argument was validated before reservation");
    let value = copy_string(text.as_str())?;
    commit_fmt(context, native_type, Fmt::string(value), reservation)
}

pub(crate) fn native_from_int(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = fmt_type(context)?;
    let value = context
        .value(context.argument(0)?)?
        .unwrap_declared()
        .and_then(ValueRef::as_int)
        .ok_or_else(|| NativeError::new("std/fmt.from_int expects Int"))?;
    let reservation = context.reserve_opaque_allocation(fmt_payload_bytes(0)?)?;
    commit_fmt(context, native_type, Fmt::int(value), reservation)
}

pub(crate) fn native_from_float(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = fmt_type(context)?;
    let value = context
        .value(context.argument(0)?)?
        .unwrap_declared()
        .and_then(ValueRef::as_float)
        .ok_or_else(|| NativeError::new("std/fmt.from_float expects Float"))?;
    let reservation = context.reserve_opaque_allocation(fmt_payload_bytes(0)?)?;
    commit_fmt(context, native_type, Fmt::float(value), reservation)
}

pub(crate) fn native_from_atom(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = fmt_type(context)?;
    let argument = context.argument(0)?;
    let length = context
        .value(argument)?
        .unwrap_declared()
        .and_then(ValueRef::as_atom)
        .ok_or_else(|| NativeError::new("std/fmt.from_atom expects Atom"))?
        .as_str()
        .len();
    let reservation = context.reserve_opaque_allocation(fmt_payload_bytes(length)?)?;
    let text = context
        .value(argument)?
        .unwrap_declared()
        .and_then(ValueRef::as_atom)
        .expect("Atom argument was validated before reservation");
    let value = copy_string(text.as_str())?;
    commit_fmt(context, native_type, Fmt::atom(value), reservation)
}

pub(crate) fn native_concat(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = fmt_type(context)?;
    let strings = context.value(context.argument(0)?)?;
    let items = context.value(context.argument(1)?)?;
    let string_count = strings
        .sequence_len()
        .ok_or_else(|| NativeError::new("std/fmt.concat expects an Array(String)"))?;
    let item_count = items
        .sequence_len()
        .ok_or_else(|| NativeError::new("std/fmt.concat expects an Array(Fmt)"))?;
    if string_count != item_count.saturating_add(1) {
        return Err(NativeError::new(format!(
            "std/fmt.concat requires strings.len == items.len + 1, got {string_count} and {item_count}"
        )));
    }
    let text_bytes = (0..string_count).try_fold(0usize, |total, index| {
        let length = strings
            .sequence_get(index)
            .and_then(ValueRef::as_str)
            .ok_or_else(|| NativeError::new("std/fmt.concat expects an Array(String)"))?
            .as_str()
            .len();
        total
            .checked_add(length)
            .ok_or_else(|| NativeError::allocation_limit("Fmt payload size overflowed"))
    })?;
    for index in 0..item_count {
        let value = items
            .sequence_get(index)
            .ok_or_else(|| NativeError::new("std/fmt.concat expects an Array(Fmt)"))?;
        fmt_value(value).map_err(|_| NativeError::new("std/fmt.concat expects an Array(Fmt)"))?;
    }
    let payload_bytes = fmt_concat_payload_bytes(string_count, item_count, text_bytes)?;
    let reservation = context.reserve_opaque_allocation(payload_bytes)?;

    let strings_value = context.value(context.argument(0)?)?;
    let mut strings = Vec::new();
    strings
        .try_reserve_exact(string_count)
        .map_err(|_| NativeError::allocation_limit("Fmt concat String slots cannot be reserved"))?;
    for index in 0..string_count {
        let value = strings_value
            .sequence_get(index)
            .and_then(ValueRef::as_str)
            .expect("String item was validated before reservation");
        strings.push(copy_string(value.as_str())?);
    }
    let items_value = context.value(context.argument(1)?)?;
    let mut items = Vec::new();
    items
        .try_reserve_exact(item_count)
        .map_err(|_| NativeError::allocation_limit("Fmt concat item slots cannot be reserved"))?;
    for index in 0..item_count {
        items.push(
            fmt_value(
                items_value
                    .sequence_get(index)
                    .expect("Fmt item was validated before reservation"),
            )
            .expect("Fmt item was validated before reservation")
            .clone(),
        );
    }
    commit_fmt(
        context,
        native_type,
        Fmt::concat(strings, items),
        reservation,
    )
}

pub(crate) fn native_render(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let value = fmt_argument(context, 0)?;
    let length = measure_fmt(&value)?;
    let result = context.result();
    context.set_string_exact(result, length, |output| write_fmt(&value, output, 0))
}

pub(crate) fn render_value(value: ValueRef<'_>) -> Result<String, NativeError> {
    let value = fmt_value(value)?;
    let length = measure_fmt(value)?;
    let mut output = string_with_capacity(length)?;
    write_fmt(value, &mut output, 0)?;
    debug_assert_eq!(output.len(), length);
    Ok(output)
}
