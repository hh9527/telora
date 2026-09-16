//! Source storage is independent of editor snapshots. Data sources retain the
//! original contiguous allocation; editable code sources retain their Rope.
use super::TextRange;
use crate::document::{DocumentError, DocumentText};
use alloc::{borrow::Cow, string::String};
use core::fmt;

#[derive(Clone, Debug)]
pub enum SourceText {
    Document(DocumentText),
    Contiguous(String),
}

impl SourceText {
    pub fn document(&self) -> Option<&DocumentText> {
        match self {
            Self::Document(text) => Some(text),
            Self::Contiguous(_) => None,
        }
    }
    pub fn contiguous(&self) -> Option<&str> {
        match self {
            Self::Contiguous(text) => Some(text),
            Self::Document(_) => None,
        }
    }
    pub fn byte_len(&self) -> usize {
        match self {
            Self::Document(text) => text.byte_len(),
            Self::Contiguous(text) => text.len(),
        }
    }
    pub fn chunks(&self) -> impl DoubleEndedIterator<Item = &str> + Clone {
        self.document()
            .into_iter()
            .flat_map(|text| text.chunks())
            .chain(self.contiguous())
    }
    pub fn slice(&self, range: TextRange) -> Result<Cow<'_, str>, DocumentError> {
        match self {
            Self::Document(text) => text.slice(range),
            Self::Contiguous(text) => text
                .get(range.to_usize())
                .map(Cow::Borrowed)
                .ok_or(DocumentError::InvalidRange(range)),
        }
    }
}

impl fmt::Display for SourceText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Document(text) => text.fmt(f),
            Self::Contiguous(text) => text.fmt(f),
        }
    }
}
