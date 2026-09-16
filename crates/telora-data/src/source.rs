use alloc::borrow::Cow;
use alloc::sync::Arc;
use alloc::{string::String, vec::Vec};
use core::fmt;
use core::num::NonZeroU32;
use core::ops::Range;

pub mod coordinates;
mod text;
pub use text::SourceText;
pub use coordinates::{SourceCoordinates, LineIndex};

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SourceId(NonZeroU32);

impl SourceId {
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    const fn index(self) -> u32 {
        self.get() - 1
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TextRange {
    pub start: u32,
    pub end: u32,
}

impl TextRange {
    pub fn new(start: u32, end: u32) -> Result<Self, LocationError> {
        if start > end {
            return Err(LocationError::ReversedRange { start, end });
        }
        Ok(Self { start, end })
    }

    pub fn at(offset: u32) -> Self {
        Self {
            start: offset,
            end: offset,
        }
    }

    pub fn from_usize(range: Range<usize>) -> Result<Self, LocationError> {
        let start = u32::try_from(range.start).map_err(|_| LocationError::OffsetTooLarge)?;
        let end = u32::try_from(range.end).map_err(|_| LocationError::OffsetTooLarge)?;
        Self::new(start, end)
    }

    pub fn to_usize(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Loc {
    pub source: SourceId,
    pub start: u32,
    pub end: u32,
}

impl Loc {
    pub fn new(source: SourceId, range: TextRange) -> Self {
        Self {
            source,
            start: range.start,
            end: range.end,
        }
    }

    pub fn from_usize(source: SourceId, range: Range<usize>) -> Result<Self, LocationError> {
        Ok(Self::new(source, TextRange::from_usize(range)?))
    }

    pub fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }

    pub const fn text_range(self) -> TextRange {
        TextRange {
            start: self.start,
            end: self.end,
        }
    }
}

pub type Location = Loc;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocationError {
    CoordinateCapacity,
    OffsetTooLarge,
    SourceTooLarge,
    ReversedRange { start: u32, end: u32 },
}

impl fmt::Display for LocationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CoordinateCapacity => formatter.write_str("source exceeds location capacity (u32 source IDs, lines and UTF-8 byte offsets)"),
            Self::OffsetTooLarge => formatter.write_str("source offset exceeds u32::MAX"),
            Self::SourceTooLarge => formatter.write_str("source text exceeds u32::MAX bytes"),
            Self::ReversedRange { start, end } => {
                write!(
                    formatter,
                    "source range starts at {start} after ending at {end}"
                )
            }
        }
    }
}

impl core::error::Error for LocationError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Located<T> {
    pub value: T,
    pub location: Location,
}

impl<T> Located<T> {
    pub fn new(value: T, location: Location) -> Self {
        Self { value, location }
    }

    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> Located<U> {
        Located::new(map(self.value), self.location)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Origin {
    Source(Location),
    Synthetic { derived_from: Option<Location> },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WithOrigin<T> {
    pub value: T,
    pub origin: Origin,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Label {
    pub location: Location,
    pub message: String,
    pub primary: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
}

impl Diagnostic {
    pub fn new(severity: Severity, message: impl Into<String>, location: Location) -> Self {
        Self {
            severity,
            message: message.into(),
            labels: vec![Label {
                location,
                message: String::new(),
                primary: true,
            }],
            notes: Vec::new(),
        }
    }

    pub fn error(message: impl Into<String>, location: Location) -> Self {
        Self::new(Severity::Error, message, location)
    }

    pub fn with_secondary(mut self, message: impl Into<String>, location: Location) -> Self {
        self.labels.push(Label {
            location,
            message: message.into(),
            primary: false,
        });
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug)]
pub struct SourceFile {
    id: SourceId,
    pub name: Arc<str>,
    text: SourceText,
    lines: Arc<LineIndex>,
}

impl SourceFile {
    fn new(
        id: SourceId,
        name: impl Into<Arc<str>>,
        text: impl AsRef<str>,
    ) -> Result<Self, LocationError> {
        let lines = Arc::new(LineIndex::new(text.as_ref())?);
        let text = SourceText::Document(crate::document::DocumentText::new(text));
        if text.byte_len() > u32::MAX as usize {
            return Err(LocationError::SourceTooLarge);
        }
        Ok(Self {
            id,
            name: name.into(),
            text,
            lines,
        })
    }

    fn from_document(
        id: SourceId,
        name: impl Into<Arc<str>>,
        text: crate::document::DocumentText,
    ) -> Result<Self, LocationError> {
        if text.byte_len() > u32::MAX as usize {
            return Err(LocationError::SourceTooLarge);
        }
        let lines = Arc::new(LineIndex::new(&text.chunks().collect::<String>())?);
        Ok(Self {
            id,
            name: name.into(),
            text: SourceText::Document(text),
            lines,
        })
    }

    pub fn slice(&self, location: Loc) -> Option<Cow<'_, str>> {
        (location.source == self.id)
            .then(|| self.text.slice(location.text_range()).ok())
            .flatten()
    }

    pub fn coordinates(&self, location: Loc) -> SourceCoordinates {
        assert_eq!(location.source, self.id);
        self.lines.pack(location)
    }

    pub fn line_index(&self) -> &Arc<LineIndex> { &self.lines }

    pub fn byte_location(&self, location: SourceCoordinates) -> Option<Loc> {
        if location.source() != self.id.get() || location.start() > location.end() { return None; }
        Some(Loc { source: self.id, start: self.lines.byte(location.start())?, end: self.lines.byte(location.end())? })
    }

    pub const fn text(&self) -> &SourceText {
        &self.text
    }

    pub const fn id(&self) -> SourceId {
        self.id
    }

    pub fn position(&self, offset: u32) -> Position {
        let offset = offset.min(self.text.byte_len() as u32);
        let point = self.lines.point(offset);
        let (line, _) = SourceCoordinates::position(point);
        let start = self.lines.byte((line as u64) << 32).unwrap();
        let end = self.lines.byte(point).unwrap();
        let column = self.text.slice(TextRange { start, end })
            .expect("registered source offset is valid").chars().count();
        Position {
            line: line as usize + 1,
            column: column + 1,
        }
    }

    pub fn utf8_position(&self, offset: u32) -> crate::document::TextPosition {
        let (line, column) = SourceCoordinates::position(self.lines.point(offset));
        crate::document::TextPosition::new(line, column)
    }

    pub fn offset(&self, line: usize, column: usize) -> Option<u32> {
        let line = line.checked_sub(1)?;
        let column = column.checked_sub(1)?;
        match &self.text {
            SourceText::Document(text) => text.scalar_offset(line, column),
            SourceText::Contiguous(text) => {
                let start = self.lines.byte((line as u64) << 32)?;
                let tail = &text[start as usize..];
                let end = tail.find(['\r', '\n']).unwrap_or(tail.len());
                let line = &tail[..end];
                let relative = line.char_indices().map(|(at, _)| at)
                    .chain(core::iter::once(line.len())).nth(column)?;
                Some(start + relative as u32)
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SourceDatabase {
    files: Vec<SourceFile>,
}

impl SourceDatabase {
    /// Register data without copying the input into an editor Rope.
    pub fn try_add_data(
        &mut self,
        name: impl Into<Arc<str>>,
        text: String,
    ) -> Result<SourceId, LocationError> {
        let id = self.next_id()?;
        let lines = Arc::new(LineIndex::new(&text)?);
        self.files.push(SourceFile { id, name: name.into(), text: SourceText::Contiguous(text), lines });
        Ok(id)
    }

    fn next_id(&self) -> Result<SourceId, LocationError> {
        if self.files.len() >= u32::MAX as usize { return Err(LocationError::CoordinateCapacity); }
        let raw = u32::try_from(self.files.len())
            .ok()
            .and_then(|length| length.checked_add(1))
            .and_then(NonZeroU32::new)
            .ok_or(LocationError::SourceTooLarge)?;
        Ok(SourceId(raw))
    }

    pub fn try_add(
        &mut self,
        name: impl Into<Arc<str>>,
        text: impl AsRef<str>,
    ) -> Result<SourceId, LocationError> {
        let id = self.next_id()?;
        self.files.push(SourceFile::new(id, name, text)?);
        Ok(id)
    }

    pub fn try_add_document(
        &mut self,
        name: impl Into<Arc<str>>,
        text: crate::document::DocumentText,
    ) -> Result<SourceId, LocationError> {
        let id = self.next_id()?;
        self.files.push(SourceFile::from_document(id, name, text)?);
        Ok(id)
    }

    pub fn add_document(
        &mut self,
        name: impl Into<Arc<str>>,
        text: crate::document::DocumentText,
    ) -> SourceId {
        self.try_add_document(name, text)
            .expect("source fits source coordinate model")
    }

    pub fn add(&mut self, name: impl Into<Arc<str>>, text: impl AsRef<str>) -> SourceId {
        self.try_add(name, text)
            .expect("source fits source coordinate model")
    }

    pub fn get(&self, id: SourceId) -> &SourceFile {
        &self.files[id.index() as usize]
    }

    /// Reuse a runtime-owned slot after all references to its previous contents
    /// have been released. Static MIR source slots must never be reused.
    pub fn replace_unreferenced(
        &mut self,
        id: SourceId,
        name: impl Into<Arc<str>>,
        text: impl AsRef<str>,
    ) -> Result<(), LocationError> {
        let file = SourceFile::new(id, name, text)?;
        self.files[id.index() as usize] = file;
        Ok(())
    }

    pub fn replace_unreferenced_data(
        &mut self,
        id: SourceId,
        name: impl Into<Arc<str>>,
        text: String,
    ) -> Result<(), LocationError> {
        let lines = Arc::new(LineIndex::new(&text)?);
        self.files[id.index() as usize] = SourceFile {
            id, name: name.into(), text: SourceText::Contiguous(text), lines,
        };
        Ok(())
    }

    pub fn files(&self) -> impl ExactSizeIterator<Item = &SourceFile> {
        self.files.iter()
    }

    pub fn render(&self, diagnostic: &Diagnostic) -> String {
        let Some(label) = diagnostic.labels.iter().find(|label| label.primary) else {
            return diagnostic.message.clone();
        };
        let Some(file) = self.files.get(label.location.source.index() as usize) else {
            return diagnostic.message.clone();
        };
        let position = file.position(label.location.start);
        let mut rendered = format!(
            "{}:{}:{}: {}",
            file.name, position.line, position.column, diagnostic.message
        );
        for secondary in diagnostic.labels.iter().filter(|label| !label.primary) {
            let Some(file) = self.files.get(secondary.location.source.index() as usize) else {
                continue;
            };
            let position = file.position(secondary.location.start);
            rendered.push_str(&format!(
                "\n  {}:{}:{}: {}",
                file.name, position.line, position.column, secondary.message
            ));
        }
        rendered
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl core::error::Error for Diagnostic {}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod coordinates_tests;
