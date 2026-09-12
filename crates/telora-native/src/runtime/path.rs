use super::*;

// Language paths are lexical slash-separated paths, independent of host OS.
fn normalize(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut components = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {},
            ".." if components.last().is_some_and(|last| *last != "..") => { components.pop(); },
            ".." if !absolute => components.push(component),
            ".." => {},
            _ => components.push(component),
        }
    }
    if absolute { format!("/{}", components.join("/")) }
    else if components.is_empty() { ".".into() }
    else { components.join("/") }
}

impl Runtime {
    pub(super) fn path(&mut self, ty: TypeId, input: &Value, loc: Location, operation: usize) -> Result<Value> {
        let normalized = if operation == 0 {
            let mut joined = String::new();
            for index in 0..self.array_len(input)? {
                let part = self.text(self.array_get(input, index)?)?;
                let part = part.as_str();
                if part.starts_with('/') { joined.clear(); }
                else if !joined.is_empty() && !joined.ends_with('/') { joined.push('/'); }
                joined.push_str(part);
            }
            normalize(&joined)
        } else { normalize(self.text(input.as_ref())?.as_str()) };
        let result = match operation {
            0 | 1 => return self.owned_string(ty, loc, normalized),
            2 => match normalized.as_str() {
                "." | "/" => None,
                path => Some(match path.rfind('/') {
                    Some(0) => "/",
                    Some(index) => &path[..index],
                    None => ".",
                }),
            },
            3 => match normalized.as_str() {
                "." | "/" | ".." => None,
                path => path.rsplit('/').next(),
            },
            _ => return Err("unknown native path operation".into()),
        };
        let payload = result.map(|text| {
            let string = self.layout(ty)?.arguments[0];
            self.string(string, loc, text)
        }).transpose()?;
        self.named_variant(ty, loc, if payload.is_some() { "Some" } else { "None" }, payload.as_ref())
    }
}
