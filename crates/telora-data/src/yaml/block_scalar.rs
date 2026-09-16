use super::*;

impl Parser<'_> {
    pub(super) fn block_scalar(
        &mut self,
        header: &str,
        parent: usize,
        depth: usize,
        start: usize,
    ) -> Result<DataNodeId, Diagnostic> {
        let style = header.as_bytes()[0];
        let indicators = header[1..].trim();
        let loc = self.build.loc(start..start + header.len());
        self.build.reserve(depth, loc)?;
        let mut chomping = None;
        let mut explicit = None;
        for c in indicators.chars() {
            match c {
                '+' | '-' if chomping.is_none() => chomping = Some(c),
                '1'..='9' if explicit.is_none() => {
                    explicit = Some(parent + c.to_digit(10).unwrap() as usize)
                }
                _ => return Err(Diagnostic::error("invalid YAML block scalar header", loc)),
            }
        }
        let inferred = (self.position..self.lines.len())
            .find_map(|i| {
                if self.content(i).trim().is_empty() {
                    None
                } else {
                    Some(self.lines[i].indent)
                }
            })
            .filter(|indent| *indent > parent);
        let indent = explicit.or(inferred).unwrap_or(parent + 1);
        let mut output = String::new();
        let mut pending_newlines = 0usize;
        let mut previous: Option<(bool, bool)> = None;
        let mut end = start + header.len();
        while let Some(line) = self.lines.get(self.position).copied() {
            let content = self.content(self.position);
            if !content.trim().is_empty() && line.indent < indent {
                break;
            }
            let raw = if line.indent <= parent && content.trim().is_empty() {
                Cow::Borrowed("")
            } else {
                self.text((line.start + indent).min(line.end)..line.end)
            };
            let empty = raw.is_empty();
            let more = line.indent > indent;
            let piece_loc = self.build.loc(line.start..line.end);
            if let Some((prev_empty, prev_more)) = previous {
                if style == b'|' || prev_empty || empty || prev_more || more {
                    pending_newlines += 1;
                } else {
                    self.flush_newlines(&mut output, &mut pending_newlines, piece_loc)?;
                    self.build.append(&mut output, " ", piece_loc)?;
                }
            }
            if !empty {
                self.flush_newlines(&mut output, &mut pending_newlines, piece_loc)?;
                self.build.append(&mut output, &raw, piece_loc)?;
            }
            previous = Some((empty, more));
            end = line.end;
            self.position += 1;
        }
        match chomping {
            Some('-') => {}
            Some('+') => {
                pending_newlines += 1;
                self.flush_newlines(&mut output, &mut pending_newlines, loc)?;
            }
            _ if previous.is_some() => self.build.append(&mut output, "\n", loc)?,
            _ => {}
        }
        Ok(self
            .build
            .plan
            .scalar(DataScalar::String(output), self.build.loc(start..end)))
    }
    fn flush_newlines(
        &mut self,
        output: &mut String,
        count: &mut usize,
        loc: Location,
    ) -> Result<(), Diagnostic> {
        while *count > 0 {
            self.build.append(output, "\n", loc)?;
            *count -= 1;
        }
        Ok(())
    }
}
