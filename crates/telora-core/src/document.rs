use std::borrow::Cow;
use std::fmt;

use crate::source::TextRange;

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DocumentVersion(pub i64);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PositionEncoding {
    Utf8,
    Utf16,
    Utf32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TextPosition {
    pub line: u32,
    pub character: u32,
}

impl TextPosition {
    pub const fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TextEdit {
    Replace {
        range: TextRange,
        replacement: String,
    },
    Full(String),
}

#[derive(Clone, Debug)]
pub struct DocumentText {
    rope: crop::Rope,
}

impl DocumentText {
    pub fn new(text: impl AsRef<str>) -> Self {
        Self {
            rope: crop::Rope::from(text.as_ref()),
        }
    }

    pub fn byte_len(&self) -> usize {
        self.rope.byte_len()
    }

    pub fn is_empty(&self) -> bool {
        self.rope.is_empty()
    }

    pub fn is_char_boundary(&self, offset: usize) -> bool {
        offset <= self.byte_len() && self.rope.is_char_boundary(offset)
    }

    pub fn chunks(&self) -> impl DoubleEndedIterator<Item = &str> + Clone {
        self.rope.chunks()
    }

    pub fn slice_to_string(&self, range: TextRange) -> Result<String, DocumentError> {
        let range = self.validate_range(range)?;
        Ok(self.rope.byte_slice(range).to_string())
    }

    pub fn slice(&self, range: TextRange) -> Result<Cow<'_, str>, DocumentError> {
        let range = self.validate_range(range)?;
        let slice = self.rope.byte_slice(range);
        let mut chunks = slice.chunks();
        let first = chunks.next().unwrap_or("");
        if chunks.next().is_none() {
            Ok(Cow::Borrowed(first))
        } else {
            Ok(Cow::Owned(slice.to_string()))
        }
    }

    pub fn apply(&self, edits: &[TextEdit]) -> Result<Self, DocumentError> {
        let mut next = self.clone();
        for edit in edits {
            match edit {
                TextEdit::Replace { range, replacement } => {
                    let range = next.validate_range(*range)?;
                    next.rope.replace(range, replacement);
                }
                TextEdit::Full(text) => next.rope = crop::Rope::from(text.as_str()),
            }
        }
        Ok(next)
    }

    pub fn offset(
        &self,
        position: TextPosition,
        encoding: PositionEncoding,
    ) -> Result<u32, DocumentError> {
        let line = position.line as usize;
        let (line_start, line_end) = self.line_content_range(line)?;
        let line_text = self.rope.byte_slice(line_start..line_end);
        let character = position.character as usize;
        let relative = match encoding {
            PositionEncoding::Utf8 => {
                if character > line_text.byte_len() || !line_text.is_char_boundary(character) {
                    return Err(DocumentError::InvalidPosition(position));
                }
                character
            }
            PositionEncoding::Utf16 => {
                if character > line_text.utf16_len() {
                    return Err(DocumentError::InvalidPosition(position));
                }
                let byte = line_text.byte_of_utf16_code_unit(character);
                if line_text.utf16_code_unit_of_byte(byte) != character {
                    return Err(DocumentError::InvalidPosition(position));
                }
                byte
            }
            PositionEncoding::Utf32 => utf32_to_byte(line_text, character)
                .ok_or(DocumentError::InvalidPosition(position))?,
        };
        u32::try_from(line_start + relative).map_err(|_| DocumentError::TextTooLarge)
    }

    pub fn position(
        &self,
        offset: u32,
        encoding: PositionEncoding,
    ) -> Result<TextPosition, DocumentError> {
        let offset = offset as usize;
        if offset > self.byte_len() || !self.is_char_boundary(offset) {
            return Err(DocumentError::InvalidOffset(offset));
        }
        let line = self.rope.line_of_byte(offset);
        let (line_start, line_end) = self.line_content_range(line)?;
        if offset > line_end {
            return Err(DocumentError::InvalidOffset(offset));
        }
        let prefix = self.rope.byte_slice(line_start..offset);
        let character = match encoding {
            PositionEncoding::Utf8 => prefix.byte_len(),
            PositionEncoding::Utf16 => prefix.utf16_len(),
            PositionEncoding::Utf32 => prefix.chunks().map(|chunk| chunk.chars().count()).sum(),
        };
        Ok(TextPosition {
            line: u32::try_from(line).map_err(|_| DocumentError::TextTooLarge)?,
            character: u32::try_from(character).map_err(|_| DocumentError::TextTooLarge)?,
        })
    }

    pub fn line_content_offsets(&self, line: u32) -> Result<(u32, u32), DocumentError> {
        let (start, end) = self.line_content_range(line as usize)?;
        Ok((
            u32::try_from(start).map_err(|_| DocumentError::TextTooLarge)?,
            u32::try_from(end).map_err(|_| DocumentError::TextTooLarge)?,
        ))
    }

    pub(crate) fn scalar_position(&self, offset: u32) -> Option<TextPosition> {
        let offset = offset as usize;
        if offset > self.byte_len() || !self.is_char_boundary(offset) {
            return None;
        }
        let line = self.rope.line_of_byte(offset);
        let line_start = self.rope.byte_of_line(line);
        let character = self
            .rope
            .byte_slice(line_start..offset)
            .chunks()
            .map(|chunk| chunk.chars().count())
            .sum::<usize>();
        Some(TextPosition::new(
            line.try_into().ok()?,
            character.try_into().ok()?,
        ))
    }

    pub(crate) fn scalar_offset(&self, line: usize, character: usize) -> Option<u32> {
        let logical_lines = self.logical_line_count();
        if line >= logical_lines {
            return None;
        }
        let start = self.rope.byte_of_line(line);
        let end = if line + 1 < logical_lines {
            let next = self.rope.byte_of_line(line + 1);
            if next > start && self.rope.byte(next - 1) == b'\n' {
                next - 1
            } else {
                next
            }
        } else {
            self.byte_len()
        };
        let relative = utf32_to_byte(self.rope.byte_slice(start..end), character)?;
        u32::try_from(start + relative).ok()
    }

    fn validate_range(&self, range: TextRange) -> Result<std::ops::Range<usize>, DocumentError> {
        let range = range.to_usize();
        if range.start > range.end
            || range.end > self.byte_len()
            || !self.is_char_boundary(range.start)
            || !self.is_char_boundary(range.end)
        {
            return Err(DocumentError::InvalidRange(TextRange {
                start: range.start.try_into().unwrap_or(u32::MAX),
                end: range.end.try_into().unwrap_or(u32::MAX),
            }));
        }
        Ok(range)
    }

    fn line_content_range(&self, line: usize) -> Result<(usize, usize), DocumentError> {
        let logical_lines = self.logical_line_count();
        if line >= logical_lines {
            return Err(DocumentError::InvalidPosition(TextPosition::new(
                line.try_into().unwrap_or(u32::MAX),
                0,
            )));
        }
        let start = self.rope.byte_of_line(line);
        let mut end = if line + 1 < logical_lines {
            self.rope.byte_of_line(line + 1)
        } else {
            self.byte_len()
        };
        if end > start && self.rope.byte(end - 1) == b'\n' {
            end -= 1;
            if end > start && self.rope.byte(end - 1) == b'\r' {
                end -= 1;
            }
        }
        Ok((start, end))
    }

    fn logical_line_count(&self) -> usize {
        if self.is_empty() {
            1
        } else if self.rope.byte(self.byte_len() - 1) == b'\n' {
            self.rope.line_len() + 1
        } else {
            self.rope.line_len()
        }
    }
}

impl fmt::Display for DocumentText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.rope, formatter)
    }
}

#[derive(Clone, Debug)]
pub struct DocumentSnapshot {
    version: DocumentVersion,
    text: DocumentText,
}

impl DocumentSnapshot {
    pub fn new(version: DocumentVersion, text: impl AsRef<str>) -> Self {
        Self {
            version,
            text: DocumentText::new(text),
        }
    }

    pub const fn version(&self) -> DocumentVersion {
        self.version
    }

    pub const fn text(&self) -> &DocumentText {
        &self.text
    }

    pub fn changed(
        &self,
        expected: DocumentVersion,
        version: DocumentVersion,
        edits: &[TextEdit],
    ) -> Result<Self, DocumentError> {
        if self.version != expected || version <= self.version {
            return Err(DocumentError::VersionMismatch {
                expected: self.version,
                actual: expected,
                next: version,
            });
        }
        Ok(Self {
            version,
            text: self.text.apply(edits)?,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DocumentError {
    InvalidOffset(usize),
    InvalidPosition(TextPosition),
    InvalidRange(TextRange),
    TextTooLarge,
    VersionMismatch {
        expected: DocumentVersion,
        actual: DocumentVersion,
        next: DocumentVersion,
    },
}

impl fmt::Display for DocumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidOffset(offset) => {
                write!(formatter, "invalid document byte offset {offset}")
            }
            Self::InvalidPosition(position) => write!(
                formatter,
                "invalid document position {}:{}",
                position.line, position.character
            ),
            Self::InvalidRange(range) => {
                write!(
                    formatter,
                    "invalid document byte range {}..{}",
                    range.start, range.end
                )
            }
            Self::TextTooLarge => formatter.write_str("document exceeds compact position limits"),
            Self::VersionMismatch {
                expected,
                actual,
                next,
            } => write!(
                formatter,
                "document version mismatch: current {}, change based on {}, next {}",
                expected.0, actual.0, next.0
            ),
        }
    }
}

impl std::error::Error for DocumentError {}

fn utf32_to_byte(slice: crop::RopeSlice<'_>, target: usize) -> Option<usize> {
    let mut scalars = 0;
    let mut bytes = 0;
    for chunk in slice.chunks() {
        for character in chunk.chars() {
            if scalars == target {
                return Some(bytes);
            }
            scalars += 1;
            bytes += character.len_utf8();
        }
    }
    (scalars == target).then_some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_negotiated_positions_without_conflating_encodings() {
        let text = DocumentText::new("中😀\r\ncombining: e\u{301}\n");
        let end = "中😀".len() as u32;
        assert_eq!(
            text.position(end, PositionEncoding::Utf8).unwrap(),
            TextPosition::new(0, 7)
        );
        assert_eq!(
            text.position(end, PositionEncoding::Utf16).unwrap(),
            TextPosition::new(0, 3)
        );
        assert_eq!(
            text.position(end, PositionEncoding::Utf32).unwrap(),
            TextPosition::new(0, 2)
        );
        for encoding in [
            PositionEncoding::Utf8,
            PositionEncoding::Utf16,
            PositionEncoding::Utf32,
        ] {
            for offset in [0, 3, 7, 9, 10, 21, 23] {
                let position = text.position(offset, encoding).unwrap();
                assert_eq!(text.offset(position, encoding).unwrap(), offset);
            }
        }
        assert!(
            text.offset(TextPosition::new(0, 2), PositionEncoding::Utf16)
                .is_err()
        );
        assert!(text.position(8, PositionEncoding::Utf8).is_err());
        assert_eq!(
            text.offset(TextPosition::new(2, 0), PositionEncoding::Utf16)
                .unwrap(),
            text.byte_len() as u32
        );
        assert_eq!(text.line_content_offsets(0).unwrap(), (0, 7));
        assert_eq!(text.line_content_offsets(1).unwrap(), (9, 23));
        assert_eq!(text.line_content_offsets(2).unwrap(), (24, 24));
    }

    #[test]
    fn applies_ordered_edits_to_a_cow_snapshot_atomically() {
        let original = DocumentSnapshot::new(DocumentVersion(1), "hello 世界");
        let changed = original
            .changed(
                DocumentVersion(1),
                DocumentVersion(2),
                &[
                    TextEdit::Replace {
                        range: TextRange::new(0, 5).unwrap(),
                        replacement: "goodbye".into(),
                    },
                    TextEdit::Replace {
                        range: TextRange::new(7, 7).unwrap(),
                        replacement: ",".into(),
                    },
                ],
            )
            .unwrap();
        assert_eq!(original.text().to_string(), "hello 世界");
        assert_eq!(changed.text().to_string(), "goodbye, 世界");
        assert_eq!(changed.version(), DocumentVersion(2));

        let invalid = changed.changed(
            DocumentVersion(2),
            DocumentVersion(3),
            &[
                TextEdit::Replace {
                    range: TextRange::new(0, 7).unwrap(),
                    replacement: "hello".into(),
                },
                TextEdit::Replace {
                    range: TextRange::new(7, 100).unwrap(),
                    replacement: String::new(),
                },
            ],
        );
        assert!(invalid.is_err());
        assert_eq!(changed.text().to_string(), "goodbye, 世界");
    }
}
