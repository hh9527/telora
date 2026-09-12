use super::*;
use super::codec::{Codec, DecodeFailure};
use std::ops::Range;

impl Codec<'_> {
    pub(super) fn display(&mut self, input: &Value, property: TypeId) -> std::result::Result<String, DecodeFailure> {
        let capability = self.property(input.type_id(), property, input.origin())?.ok_or("text codec requires a DisplayBy property")?;
        let plan = self.properties.iter().find(|plan| plan.owner == input.type_id() && plan.property == property).copied().ok_or("codec display plan missing")?;
        if plan.display_dispatcher == 0 { return Err("codec display dispatcher missing".into()); }
        let rt = self.runtime()?;
        let index = rt.layout(property)?.field_names.iter().position(|name| name == "display").ok_or("DisplayBy has no display function")?;
        let closure = rt.field(&capability, index)?.to_owned();
        let signature = rt.layout(closure.type_id())?;
        if signature.kind != Kind::Function || signature.arguments.len() != 2 { return Err("DisplayBy has no unary display function".into()); }
        let (argument_type, output) = (signature.arguments[0], signature.arguments[1]);
        if rt.layout(argument_type)?.kind != Kind::Dyn || rt.layout(output)?.kind != Kind::Format { return Err("DisplayBy signature must be Fn(Dyn) -> Fmt".into()); }
        let mut words = vec![0; rt.layout(output)?.words].into_boxed_slice();
        let argument = self.runtime_mut()?.dynamic(argument_type, input.origin().words(), input)?;
        type Callback = unsafe extern "C" fn(*mut crate::abi::CallContext, *const u64, *mut u64, *const u64) -> u32;
        let callback = unsafe { std::mem::transmute::<usize, Callback>(plan.display_dispatcher) };
        let status = unsafe { callback(self.context, argument.words().as_ptr(), words.as_mut_ptr(), closure.words().as_ptr()) };
        if status == 1 { return Err(DecodeFailure::Failed); }
        if status != 0 { return Err("invalid display callback status".into()); }
        let rt = self.runtime()?;
        let value = Value { arena: rt.identity, words };
        rt.validate(value.as_ref(), output)?;
        Ok(rt.format_render(&value)?)
    }

    pub(super) fn parse(&mut self, result: TypeId, target: TypeId, property: &Value, input: &Value, loc: Location) -> Result<Option<Value>> {
        let property = self.runtime()?.represented_type(property.as_ref())?;
        let length = self.runtime()?.text(input.as_ref())?.as_str().len();
        match self.parse_value(target, property, input, Some(0..length), "$", 0) {
            Ok(value) => self.runtime_mut()?.named_variant(result, loc, "Ok", Some(&value)).map(Some),
            Err(DecodeFailure::Rejected(message, _)) => {
                let string_type = self.runtime()?.layout(result)?.arguments[1];
                let message = self.runtime_mut()?.owned_string(string_type, loc, message)?;
                self.runtime_mut()?.named_variant(result, loc, "Err", Some(&message)).map(Some)
            }
            Err(DecodeFailure::Blame(blame)) => {
                let (message, subjects) = self.runtime()?.blame_diagnostic(&blame)?;
                self.context.fail_with_subjects(message, blame.origin(), subjects);
                Ok(None)
            }
            Err(DecodeFailure::Failed) => Ok(None),
            Err(DecodeFailure::Runtime(error)) => Err(error),
        }
    }

    pub(super) fn parse_value(&mut self, target: TypeId, property: TypeId, input: &Value, range: Option<Range<usize>>, path: &str, depth: usize) -> std::result::Result<Value, DecodeFailure> {
        self.charge(1, input)?;
        if depth > 512 { return Err("native string parse nesting limit".into()); }
        let loc = input.origin().words();
        let layout = self.runtime()?.layout(target)?;
        if layout.optional {
            if range.is_none() { return Ok(self.runtime_mut()?.named_variant(target, loc, "None", None)?); }
            let child = layout.arguments[0];
            let value = self.parse_value(child, property, input, range, path, depth + 1)?;
            return Ok(self.runtime_mut()?.named_variant(target, loc, "Some", Some(&value))?);
        }
        let reject = |message: &str| DecodeFailure::Rejected(format!("{path}: {message}"), vec![input.origin()]);
        let range = range.ok_or_else(|| reject("required capture is absent"))?;
        let rt = self.runtime()?;
        let source = rt.text(input.as_ref())?;
        let text = source.as_str().get(range.clone()).ok_or("invalid regex capture range")?;
        match rt.type_info[target.index()].kind {
            Some("String") => {
                if range == (0..source.as_str().len()) { return Ok(input.clone()); }
                let text = text.to_owned();
                return Ok(self.runtime_mut()?.owned_string(target, loc, text)?);
            }
            Some("Int") => {
                let value = text.parse::<i64>().map_err(|_| reject("input is not a valid Int"))?;
                return Ok(rt.scalar(target, loc, value as u64)?);
            }
            Some("Float") => {
                let value = text.parse::<f64>().ok().filter(|value| value.is_finite()).ok_or_else(|| reject("input is not a finite Float"))?;
                return Ok(rt.scalar(target, loc, value.to_bits())?);
            }
            _ => {}
        }
        let capability = self.property(target, property, input.origin())?.ok_or_else(|| reject("type has no std/string.parse capability"))?;
        let rt = self.runtime()?;
        let index = rt.layout(capability.type_id())?.field_names.iter().position(|name| name == "regex").ok_or("ParseBy has no regex")?;
        let regex = rt.field(&capability, index)?.to_owned();
        let source = rt.text(input.as_ref())?;
        let captures = rt.regex_captures(&regex, &source.as_str()[range.clone()], target, property).map_err(|error| reject(&error))?;
        let mut fields = Vec::with_capacity(captures.len());
        for (name, child, capture) in captures {
            let range = capture.map(|capture| range.start + capture.start..range.start + capture.end);
            fields.push(self.parse_value(child, property, input, range, &format!("{path}.{name}"), depth + 1)?);
        }
        let value = self.runtime_mut()?.aggregate(target, loc, &fields)?;
        self.check(target, 0, &value)?;
        Ok(value)
    }
}
