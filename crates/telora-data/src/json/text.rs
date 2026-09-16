//! Offsets survive buffer growth; no node owns or reference-counts text.
use alloc::string::String;
use core::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextSpan {
    Source(Range<usize>),
    Decoded(Range<usize>),
}

impl TextSpan {
    pub fn resolve<'a>(&self, source: &'a str, decoded: &'a str) -> &'a str {
        match self {
            Self::Source(range) => &source[range.clone()],
            Self::Decoded(range) => &decoded[range.clone()],
        }
    }
}

#[derive(Debug)]
pub struct ParseCtx<'a> {
    pub(crate) src: &'a str,
    pub(crate) decoded: String,
}

impl<'a> ParseCtx<'a> {
    pub fn new(src: &'a str) -> Self {
        Self {
            src,
            decoded: String::new(),
        }
    }

    pub fn text(&self, span: &TextSpan) -> &str {
        span.resolve(self.src, &self.decoded)
    }

    pub fn decoded_bytes(&self) -> usize {
        self.decoded.len()
    }

    /// Transfer the sole decoded allocation to a publication backend.
    pub fn into_decoded(self) -> String {
        self.decoded
    }
}
