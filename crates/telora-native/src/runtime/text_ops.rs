use super::*;

impl Runtime {
    pub(super) fn owned_string(&mut self, ty: TypeId, loc: Location, text: String) -> Result<Value> {
        if text.len() > 14 {
            self.charge_allocation(text.len(), 1, std::mem::size_of::<RawStringItem>())?;
        }
        self.precharged_string(ty, loc, text)
    }

    fn string_buffer(&self, length: usize) -> Result<String> {
        u32::try_from(length).map_err(|_| "string length overflow")?;
        if length > 14 {
            self.charge_allocation(length, 1, std::mem::size_of::<RawStringItem>())?;
        }
        let mut text = String::new();
        text.try_reserve_exact(length).map_err(|_| "String allocation failed")?;
        Ok(text)
    }

    fn precharged_string(&mut self, ty: TypeId, loc: Location, text: String) -> Result<Value> {
        self.expect(ty, Kind::String)?;
        if text.len() <= 14 {
            return self.string(ty, loc, &text);
        }
        let len = u32::try_from(text.len()).map_err(|_| "string length overflow")?;
        let slot =
            u32::try_from(self.work.strings.entries.len()).map_err(|_| "String table overflow")?;
        let heap = HeapRef::new(World::Work, slot)?.raw();
        self.work.strings.entries.push(RawStringItem {
            bytes: text.into_bytes(),
        });
        self.pack(
            ty,
            loc,
            &[1 | (u64::from(heap) << 32), u64::from(len) << 32],
        )
    }
    /// A substring of a heap String keeps the original backing allocation.
    pub fn string_slice(
        &self,
        value: &Value,
        start: usize,
        end: usize,
        loc: Location,
    ) -> Result<Value> {
        let text = self.text(value.as_ref())?;
        let slice = text
            .as_str()
            .get(start..end)
            .ok_or("String slice is outside UTF-8 boundaries")?;
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&value.words[2].to_le_bytes());
        bytes[8..].copy_from_slice(&value.words[3].to_le_bytes());
        if bytes[0] == 0 {
            bytes.fill(0);
            bytes[1] = slice.len() as u8;
            bytes[2..2 + slice.len()].copy_from_slice(slice.as_bytes());
        } else {
            let base = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
            let start = base
                .checked_add(u32::try_from(start).map_err(|_| "String slice overflow")?)
                .ok_or("String slice overflow")?;
            let end = base
                .checked_add(u32::try_from(end).map_err(|_| "String slice overflow")?)
                .ok_or("String slice overflow")?;
            bytes[8..12].copy_from_slice(&start.to_le_bytes());
            bytes[12..].copy_from_slice(&end.to_le_bytes());
        }
        self.pack(
            value.type_id(),
            loc,
            &[
                u64::from_le_bytes(bytes[..8].try_into().unwrap()),
                u64::from_le_bytes(bytes[8..].try_into().unwrap()),
            ],
        )
    }
    pub(crate) fn text_operation(
        &mut self,
        ty: TypeId,
        loc: Location,
        operation: usize,
        arguments: &[Value],
    ) -> Result<Value> {
        if operation <= 1 {
            let separator = if operation == 0 {
                Some(self.text(arguments[1].as_ref())?)
            } else {
                None
            };
            let separator = separator.as_ref().map_or("\n", Text::as_str);
            let count = self.array_len(&arguments[0])?;
            let mut length = count.saturating_sub(1).checked_mul(separator.len()).ok_or("String join size overflowed")?;
            for index in 0..count {
                length = length.checked_add(self.text(self.array_get(&arguments[0], index)?)?.as_str().len()).ok_or("String join size overflowed")?;
            }
            let mut output = self.string_buffer(length)?;
            for index in 0..count {
                if index != 0 {
                    output.push_str(separator);
                }
                output.push_str(self.text(self.array_get(&arguments[0], index)?)?.as_str());
            }
            return self.precharged_string(ty, loc, output);
        }
        if operation == 2 || operation == 3 {
            let source = self.text(arguments[0].as_ref())?;
            let source = source.as_str();
            let separator = if operation == 2 { Some(self.text(arguments[1].as_ref())?) } else { None };
            let pieces = source.split(separator.as_ref().map_or("\n", Text::as_str));
            let length = pieces.clone().count();
            let element = self.expect(ty, Kind::Array)?.arguments[0];
            let mut words = self.array_buffer(element, length)?;
            for piece in pieces {
                let piece = if operation == 3 { piece.strip_suffix('\r').unwrap_or(piece) } else { piece };
                let start = piece.as_ptr() as usize - source.as_ptr() as usize;
                let value = self.string_slice(&arguments[0], start, start + piece.len(), loc)?;
                self.validate(value.as_ref(), element)?;
                words.extend_from_slice(value.words());
            }
            let id = self.push_precharged_words(Table::Arrays, words)?;
            return self.pack(ty, loc, &[u64::from(id), length as u64]);
        }
        let source = self.text(arguments[0].as_ref())?;
        let source = source.as_str();
        if (4..=6).contains(&operation) {
            let needle = self.text(arguments[1].as_ref())?;
            let matched = match operation {
                4 => source.starts_with(needle.as_str()),
                5 => source.ends_with(needle.as_str()),
                _ => source.contains(needle.as_str()),
            };
            return self.scalar(ty, loc, u64::from(matched));
        }
        match operation {
            7 => {
                let needle = self.text(arguments[1].as_ref())?;
                let replacement = self.text(arguments[2].as_ref())?;
                let (needle, replacement) = (needle.as_str(), replacement.as_str());
                let count = source.match_indices(needle).count();
                let removed = count.checked_mul(needle.len()).ok_or("String replacement size overflowed")?;
                let added = count.checked_mul(replacement.len()).ok_or("String replacement size overflowed")?;
                let length = source.len().checked_sub(removed).and_then(|remaining| remaining.checked_add(added)).ok_or("String replacement size overflowed")?;
                let mut output = self.string_buffer(length)?;
                let mut previous = 0;
                for (start, matched) in source.match_indices(needle) {
                    output.push_str(&source[previous..start]);
                    output.push_str(replacement);
                    previous = start + matched.len();
                }
                output.push_str(&source[previous..]);
                self.precharged_string(ty, loc, output)
            }
            8 => {
                let width = usize::try_from(self.scalar_bits(arguments[1].as_ref())? as i64)
                    .map_err(|_| "String indentation width must be non-negative")?;
                let lines = source
                    .split_inclusive('\n')
                    .filter(|line| !line.trim_matches(['\r', '\n']).is_empty())
                    .count();
                let size = width
                    .checked_mul(lines)
                    .and_then(|extra| source.len().checked_add(extra))
                    .ok_or("String indentation size overflowed")?;
                let mut output = self.string_buffer(size)?;
                for line in source.split_inclusive('\n') {
                    if !line.trim_matches(['\r', '\n']).is_empty() {
                        output.extend(std::iter::repeat_n(' ', width));
                    }
                    output.push_str(line);
                }
                self.precharged_string(ty, loc, output)
            }
            9 => {
                let length = source.len().checked_add(usize::from(!source.ends_with('\n'))).ok_or("String newline size overflowed")?;
                let mut output = self.string_buffer(length)?;
                output.push_str(source);
                if !output.ends_with('\n') {
                    output.push('\n');
                }
                self.precharged_string(ty, loc, output)
            }
            10 => {
                let margin = self.text(arguments[1].as_ref())?;
                if margin.as_str().is_empty() {
                    return Err("String margin marker must not be empty".into());
                }
                let pieces = source.split_inclusive('\n').map(|line| {
                    let end = line.trim_end_matches(['\r', '\n']).len();
                    let content = &line[..end];
                    let start = content
                        .bytes()
                        .take_while(|byte| matches!(byte, b' ' | b'\t'))
                        .count();
                    (content[start..].strip_prefix(margin.as_str()).unwrap_or(content), &line[end..])
                });
                // Trimming only removes bytes, so the sum cannot exceed source.len().
                let length = pieces.clone().map(|(content, newline)| content.len() + newline.len()).sum();
                let mut output = self.string_buffer(length)?;
                for (content, newline) in pieces {
                    output.push_str(content);
                    output.push_str(newline);
                }
                self.precharged_string(ty, loc, output)
            }
            _ => Err("unknown native String operation".into()),
        }
    }
}
